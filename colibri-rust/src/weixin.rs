use std::fs;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use aes::cipher::{BlockDecrypt, BlockEncrypt, KeyInit};
use aes::Aes128;
use serde_json::json;

use crate::config::AgentConfig;
use crate::http::{json_string_field, request_binary, request_json};
use crate::messages::MediaPart;
use crate::model::escape_json;
use crate::terminal_qr::render_terminal_qr;
use crate::tools::content_type_for_path;

const WEIXIN_CHANNEL_VERSION: &str = "2.1.1";
const WEIXIN_ILINK_APP_ID: &str = "bot";
const WEIXIN_CLIENT_VERSION: &str = "131329";
const WEIXIN_MEDIA_MAX_BYTES: usize = 25 * 1024 * 1024;
const WEIXIN_DEFAULT_CDN_BASE_URL: &str = "https://novac2c.cdn.weixin.qq.com/c2c";
const MEDIA_TEMP_DIR: &str = "/tmp/colibri/media";
const MEDIA_RETENTION_SECONDS: u64 = 24 * 60 * 60;
const MEDIA_MAX_TOTAL_BYTES: usize = 256 * 1024 * 1024;

#[derive(Clone, Debug)]
pub struct WeixinAuthResult {
    pub token: String,
    pub user_id: String,
    pub account_id: String,
    pub base_url: String,
}

#[derive(Clone, Debug)]
pub struct InboundWeixinMessage {
    pub sender_id: String,
    pub text: String,
    pub context_token: String,
    pub message_id: String,
    pub media: Vec<MediaPart>,
}

pub fn parse_weixin_updates<F>(
    config: &AgentConfig,
    body: &str,
    mut download_media: F,
) -> Result<(String, Vec<InboundWeixinMessage>), String>
where
    F: FnMut(&serde_json::Value) -> Result<MediaPart, String>,
{
    let data: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| format!("Weixin API response was not valid JSON: {}", error))?;
    let next = data
        .get("get_updates_buf")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .to_string();
    let mut messages = Vec::new();
    for raw in data
        .get("msgs")
        .and_then(serde_json::Value::as_array)
        .into_iter()
        .flatten()
    {
        if raw.get("message_type").and_then(serde_json::Value::as_i64) != Some(1)
            || raw.get("message_state").and_then(serde_json::Value::as_i64) != Some(2)
        {
            continue;
        }
        let sender_id = raw
            .get("from_user_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .trim();
        if sender_id.is_empty() || !weixin_sender_allowed(config, sender_id) {
            continue;
        }
        let mut texts = Vec::new();
        let mut media = Vec::new();
        for item in raw
            .get("item_list")
            .and_then(serde_json::Value::as_array)
            .into_iter()
            .flatten()
        {
            match item.get("type").and_then(serde_json::Value::as_i64) {
                Some(1) => {
                    if let Some(text) = item
                        .get("text_item")
                        .and_then(|value| value.get("text"))
                        .and_then(serde_json::Value::as_str)
                        .map(str::trim)
                        .filter(|text| !text.is_empty())
                    {
                        texts.push(text.to_string());
                    }
                }
                Some(item_type @ (2 | 4)) if media.is_empty() => {
                    if let Ok(part) = download_media(item) {
                        texts.push(if item_type == 2 {
                            format!("[image: {}]", part.filename)
                        } else {
                            format!("[file: {}]", part.filename)
                        });
                        media.push(part);
                    }
                }
                _ => {}
            }
        }
        let text = texts.join("\n");
        if text.trim().is_empty() && media.is_empty() {
            continue;
        }
        messages.push(InboundWeixinMessage {
            sender_id: sender_id.to_string(),
            text: text.trim().to_string(),
            context_token: raw
                .get("context_token")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_string(),
            message_id: raw
                .get("message_id")
                .map(|value| {
                    value
                        .as_str()
                        .map(ToString::to_string)
                        .unwrap_or_else(|| value.to_string())
                })
                .unwrap_or_default(),
            media,
        });
    }
    Ok((next, messages))
}

