use crate::process_output::{run_bounded_command, CommandRunError};
use serde_json::{Map, Value};
use std::env;
use std::fs::{self, File};
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::text;
use crate::config_io::Scratch;
use crate::http::http_endpoint as endpoint;

const NCNN: [&str; 7] = [
    "tokens.txt",
    "encoder.ncnn.param",
    "encoder.ncnn.bin",
    "decoder.ncnn.param",
    "decoder.ncnn.bin",
    "joiner.ncnn.param",
    "joiner.ncnn.bin",
];
const ONNX: [&str; 3] = ["sherpa-onnx", "model.int8.onnx", "tokens.txt"];
const SAMPLE_RATE: u32 = 16000;
const MAX_FRAMES: usize = 60 * SAMPLE_RATE as usize;

enum Backend {
    Ncnn(PathBuf),
    Onnx(PathBuf),
    Online {
        url: String,
        key: String,
        model: String,
    },
}

impl Backend {
    fn select(values: &Map<String, Value>) -> Result<Self, String> {
        let mode = text(values, "speech.backend");
        let service = text(values, "speech.base_url");
        if mode == "offline" || (mode == "auto" && service.is_empty()) {
            let directory = env::var_os("COLIBRI_OFFLINE_SPEECH")
                .map(PathBuf::from)
                .or_else(|| {
                    env::current_exe()
                        .ok()
                        .and_then(|p| p.parent().map(|p| p.join("offline-speech")))
                })
                .unwrap_or_default();
            if directory.join("sherpa-ncnn").is_file()
                && NCNN.iter().all(|f| directory.join(f).is_file())
            {
                return Ok(Self::Ncnn(directory));
            }
            if ONNX.iter().all(|f| directory.join(f).is_file()) {
                return Ok(Self::Onnx(directory));
            }
            if mode == "offline" {
                return Err("离线语音模型未安装，请安装包含语音模型的 colibri 包。".into());
            }
        }
        let url = if service.is_empty() {
            text(values, "model.base_url")
        } else {
            service
        };
        let mut key = text(values, "speech.api_key").to_owned();
        if key.is_empty() && service.is_empty() {
            key = text(values, "model.api_key").to_owned();
            if key.is_empty() {
                key = env::var("COLIBRI_API_KEY").unwrap_or_default();
            }
        }
        if endpoint(url)?.host_str() == Some("api.openai.com") && key.is_empty() {
            return Err("缺少语音 API 密钥。请在设置中填写语音 API。".into());
        }
        Ok(Self::Online {
            url: url.trim_end_matches('/').into(),
            key,
            model: text(values, "speech.model").into(),
        })
    }

    fn label(&self) -> &'static str {
        match self {
            Self::Online { .. } => "online",
            _ => "offline",
        }
    }

    fn transcribe(&self, recording: &Path) -> Result<String, String> {
        let (root, mut command, ncnn) = match self {
            Self::Online { url, key, model } => return online(url, key, model, recording),
            Self::Ncnn(root) => {
                let mut command = Command::new(root.join("sherpa-ncnn"));
                command
                    .args(NCNN.iter().map(|name| root.join(name)))
                    .arg(recording)
                    .arg("2");
                (root, command, true)
            }
            Self::Onnx(root) => {
                let mut command = Command::new(root.join("sherpa-onnx"));
                command
                    .args(["--num-threads=1", "--enable-endpoint=false"])
                    .arg(format!(
                        "--zipformer2-ctc-model={}",
                        root.join("model.int8.onnx").display()
                    ))
                    .arg(format!("--tokens={}", root.join("tokens.txt").display()))
                    .arg(recording);
                (root, command, false)
            }
        };
        command
            .env("LD_LIBRARY_PATH", root.join("lib"))
            .stdin(Stdio::null());
        let output =
            run_bounded_command(command, Duration::from_secs(90), 32000).map_err(|error| {
                match error {
                    CommandRunError::Timeout => "离线转写超过 90 秒，请尝试更短的录音。",
                    CommandRunError::Io(_) => "无法启动离线语音引擎，请检查安装包。",
                }
            })?;
        if !output.success {
            return Err("离线语音引擎运行失败，请检查模型和可用内存。".into());
        }
        for line in output.text.lines().rev() {
            if ncnn {
                if let Some(value) = line.strip_prefix("text:") {
                    return Ok(normalize_nas(value.trim()));
                }
            } else if let Ok(value) = serde_json::from_str::<Value>(line.trim()) {
                if let Some(text) = value["text"].as_str() {
                    return Ok(text.trim().into());
                }
            }
        }
        Err("离线语音引擎没有返回识别结果。".into())
    }
}

fn normalize_nas(text: &str) -> String {
    let mut output = String::new();
    let mut tail = text;
    while let Some((before, after)) = tail.split_once("纳斯") {
        let storage = [
            "同步到",
            "备份到",
            "保存到",
            "上传到",
            "传到",
            "写入",
            "挂载到",
            "连接到",
        ]
        .iter()
        .any(|word| before.trim_end().ends_with(word));
        let end = after
            .chars()
            .next()
            .is_none_or(|ch| ch.is_whitespace() || "，。！？,.!?".contains(ch));
        if storage && end {
            output.push_str(before.trim_end());
            output.push_str(" NAS");
        } else {
            output.push_str(before);
            output.push_str("纳斯");
        }
        tail = after;
    }
    output.push_str(tail);
    output
}

