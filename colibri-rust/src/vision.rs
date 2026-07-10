use std::fs;
use std::path::Path;

use crate::config::{AgentConfig, ModelConfig};
use crate::messages::ModelLimits;
use crate::model::{build_model, ModelClient};
use crate::tools::content_type_for_path;

pub fn analyze_image(config: &AgentConfig, path: &Path, prompt: &str) -> Result<String, String> {
    let content_type = content_type_for_path(path);
    if !content_type.starts_with("image/") {
        return Err("invalid_media:image.understand requires an image file".to_string());
    }
    let metadata = fs::metadata(path).map_err(|error| format!("read_error:{}", error))?;
    let max_bytes = config.vision.max_image_bytes.max(1);
    if metadata.len() as usize > max_bytes {
        return Err(format!(
            "image_too_large:Image is too large: {} bytes exceeds {} bytes",
            metadata.len(),
            max_bytes
        ));
    }
    let bytes = fs::read(path).map_err(|error| format!("read_error:{}", error))?;
    if bytes.len() > max_bytes {
        return Err(format!(
            "image_too_large:Image is too large: {} bytes exceeds {} bytes",
            bytes.len(),
            max_bytes
        ));
    }
    let effective_prompt = if prompt.is_empty() {
        "Describe this image and extract its important information."
    } else {
        prompt
    };
    if config.model.provider == "fake" && config.vision.model.is_empty() {
        return Ok(format!("fake image: {}", effective_prompt));
    }
    let mut model = vision_model(config)?;
    let data_url = format!("data:{};base64,{}", content_type, encode_base64(&bytes));
    let response = model.complete_image(
        effective_prompt,
        &data_url,
        &ModelLimits {
            timeout_seconds: config.vision.timeout_seconds,
            max_output_tokens: config.model.max_output_tokens,
        },
    )?;
    if !response.tool_calls.is_empty() {
        return Err(
            "model_error:Vision model returned tool calls instead of an image description"
                .to_string(),
        );
    }
    Ok(response.text)
}

fn vision_model(config: &AgentConfig) -> Result<Box<dyn ModelClient>, String> {
    let vision = &config.vision;
    let model_config = ModelConfig {
        provider: config.model.provider.clone(),
        base_url: if vision.base_url.is_empty() {
            config.model.base_url.clone()
        } else {
            vision.base_url.clone()
        },
        model: if vision.model.is_empty() {
            config.model.model.clone()
        } else {
            vision.model.clone()
        },
        api_key: if vision.api_key.is_empty() {
            config.model.api_key.clone()
        } else {
            vision.api_key.clone()
        },
        timeout_seconds: vision.timeout_seconds,
        max_output_tokens: config.model.max_output_tokens,
    };
    build_model(&model_config)
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