pub fn encrypt_aes_ecb(data: &[u8], key: &[u8; 16]) -> Result<Vec<u8>, String> {
    let cipher = Aes128::new_from_slice(key).map_err(|error| error.to_string())?;
    let padding = 16 - data.len() % 16;
    let mut output = data.to_vec();
    output.extend(std::iter::repeat_n(padding as u8, padding));
    for chunk in output.chunks_exact_mut(16) {
        cipher.encrypt_block(chunk.into());
    }
    Ok(output)
}

pub fn decrypt_aes_ecb(data: &[u8], key: &[u8; 16]) -> Result<Vec<u8>, String> {
    if data.is_empty() || data.len() % 16 != 0 {
        return Err(format!("Invalid AES-ECB ciphertext length: {}", data.len()));
    }
    let cipher = Aes128::new_from_slice(key).map_err(|error| error.to_string())?;
    let mut output = data.to_vec();
    for chunk in output.chunks_exact_mut(16) {
        cipher.decrypt_block(chunk.into());
    }
    let padding = *output.last().unwrap() as usize;
    if padding == 0 || padding > 16 || padding > output.len() {
        return Err("Invalid PKCS7 padding".to_string());
    }
    if output[output.len() - padding..]
        .iter()
        .any(|value| *value as usize != padding)
    {
        return Err("Invalid PKCS7 padding bytes".to_string());
    }
    output.truncate(output.len() - padding);
    Ok(output)
}

pub fn cleanup_media_directory(root: &Path, retention_seconds: u64, max_total_bytes: usize) {
    let Ok(entries) = fs::read_dir(root) else {
        return;
    };
    let now = std::time::SystemTime::now();
    let mut retained = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.file_type().is_file() {
            continue;
        }
        let modified = metadata.modified().unwrap_or(std::time::UNIX_EPOCH);
        let expired = now
            .duration_since(modified)
            .ok()
            .is_some_and(|age| age.as_secs() >= retention_seconds);
        if expired && fs::remove_file(&path).is_ok() {
            continue;
        }
        retained.push((modified, path, metadata.len() as usize));
    }
    let mut total = retained.iter().map(|(_, _, size)| *size).sum::<usize>();
    if total <= max_total_bytes {
        return;
    }
    retained.sort_by_key(|(modified, path, _)| (*modified, path.clone()));
    for (_, path, size) in retained {
        if fs::remove_file(path).is_ok() {
            total = total.saturating_sub(size);
        }
        if total <= max_total_bytes {
            break;
        }
    }
}

pub fn permission_choice(reply: &str) -> String {
    let first = reply.split_whitespace().next().unwrap_or("0");
    if matches!(first, "0" | "1" | "2" | "3" | "4" | "5") {
        first.to_string()
    } else {
        "0".to_string()
    }
}

pub fn perform_weixin_auth<F>(
    config: &AgentConfig,
    mut emit_line: F,
) -> Result<WeixinAuthResult, String>
where
    F: FnMut(&str) -> Result<(), String>,
{
    let base_url = normalized_base_url(&config.channels_weixin.base_url);
    let qrcode_url = format!("{}ilink/bot/get_bot_qrcode?bot_type=3", base_url);
    let qr_response = request_json("GET", &qrcode_url, &weixin_headers(false, ""), None, 35)?;
    fail_on_http("Weixin get_bot_qrcode", &qr_response)?;
    let qr_payload = json_string_field(&qr_response.body, "qrcode_img_content")
        .ok_or_else(|| "Weixin auth did not return a QR payload".to_string())?;
    let qr_id = json_string_field(&qr_response.body, "qrcode")
        .ok_or_else(|| "Weixin auth did not return a QR code id".to_string())?;
    emit_line("Scan this Weixin QR code with WeChat:")?;
    if let Some(rendered) = render_terminal_qr(&qr_payload) {
        emit_line(&rendered)?;
    }
    emit_line("QR payload:")?;
    emit_line(&qr_payload)?;

    let deadline =
        Instant::now() + Duration::from_secs(config.channels_weixin.auth_timeout_seconds);
    while Instant::now() < deadline {
        let status_url = format!(
            "{}ilink/bot/get_qrcode_status?qrcode={}",
            base_url,
            url_encode(&qr_id)
        );
        let status_response =
            request_json("GET", &status_url, &weixin_headers(false, ""), None, 35)?;
        fail_on_http("Weixin get_qrcode_status", &status_response)?;
        let state = json_string_field(&status_response.body, "status").unwrap_or_default();
        if state == "confirmed" {
            let token = json_string_field(&status_response.body, "bot_token").unwrap_or_default();
            let account_id =
                json_string_field(&status_response.body, "ilink_bot_id").unwrap_or_default();
            let user_id =
                json_string_field(&status_response.body, "ilink_user_id").unwrap_or_default();
            if token.is_empty() || account_id.is_empty() {
                return Err("Weixin auth confirmed but missing token".to_string());
            }
            let base_url = json_string_field(&status_response.body, "baseurl")
                .unwrap_or_else(|| config.channels_weixin.base_url.clone());
            return Ok(WeixinAuthResult {
                token,
                user_id,
                account_id,
                base_url,
            });
        }
        if state == "expired" {
            return Err("Weixin auth QR code expired".to_string());
        }
        thread::sleep(Duration::from_secs(2));
    }
    Err("Weixin auth timed out".to_string())
}

