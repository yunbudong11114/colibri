//! Interactive model setup. Writes only after validation and explicit confirmation.
use crate::{
    config::{default_config_path, AgentConfig},
    model::build_model_for_path,
};
use std::{
    fs,
    io::{BufRead, Write},
    path::{Path, PathBuf},
};
use toml_edit::{value, DocumentMut};
const INPUT_LIMIT: u64 = 8192;

fn line(input: &mut impl BufRead) -> Result<Option<String>, String> {
    let mut bytes = Vec::new();
    let n = std::io::Read::take(input, INPUT_LIMIT + 1)
        .read_until(b'\n', &mut bytes)
        .map_err(|e| e.to_string())?;
    if n == 0 {
        return Ok(None);
    }
    if bytes.len() > INPUT_LIMIT as usize {
        return Err("输入过长，未保存配置".into());
    }
    let text = String::from_utf8(bytes)
        .map_err(|_| "输入不是 UTF-8")?
        .trim()
        .to_string();
    if text == ":cancel" {
        Ok(None)
    } else {
        Ok(Some(text))
    }
}
fn prompt(
    input: &mut impl BufRead,
    out: &mut impl Write,
    label: &str,
) -> Result<Option<String>, String> {
    write!(out, "{label}")
        .and_then(|_| out.flush())
        .map_err(|e| e.to_string())?;
    line(input)
}

