use crate::config::AgentConfig;
use serde_json::Value;

const MASK: &str = "<redacted>";

fn collect(value: &Value, secrets: &mut Vec<String>, sensitive: bool) {
    match value {
        Value::Object(entries) => {
            for (key, value) in entries {
                let field = key.rsplit('.').next().unwrap_or(key).to_ascii_lowercase();
                let hidden = matches!(
                    field.as_str(),
                    "authorization"
                        | "token"
                        | "api_key"
                        | "password"
                        | "secret"
                        | "access_token"
                        | "refresh_token"
                ) || field.ends_with("_key")
                    || field.ends_with("_token")
                    || field.ends_with("_secret");
                collect(value, secrets, sensitive || hidden);
            }
        }
        Value::Array(values) => {
            for item in values {
                collect(item, secrets, sensitive);
            }
        }
        Value::String(value) if sensitive && !value.trim().is_empty() => {
            secrets.push(value.clone())
        }
        _ => {}
    }
}

fn replace(text: &mut String, mut secrets: Vec<String>) {
    let escaped: Vec<_> = secrets
        .iter()
        .filter_map(|secret| {
            let encoded = serde_json::to_string(secret).ok()?;
            let value = &encoded[1..encoded.len() - 1];
            (value != secret).then(|| value.to_owned())
        })
        .collect();
    secrets.extend(escaped);
    secrets.sort_unstable_by(|a, b| b.len().cmp(&a.len()).then_with(|| a.cmp(b)));
    secrets.dedup();
    for secret in secrets {
        if secret != MASK && !secret.trim().is_empty() && text.contains(&secret) {
            *text = text.replace(&secret, MASK);
        }
    }
}

pub(crate) fn redact_tool_output(config: &AgentConfig, text: &mut String) {
    let mut secrets: Vec<String> = [
        &config.model.api_key,
        &config.vision.api_key,
        &config.web_search.api_key,
        &config.speech.api_key,
        &config.channels_weixin.token,
    ]
    .into_iter()
    .cloned()
    .collect();
    if let Ok(key) = std::env::var("COLIBRI_API_KEY") {
        secrets.push(key);
    }
    collect(&config.mcp_servers, &mut secrets, false);
    replace(text, secrets);
}

pub(crate) fn redact_config_content(text: &mut String) {
    let value: Option<Value> = serde_json::from_str(text).ok().or_else(|| {
        text.parse::<toml::Value>()
            .ok()
            .and_then(|table| serde_json::to_value(table).ok())
    });
    let Some(value) = value else {
        return;
    };
    let mut secrets = Vec::new();
    collect(&value, &mut secrets, false);
    replace(text, secrets);
}