pub fn save_weixin_auth_config(path: &Path, result: &WeixinAuthResult) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let text = fs::read_to_string(path).unwrap_or_default();
    let text = upsert_toml_section_values(
        &text,
        "channels.weixin",
        &[
            ("enabled", "true".to_string()),
            ("token", toml_string(&result.token)),
            ("base_url", toml_string(&result.base_url)),
        ],
    );
    fs::write(path, text).map_err(|error| error.to_string())
}

pub fn poll_weixin_once(
    config: &AgentConfig,
    get_updates_buf: &str,
) -> Result<(String, Vec<InboundWeixinMessage>), String> {
    if config.channels_weixin.token.is_empty() {
        return Err("channels.weixin.token is required".to_string());
    }
    let base_url = normalized_base_url(&config.channels_weixin.base_url);
    let url = format!("{}ilink/bot/getupdates", base_url);
    let body = format!(
        "{{\"get_updates_buf\":\"{}\",\"base_info\":{{\"channel_version\":\"{}\"}}}}",
        escape_json(get_updates_buf),
        WEIXIN_CHANNEL_VERSION
    );
    let response = request_json(
        "POST",
        &url,
        &weixin_headers(true, &config.channels_weixin.token),
        Some(&body),
        config.channels_weixin.poll_timeout_seconds + 5,
    )?;
    fail_on_http("Weixin getupdates", &response)?;
    let (parsed_buf, messages) = parse_weixin_updates(config, &response.body, |item| {
        download_inbound_media(config, item)
    })?;
    let next_buf = if parsed_buf.is_empty() {
        get_updates_buf.to_string()
    } else {
        parsed_buf
    };
    Ok((next_buf, messages))
}

pub fn send_weixin_text(
    config: &AgentConfig,
    to_user_id: &str,
    context_token: &str,
    text: &str,
) -> Result<(), String> {
    if text.trim().is_empty() {
        return Ok(());
    }
    let base_url = normalized_base_url(&config.channels_weixin.base_url);
    let url = format!("{}ilink/bot/sendmessage", base_url);
    let client_id = random_hex(8)?;
    let body = format!(
        "{{\"msg\":{{\"to_user_id\":\"{}\",\"client_id\":\"colibri-{}\",\"message_type\":2,\"message_state\":2,\"item_list\":[{{\"type\":1,\"text_item\":{{\"text\":\"{}\"}}}}],\"context_token\":\"{}\"}},\"base_info\":{{\"channel_version\":\"{}\"}}}}",
        escape_json(to_user_id),
        client_id,
        escape_json(text),
        escape_json(context_token),
        WEIXIN_CHANNEL_VERSION
    );
    let response = request_json(
        "POST",
        &url,
        &weixin_headers(true, &config.channels_weixin.token),
        Some(&body),
        config.channels_weixin.poll_timeout_seconds,
    )?;
    fail_on_http("Weixin sendmessage", &response)
}

