//! Unreleased API speech transport. Excluded from default builds.
use serde_json::Value;
use std::{
    fs,
    io::Read,
    path::Path,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

pub(super) fn transcribe(url: &str, key: &str, model: &str, path: &Path) -> Result<String, String> {
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
