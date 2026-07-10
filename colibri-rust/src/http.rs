use std::io::Read;
use std::sync::OnceLock;
use std::time::Duration;

#[derive(Clone, Debug)]
pub struct HttpResponse {
    pub status: Option<u16>,
    pub body: String,
    pub headers: Vec<(String, String)>,
}

#[derive(Clone, Debug)]
pub struct BinaryHttpResponse {
    pub status: u16,
    pub body: Vec<u8>,
    pub headers: Vec<(String, String)>,
}

fn shared_agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(ureq::Agent::new)
}

pub fn request_json(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: Option<&str>,
    timeout_seconds: u64,
) -> Result<HttpResponse, String> {
    let response = request_bytes(
        method,
        url,
        headers,
        body.map(str::as_bytes),
        body.map(|_| "application/json"),
        timeout_seconds,
    )?;
    Ok(HttpResponse {
        status: Some(response.status),
        body: String::from_utf8_lossy(&response.body).to_string(),
        headers: response.headers,
    })
}

pub fn request_binary(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: Option<&[u8]>,
    content_type: Option<&str>,
    timeout_seconds: u64,
) -> Result<BinaryHttpResponse, String> {
    request_bytes(method, url, headers, body, content_type, timeout_seconds)
}

fn request_bytes(
    method: &str,
    url: &str,
    headers: &[(&str, String)],
    body: Option<&[u8]>,
    content_type: Option<&str>,
    timeout_seconds: u64,
) -> Result<BinaryHttpResponse, String> {
    let mut request = shared_agent()
        .request(method, url)
        .timeout(Duration::from_secs(timeout_seconds.max(1)));
    for (key, value) in headers {
        request = request.set(key, value);
    }
    if let Some(content_type) = content_type {
        request = request.set("Content-Type", content_type);
    }
    let result = match body {
        Some(bytes) => request.send_bytes(bytes),
        None => request.call(),
    };
    match result {
        Ok(response) => response_to_binary(response),
        Err(ureq::Error::Status(_, response)) => response_to_binary(response),
        Err(error) => Err(format!("HTTP request failed: {}", error)),
    }
}

fn response_to_binary(response: ureq::Response) -> Result<BinaryHttpResponse, String> {
    let status = response.status();
    let headers = response
        .headers_names()
        .into_iter()
        .filter_map(|name| {
            response
                .header(&name)
                .map(|value| (name, value.to_string()))
        })
        .collect::<Vec<_>>();
    let mut body = Vec::new();
    response
        .into_reader()
        .read_to_end(&mut body)
        .map_err(|error| format!("failed to read HTTP response body: {}", error))?;
    Ok(BinaryHttpResponse {
        status,
        body,
        headers,
    })
}

pub fn json_string_field(text: &str, key: &str) -> Option<String> {
    let needle = format!("\"{}\"", key);
    let start = text.find(&needle)?;
    let after = &text[start + needle.len()..];
    let colon = after.find(':')?;
    let value = after[colon + 1..].trim_start();
    if value.starts_with("null") {
        return Some(String::new());
    }
    parse_json_string_at(value)
}

pub fn parse_json_string_at(value: &str) -> Option<String> {
    let mut chars = value.strip_prefix('"')?.chars();
    let mut out = String::new();
    while let Some(ch) = chars.next() {
        match ch {
            '"' => return Some(out),
            '\\' => {
                if let Some(next) = chars.next() {
                    out.push(match next {
                        'n' => '\n',
                        'r' => '\r',
                        't' => '\t',
                        '"' => '"',
                        '\\' => '\\',
                        other => other,
                    });
                }
            }
            _other => out.push(ch),
        }
    }
    Some(out)
}