pub fn download_inbound_media(
    config: &AgentConfig,
    item: &serde_json::Value,
) -> Result<MediaPart, String> {
    let (media_type, filename, media_ref) = inbound_media_info(item)?;
    let Some(media_ref) = media_ref else {
        return Err("Weixin inbound media item has no media reference".to_string());
    };
    let mut data = download_inbound_media_bytes(config, media_ref)?;
    let aes_key = decode_inbound_aes_key(
        media_ref
            .get("aes_key")
            .and_then(serde_json::Value::as_str)
            .unwrap_or(""),
    )?;
    if let Some(key) = aes_key {
        data = decrypt_aes_ecb(&data, &key)?;
    }
    if data.len() > WEIXIN_MEDIA_MAX_BYTES {
        return Err(format!(
            "Weixin inbound media is too large: {} bytes",
            data.len()
        ));
    }
    let root = Path::new(MEDIA_TEMP_DIR);
    cleanup_media_directory(
        root,
        MEDIA_RETENTION_SECONDS,
        MEDIA_MAX_TOTAL_BYTES.saturating_sub(data.len()),
    );
    fs::create_dir_all(root).map_err(|error| error.to_string())?;
    let ext = Path::new(&filename)
        .extension()
        .and_then(|value| value.to_str())
        .filter(|value| !value.is_empty())
        .map(|value| format!(".{}", value))
        .unwrap_or_else(|| ".bin".to_string());
    let path = root.join(format!("weixin-inbound-{}{}", random_hex(8)?, ext));
    fs::write(&path, data).map_err(|error| error.to_string())?;
    let content_type = content_type_for_path(Path::new(&filename));
    Ok(MediaPart::new(media_type, path, filename, content_type, ""))
}

pub fn send_weixin_media(
    config: &AgentConfig,
    to_user_id: &str,
    context_token: &str,
    media: &MediaPart,
) -> Result<(), String> {
    if media.caption.trim().is_empty() {
        upload_and_send_weixin_media(config, to_user_id, context_token, media)
    } else {
        send_weixin_text(config, to_user_id, context_token, &media.caption)?;
        upload_and_send_weixin_media(config, to_user_id, context_token, media)
    }
}