/// Restore terminal settings on all ordinary exit, EOF, error and Ctrl-C paths.
struct SecretTerminal(libc::termios);
impl SecretTerminal {
    fn enter() -> Result<Self, String> {
        let mut previous = unsafe { std::mem::zeroed() };
        if unsafe { libc::tcgetattr(libc::STDIN_FILENO, &mut previous) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        let mut secret = previous;
        secret.c_lflag &= !(libc::ECHO | libc::ICANON | libc::ISIG | libc::IEXTEN);
        secret.c_cc[libc::VMIN] = 1;
        secret.c_cc[libc::VTIME] = 0;
        if unsafe { libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &secret) } != 0 {
            return Err(std::io::Error::last_os_error().to_string());
        }
        Ok(Self(previous))
    }
}
impl Drop for SecretTerminal {
    fn drop(&mut self) {
        unsafe {
            libc::tcsetattr(libc::STDIN_FILENO, libc::TCSANOW, &self.0);
        }
    }
}
fn secret(
    input: &mut impl BufRead,
    out: &mut impl Write,
    terminal: bool,
    has_existing: bool,
) -> Result<Option<String>, String> {
    let guard = if terminal {
        Some(SecretTerminal::enter()?)
    } else {
        None
    };
    write!(
        out,
        "API Key（不回显；{}；- 清空）：",
        if has_existing {
            "留空保留原值"
        } else {
            "可留空"
        }
    )
    .and_then(|_| out.flush())
    .map_err(|e| e.to_string())?;
    let result = if terminal {
        let mut bytes = Vec::new();
        loop {
            let mut byte = [0u8];
            if input.read(&mut byte).map_err(|e| e.to_string())? == 0 {
                break Ok(None);
            }
            match byte[0] {
                3 | 4 | 27 => break Ok(None),
                b'\r' | b'\n' => {
                    let s = String::from_utf8(bytes)
                        .map_err(|_| "密钥不是 UTF-8")?
                        .trim()
                        .to_string();
                    break Ok((s != ":cancel").then_some(s));
                }
                8 | 127 => {
                    bytes.pop();
                    while !bytes.is_empty() && std::str::from_utf8(&bytes).is_err() {
                        bytes.pop();
                    }
                }
                b if b >= 32 => {
                    if bytes.len() >= INPUT_LIMIT as usize {
                        return Err("密钥输入过长，未保存配置".into());
                    }
                    bytes.push(b)
                }
                _ => {}
            }
        }
    } else {
        line(input)
    };
    drop(guard);
    if terminal {
        writeln!(out).map_err(|e| e.to_string())?;
    }
    result
}
fn endpoint_display(raw: &str) -> String {
    match url::Url::parse(raw) {
        Ok(mut url) => {
            let _ = url.set_username("");
            let _ = url.set_password(None);
            if url.query().is_some() {
                url.set_query(Some("hidden"));
            }
            url.set_fragment(None);
            url.to_string()
        }
        Err(_) => "未设置".into(),
    }
}
fn read_document(path: &Path) -> Result<DocumentMut, String> {
    match fs::read_to_string(path) {
        Ok(text) => text
            .parse()
            .map_err(|_| "配置文件格式错误，请先修复，未保存变更".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(DocumentMut::new()),
        Err(e) => Err(e.to_string()),
    }
}

pub(crate) fn run(
    input: &mut impl BufRead,
    out: &mut impl Write,
    path: Option<PathBuf>,
    terminal: bool,
) -> Result<i32, String> {
    let path = path.unwrap_or_else(default_config_path);
    let path = if path.is_absolute() {
        path
    } else {
        std::env::current_dir()
            .map_err(|e| e.to_string())?
            .join(path)
    };
    let original = read_document(&path)?;
    let original_model = original.get("model").map(ToString::to_string);
    let current = if path.exists() {
        AgentConfig::load(Some(&path))?
    } else {
        AgentConfig::user_defaults()
    };
    writeln!(
        out,
        "Colibri 模型配置\n配置文件：{}\n留空保留默认值；输入 :cancel 或 EOF 取消。",
        path.display()
    )
    .map_err(|e| e.to_string())?;
    let local = url::Url::parse(&current.model.base_url)
        .ok()
        .is_some_and(|u| matches!(u.host_str(), Some("127.0.0.1" | "localhost" | "[::1]")));
    let default = if local { "2" } else { "1" };
    let Some(service) = (loop {
        let Some(s) = prompt(
            input,
            out,
            &format!("1/4 服务：1 云端/自定义，2 本地 [{default}]："),
        )?
        else {
            break None;
        };
        let s = if s.is_empty() { default.to_string() } else { s };
        if s == "1" || s == "2" {
            break Some(s);
        }
        writeln!(out, "请输入 1 或 2。").map_err(|e| e.to_string())?;
    }) else {
        return cancelled(out);
    };
    let default_url = if (service == "2") == local {
        current.model.base_url.as_str()
    } else if service == "2" {
        "http://127.0.0.1:11434/v1"
    } else {
        "https://api.openai.com/v1"
    };
    let Some(base_url) = (loop {
        let Some(raw) = prompt(
            input,
            out,
            &format!("2/4 API 地址 [{}]：", endpoint_display(default_url)),
        )?
        else {
            break None;
        };
        let raw = if raw.is_empty() {
            default_url.to_string()
        } else {
            raw
        };
        if crate::http::http_endpoint(&raw).is_ok() {
            break Some(raw);
        }
        writeln!(out, "请输入有效的 http/https API 地址。").map_err(|e| e.to_string())?;
    }) else {
        return cancelled(out);
    };
    let same_endpoint =
        base_url.trim_end_matches('/') == current.model.base_url.trim_end_matches('/');
    let default_model = if same_endpoint && current.model.provider != "fake" {
        current.model.model.as_str()
    } else {
        ""
    };
    let Some(model) = (loop {
        let Some(raw) = prompt(input, out, &format!("3/4 模型 ID [{default_model}]："))? else {
            break None;
        };
        let raw = if raw.is_empty() {
            default_model.to_string()
        } else {
            raw
        };
        if !raw.is_empty() {
            break Some(raw);
        }
        writeln!(out, "模型 ID 不能为空。").map_err(|e| e.to_string())?;
    }) else {
        return cancelled(out);
    };
    let mut candidate = current.model.clone();
    candidate.provider = "openai_compatible".into();
    candidate.base_url = base_url;
    candidate.model = model;
    let retained = if same_endpoint {
        current.model.api_key.clone()
    } else {
        String::new()
    };
    writeln!(out, "4/4 设置密钥").map_err(|e| e.to_string())?;
    loop {
        let Some(raw) = secret(input, out, terminal, !retained.is_empty())? else {
            return cancelled(out);
        };
        candidate.api_key = if raw.is_empty() {
            retained.clone()
        } else if raw == "-" {
            String::new()
        } else {
            raw
        };
        match build_model_for_path(&candidate, &path) {
            Ok(_) => break,
            Err(e) => writeln!(out, "{}", e.lines().nth(1).unwrap_or("请检查模型配置"))
                .map_err(|e| e.to_string())?,
        }
    }
    writeln!(
        out,
        "\n配置格式检查通过\nAPI：{}\n模型：{}\n密钥：{}",
        endpoint_display(&candidate.base_url),
        candidate.model,
        if !candidate.api_key.is_empty() {
            "已设置"
        } else if std::env::var("COLIBRI_API_KEY").is_ok_and(|v| !v.is_empty()) {
            "使用 COLIBRI_API_KEY"
        } else {
            "未设置"
        }
    )
    .map_err(|e| e.to_string())?;
    let Some(confirm) = prompt(input, out, "保存配置？[y/N]：")? else {
        return cancelled(out);
    };
    if !matches!(confirm.to_ascii_lowercase().as_str(), "y" | "yes") {
        return cancelled(out);
    }
    save_model(&path, original_model, candidate)?;
    writeln!(
        out,
        "已保存模型配置：{}\n请重新运行原命令。",
        path.display()
    )
    .map_err(|e| e.to_string())?;
    Ok(0)
}
fn cancelled(out: &mut impl Write) -> Result<i32, String> {
    writeln!(out, "已取消，未保存模型配置。").map_err(|e| e.to_string())?;
    Ok(0)
}

fn save_model(
    path: &Path,
    original_model: Option<String>,
    candidate: crate::config::ModelConfig,
) -> Result<(), String> {
    let parent = path.parent().ok_or("配置路径无效")?;
    fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    let _lock = crate::config_io::lock(&parent.join("settings.lock")).map_err(|e| e.to_string())?;
    let mut latest = read_document(path)?;
    if latest.get("model").map(ToString::to_string) != original_model {
        return Err("模型配置已被其他入口修改，请重新运行 configure；未覆盖现有配置".into());
    }
    for (name, text) in [
        ("provider", candidate.provider),
        ("base_url", candidate.base_url),
        ("model", candidate.model),
        ("api_key", candidate.api_key),
    ] {
        latest["model"][name] = value(text);
    }
    let encoded = latest.to_string();
    let parsed = encoded.parse().map_err(|_| "配置格式错误")?;
    let mut checked = AgentConfig::user_defaults();
    crate::config::apply_toml_value(&mut checked, &parsed)?;
    crate::config_io::atomic_write(path, encoded.as_bytes()).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn concurrent_model_change_is_not_overwritten() {
        let root = crate::config_io::Scratch::new(&std::env::temp_dir()).unwrap();
        let path = root.0.join("config.toml");
        fs::write(&path, "[model]\nprovider='fake'\nmodel='newer'\n").unwrap();
        let before = fs::read(&path).unwrap();
        assert!(save_model(&path, None, AgentConfig::default().model)
            .unwrap_err()
            .contains("其他入口修改"));
        assert_eq!(fs::read(&path).unwrap(), before);
    }
}