pub struct Recording {
    child: Child,
    scratch: Scratch,
    backend: Backend,
}

impl Recording {
    pub fn start(values: &Map<String, Value>) -> Result<Self, String> {
        let backend = Backend::select(values)?;
        let scratch = Scratch::new(&env::temp_dir())
            .or_else(|_| Scratch::new(Path::new("/tmp")))
            .map_err(|_| "无法创建录音临时目录")?;
        let path = scratch.0.join("speech.wav");
        let device = env::var("COLIBRI_CAPTURE_DEVICE").ok();
        let board = env::var("COLIBRI_DEVICE_AUDIO")
            .map(|v| v == "1")
            .unwrap_or_else(|_| {
                env::current_exe().is_ok_and(|path| path.starts_with("/usr/lib/colibri-cardputer"))
            });
        let runtime = env::var_os("XDG_RUNTIME_DIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(format!("/run/user/{}", unsafe { libc::getuid() })));
        let pulse = board
            && device.is_none()
            && runtime.join("pulse/native").exists()
            && Path::new("/usr/bin/parecord").is_file();
        let mut command = if pulse {
            let mut cmd = Command::new("timeout");
            cmd.args([
                "--foreground",
                "--signal=INT",
                "--kill-after=3s",
                "60s",
                "parecord",
                "--file-format=wav",
                "--rate=16000",
                "--channels=2",
                "--format=s16le",
                "--latency-msec=50",
                "--device=alsa_input.platform-sound.stereo-fallback",
            ])
            .env("XDG_RUNTIME_DIR", runtime);
            cmd
        } else {
            let mut cmd = Command::new("arecord");
            let device = device.as_deref().unwrap_or(if board {
                "plughw:CARD=ES8389Audio,DEV=0"
            } else {
                "default"
            });
            cmd.args([
                "-q", "-D", device, "-t", "wav", "-f", "S16_LE", "-r", "16000", "-c", "1", "-d",
                "60",
            ]);
            cmd
        };
        command
            .arg(&path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0);
        #[cfg(target_os = "linux")]
        unsafe {
            command.pre_exec(|| {
                if libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGKILL) != 0 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let child = command
            .spawn()
            .map_err(|_| "无法启动录音，请检查 ALSA 或 PipeWire。")?;
        let mut recording = Self {
            child,
            scratch,
            backend,
        };
        if pulse {
            let limit = Instant::now() + Duration::from_secs(5);
            loop {
                if fs::metadata(&path).is_ok_and(|m| m.len() > 44) {
                    break;
                }
                if Instant::now() >= limit || recording.child.try_wait().ok().flatten().is_some() {
                    return Err("麦克风未开始采样，请检查系统音频输入。".into());
                }
                thread::sleep(Duration::from_millis(50));
            }
        }
        Ok(recording)
    }

    pub fn backend(&self) -> &'static str {
        self.backend.label()
    }

    pub fn finish(mut self, cancel: bool) -> Result<String, String> {
        self.stop(libc::SIGINT, Duration::from_secs(3));
        if cancel {
            return Ok(String::new());
        }
        let source = self.scratch.0.join("speech.wav");
        let target = self.scratch.0.join("finished.wav");
        let mut bytes = Vec::new();
        File::open(source)
            .and_then(|file| {
                file.take((MAX_FRAMES * 4 + 65536) as u64)
                    .read_to_end(&mut bytes)
            })
            .map_err(|_| "麦克风录音失败，请检查系统音频输入。")?;
        fs::write(&target, normalize_wave(&bytes)?).map_err(|e| e.to_string())?;
        self.backend.transcribe(&target)
    }

    fn stop(&mut self, signal: i32, wait: Duration) {
        if !matches!(self.child.try_wait(), Ok(None)) {
            return;
        }
        let group = -(self.child.id() as i32);
        unsafe {
            libc::kill(group, signal);
        }
        let deadline = Instant::now() + wait;
        while Instant::now() < deadline {
            if !matches!(self.child.try_wait(), Ok(None)) {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        unsafe {
            libc::kill(group, libc::SIGKILL);
        }
        let _ = self.child.wait();
    }
}

impl Drop for Recording {
    fn drop(&mut self) {
        self.stop(libc::SIGKILL, Duration::ZERO);
    }
}

fn normalize_wave(bytes: &[u8]) -> Result<Vec<u8>, String> {
    if !bytes.starts_with(b"RIFF") || bytes.get(8..12) != Some(b"WAVE") {
        return Err("录音格式无效".into());
    }
    let mut offset = 12;
    let mut channels = 0usize;
    let mut data = &[][..];
    while let Some(header) = bytes.get(offset..offset + 8) {
        let length = u32::from_le_bytes(header[4..8].try_into().unwrap()) as usize;
        offset += 8;
        let end = offset.saturating_add(length).min(bytes.len());
        let chunk = &bytes[offset..end];
        if &header[..4] == b"fmt " && chunk.len() >= 16 {
            let field = |start| u16::from_le_bytes(chunk[start..start + 2].try_into().unwrap());
            let rate = u32::from_le_bytes(chunk[4..8].try_into().unwrap());
            if field(0) != 1 || rate != SAMPLE_RATE || field(14) != 16 || !matches!(field(2), 1 | 2)
            {
                return Err("录音不是 16 kHz PCM".into());
            }
            channels = field(2) as usize;
        } else if &header[..4] == b"data" {
            data = chunk;
            break;
        }
        offset = end + length % 2;
    }
    if channels == 0 || data.len() < channels * 2 {
        return Err("录音为空，请重试".into());
    }
    let mut samples: Vec<i16> = data
        .chunks_exact(channels * 2)
        .take(MAX_FRAMES)
        .map(|frame| i16::from_le_bytes([frame[0], frame[1]]))
        .collect();
    if channels == 2 {
        let peak = samples
            .iter()
            .map(|sample| i32::from(*sample).abs())
            .max()
            .unwrap_or(0);
        if (1..22000).contains(&peak) {
            let gain = (22000.0 / f64::from(peak)).min(8.0);
            for sample in &mut samples {
                *sample = (f64::from(*sample) * gain).round() as i16;
            }
        }
    }
    let length = (samples.len() * 2) as u32;
    let mut wave = Vec::with_capacity(44 + length as usize);
    wave.extend(b"RIFF");
    wave.extend((length + 36).to_le_bytes());
    wave.extend(b"WAVEfmt ");
    wave.extend(16u32.to_le_bytes());
    wave.extend(1u16.to_le_bytes());
    wave.extend(1u16.to_le_bytes());
    wave.extend(SAMPLE_RATE.to_le_bytes());
    wave.extend((SAMPLE_RATE * 2).to_le_bytes());
    wave.extend(2u16.to_le_bytes());
    wave.extend(16u16.to_le_bytes());
    wave.extend(b"data");
    wave.extend(length.to_le_bytes());
    for sample in samples {
        wave.extend(sample.to_le_bytes());
    }
    Ok(wave)
}

fn online(url: &str, key: &str, model: &str, path: &Path) -> Result<String, String> {
    let boundary = format!(
        "colibri-{}",
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    );
    let mut data = format!("--{boundary}\r\nContent-Disposition: form-data; name=\"model\"\r\n\r\n{model}\r\n--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"speech.wav\"\r\nContent-Type: audio/wav\r\n\r\n").into_bytes();
    data.extend(fs::read(path).map_err(|e| e.to_string())?);
    data.extend(format!("\r\n--{boundary}--\r\n").bytes());
    let agent = ureq::AgentBuilder::new()
        .redirects(0)
        .timeout_connect(Duration::from_secs(5))
        .build();
    let address = format!("{url}/audio/transcriptions");
    let mut request = agent.post(&address).timeout(Duration::from_secs(20)).set(
        "Content-Type",
        &format!("multipart/form-data; boundary={boundary}"),
    );
    if !key.is_empty() {
        request = request.set("Authorization", &format!("Bearer {key}"));
    }
    let response = request.send_bytes(&data).map_err(|error| match error {
        ureq::Error::Status(401 | 403, _) => "转写服务拒绝访问，请检查密钥和模型权限。".into(),
        ureq::Error::Status(404 | 405, _) => "转写接口不可用，请检查 API 地址和模型。".into(),
        ureq::Error::Status(status, _) => format!("转写服务返回 HTTP {status}。"),
        _ => "连接转写服务失败或超时。".into(),
    })?;
    if !(200..300).contains(&response.status()) {
        return Err("转写接口返回非成功状态。".into());
    }
    let value: Value = serde_json::from_reader(response.into_reader().take(256 * 1024))
        .map_err(|_| "转写接口返回格式有误。")?;
    value["text"]
        .as_str()
        .map(|text| text.trim().to_owned())
        .ok_or_else(|| "转写接口返回格式有误。".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nas_requires_a_storage_context() {
        assert_eq!(
            normalize_nas("把记忆同步到纳斯。问问纳斯"),
            "把记忆同步到 NAS。问问纳斯"
        );
    }

    #[test]
    fn stereo_left_channel_and_gain() {
        let mut bytes = b"RIFF\0\0\0\0WAVEfmt \x10\0\0\0\x01\0\x02\0\x80\x3e\0\0\0\xfa\0\0\x04\0\x10\0data\x04\0\0\0".to_vec();
        bytes.extend(3000i16.to_le_bytes());
        bytes.extend((-30000i16).to_le_bytes());
        let normalized = normalize_wave(&bytes).unwrap();
        assert_eq!(
            u16::from_le_bytes(normalized[22..24].try_into().unwrap()),
            1
        );
        assert_eq!(
            i16::from_le_bytes(normalized[44..46].try_into().unwrap()),
            22000
        );
    }
}