fn upload_and_send_weixin_media(
    config: &AgentConfig,
    to_user_id: &str,
    context_token: &str,
    media: &MediaPart,
) -> Result<(), String> {
    let data = fs::read(&media.path).map_err(|error| error.to_string())?;
    if data.len() > WEIXIN_MEDIA_MAX_BYTES {
        return Err(format!("Weixin media is too large: {} bytes", data.len()));
    }
    let filekey = random_hex(16)?;
    let aes_key = random_bytes_16()?;
    let aes_key_hex = hex_encode(&aes_key);
    let cipher_data = encrypt_aes_ecb(&data, &aes_key)?;
    let media_type = media_type_for_part(media);
    let base_url = normalized_base_url(&config.channels_weixin.base_url);
    let upload_url = format!("{}ilink/bot/getuploadurl", base_url);
    let upload_request = json!({
        "filekey": filekey,
        "media_type": weixin_upload_media_type(&media_type),
        "to_user_id": to_user_id,
        "rawsize": data.len(),
        "rawfilemd5": format!("{:x}", md5::compute(&data)),
        "filesize": cipher_data.len(),
        "no_need_thumb": true,
        "aeskey": aes_key_hex,
        "base_info": {"channel_version": WEIXIN_CHANNEL_VERSION},
    });
    let upload_response = request_json(
        "POST",
        &upload_url,
        &weixin_headers(true, &config.channels_weixin.token),
        Some(&upload_request.to_string()),
        config.channels_weixin.poll_timeout_seconds,
    )?;
    fail_on_http("Weixin getuploadurl", &upload_response)?;
    let upload_value: serde_json::Value = serde_json::from_str(&upload_response.body)
        .map_err(|error| format!("Weixin getuploadurl response was not valid JSON: {}", error))?;
    fail_on_api("getuploadurl", &upload_value)?;
    let mut cdn_url = upload_value
        .get("upload_full_url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    let upload_param = upload_value
        .get("upload_param")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim()
        .to_string();
    if cdn_url.is_empty() {
        if upload_param.is_empty() {
            return Err("Weixin getuploadurl returned no upload URL".to_string());
        }
        cdn_url = cdn_upload_url(WEIXIN_DEFAULT_CDN_BASE_URL, &upload_param, &filekey);
    }
    let download_param = upload_cdn(
        &cdn_url,
        &cipher_data,
        config.channels_weixin.poll_timeout_seconds,
    )?;
    let media_ref = json!({
        "encrypt_query_param": download_param,
        "aes_key": encode_base64(aes_key_hex.as_bytes()),
        "encrypt_type": 1,
    });
    let cipher_size = cipher_data.len();
    let item = match media_type.as_str() {
        "image" => json!({
            "type": 2,
            "image_item": {"media": media_ref, "mid_size": cipher_size},
        }),
        "video" => json!({
            "type": 5,
            "video_item": {"media": media_ref, "video_size": cipher_size},
        }),
        _ => json!({
            "type": 4,
            "file_item": {
                "media": media_ref,
                "file_name": media.filename,
                "len": data.len().to_string(),
            },
        }),
    };
    let send_url = format!("{}ilink/bot/sendmessage", base_url);
    let send_request = json!({
        "msg": {
            "to_user_id": to_user_id,
            "client_id": format!("colibri-{}", random_hex(8)?),
            "message_type": 2,
            "message_state": 2,
            "item_list": [item],
            "context_token": context_token,
        },
        "base_info": {"channel_version": WEIXIN_CHANNEL_VERSION},
    });
    let send_response = request_json(
        "POST",
        &send_url,
        &weixin_headers(true, &config.channels_weixin.token),
        Some(&send_request.to_string()),
        config.channels_weixin.poll_timeout_seconds,
    )?;
    fail_on_http("Weixin sendmessage", &send_response)?;
    let send_value: serde_json::Value = serde_json::from_str(&send_response.body)
        .map_err(|error| format!("Weixin sendmessage response was not valid JSON: {}", error))?;
    fail_on_api("sendmessage", &send_value)
}

fn weixin_sender_allowed(config: &AgentConfig, sender_id: &str) -> bool {
    config.channels_weixin.allow_from.is_empty()
        || config
            .channels_weixin
            .allow_from
            .iter()
            .any(|item| item == "*" || item == sender_id)
}

fn weixin_headers(auth: bool, token: &str) -> Vec<(&'static str, String)> {
    let mut headers = vec![
        ("iLink-App-Id", WEIXIN_ILINK_APP_ID.to_string()),
        ("iLink-App-ClientVersion", WEIXIN_CLIENT_VERSION.to_string()),
    ];
    if auth {
        headers.push(("AuthorizationType", "ilink_bot_token".to_string()));
        headers.push(("X-WECHAT-UIN", std::process::id().to_string()));
        if !token.is_empty() {
            headers.push(("Authorization", format!("Bearer {}", token)));
        }
    }
    headers
}

fn fail_on_http(action: &str, response: &crate::http::HttpResponse) -> Result<(), String> {
    if let Some(status) = response.status {
        if status >= 400 {
            return Err(format!(
                "{} failed with HTTP {}: {}",
                action, status, response.body
            ));
        }
    }
    Ok(())
}

fn fail_on_api(action: &str, response: &serde_json::Value) -> Result<(), String> {
    let ret = response
        .get("ret")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    let errcode = response
        .get("errcode")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    if ret != 0 || errcode != 0 {
        let errmsg = response
            .get("errmsg")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("");
        return Err(format!(
            "Weixin {} failed: ret={} errcode={} errmsg={}",
            action, ret, errcode, errmsg
        ));
    }
    Ok(())
}

fn normalized_base_url(value: &str) -> String {
    format!("{}/", value.trim_end_matches('/'))
}

fn upsert_toml_section_values(text: &str, section: &str, values: &[(&str, String)]) -> String {
    let mut lines = text
        .lines()
        .map(|line| line.to_string())
        .collect::<Vec<_>>();
    let header = format!("[{}]", section);
    let Some(start) = lines.iter().position(|line| line.trim() == header) else {
        let mut next = text.trim_end().to_string();
        if !next.is_empty() {
            next.push_str("\n\n");
        }
        next.push_str(&header);
        next.push('\n');
        for (key, value) in values {
            next.push_str(&format!("{} = {}\n", key, value));
        }
        return next;
    };
    let end = lines
        .iter()
        .enumerate()
        .skip(start + 1)
        .find(|(_, line)| line.trim().starts_with('[') && line.trim().ends_with(']'))
        .map(|(index, _)| index)
        .unwrap_or(lines.len());
    let mut seen = Vec::new();
    for line in &mut lines[start + 1..end] {
        let Some((key, _)) = line.split_once('=') else {
            continue;
        };
        let key_name = key.trim().to_string();
        if let Some((_, value)) = values
            .iter()
            .find(|(candidate, _)| *candidate == key_name.as_str())
        {
            *line = format!("{} = {}", key_name, value);
            seen.push(key_name);
        }
    }
    let mut insert_at = end;
    for (key, value) in values {
        if !seen.iter().any(|item| item == key) {
            lines.insert(insert_at, format!("{} = {}", key, value));
            insert_at += 1;
        }
    }
    lines.join("\n") + "\n"
}

fn toml_string(value: &str) -> String {
    format!("\"{}\"", escape_json(value))
}

fn url_encode(value: &str) -> String {
    value
        .bytes()
        .flat_map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                vec![byte as char]
            }
            other => format!("%{:02X}", other).chars().collect(),
        })
        .collect()
}

fn inbound_media_info(
    item: &serde_json::Value,
) -> Result<(String, String, Option<&serde_json::Value>), String> {
    let item_type = item
        .get("type")
        .and_then(serde_json::Value::as_i64)
        .unwrap_or(0);
    if item_type == 2 {
        let image_item = item
            .get("image_item")
            .and_then(serde_json::Value::as_object);
        let media_ref = image_item
            .and_then(|value| value.get("media"))
            .filter(|value| value.is_object());
        let filename = image_item
            .and_then(|value| value.get("file_name"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("image.png");
        return Ok(("image".to_string(), safe_filename(filename), media_ref));
    }
    if item_type == 4 {
        let file_item = item.get("file_item").and_then(serde_json::Value::as_object);
        let media_ref = file_item
            .and_then(|value| value.get("media"))
            .filter(|value| value.is_object());
        let filename = file_item
            .and_then(|value| value.get("file_name"))
            .and_then(serde_json::Value::as_str)
            .unwrap_or("file.bin");
        return Ok(("file".to_string(), safe_filename(filename), media_ref));
    }
    Err(format!(
        "Unsupported Weixin inbound media type: {}",
        item_type
    ))
}

fn download_inbound_media_bytes(
    config: &AgentConfig,
    media_ref: &serde_json::Value,
) -> Result<Vec<u8>, String> {
    let full_url = media_ref
        .get("full_url")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim();
    let encrypted_param = media_ref
        .get("encrypt_query_param")
        .and_then(serde_json::Value::as_str)
        .unwrap_or("")
        .trim();
    let url = if full_url.is_empty() {
        cdn_download_url(WEIXIN_DEFAULT_CDN_BASE_URL, encrypted_param)
    } else {
        full_url.to_string()
    };
    if url.is_empty() {
        return Err("Weixin inbound media has no download URL".to_string());
    }
    let response = request_binary(
        "GET",
        &url,
        &weixin_headers(true, &config.channels_weixin.token),
        None,
        None,
        config.channels_weixin.poll_timeout_seconds,
    )?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "Weixin CDN download HTTP {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body[..response.body.len().min(500)])
        ));
    }
    if response.body.len() > WEIXIN_MEDIA_MAX_BYTES {
        return Err(format!(
            "Weixin inbound media is too large: {} bytes",
            response.body.len()
        ));
    }
    Ok(response.body)
}

fn upload_cdn(url: &str, data: &[u8], timeout_seconds: u64) -> Result<String, String> {
    let response = request_binary(
        "POST",
        url,
        &[],
        Some(data),
        Some("application/octet-stream"),
        timeout_seconds,
    )?;
    if !(200..300).contains(&response.status) {
        return Err(format!(
            "Weixin CDN upload HTTP {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body[..response.body.len().min(500)])
        ));
    }
    for (name, value) in response.headers {
        if name.eq_ignore_ascii_case("X-Encrypted-Param") && !value.trim().is_empty() {
            return Ok(value.trim().to_string());
        }
    }
    Err("Weixin CDN upload missing X-Encrypted-Param".to_string())
}

fn media_type_for_part(media: &MediaPart) -> String {
    match media.media_type.as_str() {
        "image" | "video" | "file" => media.media_type.clone(),
        "audio" => "file".to_string(),
        _ if media.content_type.starts_with("image/") => "image".to_string(),
        _ if media.content_type.starts_with("video/") => "video".to_string(),
        _ => "file".to_string(),
    }
}

fn weixin_upload_media_type(media_type: &str) -> i64 {
    match media_type {
        "image" => 1,
        "video" => 2,
        _ => 3,
    }
}

fn cdn_upload_url(base_url: &str, upload_param: &str, filekey: &str) -> String {
    format!(
        "{}/upload?encrypted_query_param={}&filekey={}",
        base_url.trim_end_matches('/'),
        url_encode(upload_param),
        url_encode(filekey)
    )
}

fn cdn_download_url(base_url: &str, encrypted_param: &str) -> String {
    if encrypted_param.is_empty() {
        String::new()
    } else {
        format!(
            "{}/download?encrypted_query_param={}",
            base_url.trim_end_matches('/'),
            url_encode(encrypted_param)
        )
    }
}

fn safe_filename(value: &str) -> String {
    let name = Path::new(value)
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("")
        .trim();
    if name.is_empty() || name == "." || name == ".." {
        "file.bin".to_string()
    } else {
        name.to_string()
    }
}

fn decode_inbound_aes_key(value: &str) -> Result<Option<[u8; 16]>, String> {
    if value.is_empty() {
        return Ok(None);
    }
    let decoded = decode_base64(value)?;
    if decoded.len() == 16 {
        let mut key = [0u8; 16];
        key.copy_from_slice(&decoded);
        return Ok(Some(key));
    }
    if decoded.len() == 32 {
        let text = std::str::from_utf8(&decoded)
            .map_err(|_| "Invalid Weixin inbound AES key".to_string())?;
        let bytes = hex_decode(text).ok_or_else(|| "Invalid Weixin inbound AES key".to_string())?;
        if bytes.len() == 16 {
            let mut key = [0u8; 16];
            key.copy_from_slice(&bytes);
            return Ok(Some(key));
        }
    }
    Err(format!(
        "Unsupported Weixin inbound AES key length: {}",
        decoded.len()
    ))
}

fn random_bytes_16() -> Result<[u8; 16], String> {
    let mut bytes = [0u8; 16];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| format!("failed to read secure random bytes: {}", error))?;
    Ok(bytes)
}

fn random_hex(byte_count: usize) -> Result<String, String> {
    let mut bytes = vec![0u8; byte_count];
    File::open("/dev/urandom")
        .and_then(|mut file| file.read_exact(&mut bytes))
        .map_err(|error| format!("failed to read secure random bytes: {}", error))?;
    Ok(hex_encode(&bytes))
}

fn hex_encode(data: &[u8]) -> String {
    let mut output = String::with_capacity(data.len() * 2);
    for byte in data {
        output.push_str(&format!("{:02x}", byte));
    }
    output
}

fn hex_decode(value: &str) -> Option<Vec<u8>> {
    if !value.len().is_multiple_of(2) {
        return None;
    }
    let mut output = Vec::with_capacity(value.len() / 2);
    for chunk in value.as_bytes().chunks_exact(2) {
        let text = std::str::from_utf8(chunk).ok()?;
        output.push(u8::from_str_radix(text, 16).ok()?);
    }
    Some(output)
}

fn encode_base64(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut output = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        output.push(TABLE[(b0 >> 2) as usize] as char);
        output.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            output.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            output.push('=');
        }
        if chunk.len() > 2 {
            output.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            output.push('=');
        }
    }
    output
}

fn decode_base64(value: &str) -> Result<Vec<u8>, String> {
    let mut buffer = Vec::new();
    let mut quartet = [0u8; 4];
    let mut count = 0usize;
    for byte in value.bytes().filter(|byte| !byte.is_ascii_whitespace()) {
        quartet[count] = match byte {
            b'A'..=b'Z' => byte - b'A',
            b'a'..=b'z' => byte - b'a' + 26,
            b'0'..=b'9' => byte - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            b'=' => 64,
            _ => return Err("Invalid base64 data".to_string()),
        };
        count += 1;
        if count == 4 {
            buffer.push((quartet[0] << 2) | (quartet[1] >> 4));
            if quartet[2] != 64 {
                buffer.push((quartet[1] << 4) | (quartet[2] >> 2));
            }
            if quartet[3] != 64 {
                buffer.push((quartet[2] << 6) | quartet[3]);
            }
            count = 0;
        }
    }
    if count != 0 {
        return Err("Invalid base64 padding".to_string());
    }
    Ok(buffer)
}
