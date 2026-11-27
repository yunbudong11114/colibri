//! Hardware tool arguments and model-facing results; transport stays in hardware.rs.
use crate::hardware::{
    configured_hardware_devices, probe_hardware, resolve_hardware_device, serial_json_request,
    serial_read, serial_write, HardwareError,
};
use crate::{messages::ToolResult, tools::ToolContext};
use std::collections::BTreeMap;
pub(crate) fn hardware_probe(context: &ToolContext) -> ToolResult {
    hardware_json_result(probe_hardware(), context)
}

pub(crate) fn hardware_devices(context: &ToolContext) -> ToolResult {
    hardware_json_result(
        configured_hardware_devices(&context.config.hardware),
        context,
    )
}

pub(crate) fn hardware_serial_read(
    args: &BTreeMap<String, String>,
    context: &ToolContext,
) -> ToolResult {
    let device_name = match required_hardware_string(args, "device") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let encoding = args.get("encoding").map(String::as_str).unwrap_or("auto");
    if !matches!(encoding, "auto" | "text" | "hex") {
        return ToolResult::error("invalid_arguments", "encoding must be auto, text or hex");
    }
    let max_bytes = match args.get("max_bytes") {
        Some(value) => match hardware_integer(
            value,
            "max_bytes",
            1,
            context.config.hardware.max_transfer_bytes as u64,
        ) {
            Ok(value) => value as usize,
            Err(result) => return result,
        },
        None => context.config.hardware.max_transfer_bytes,
    };
    let device = match resolve_hardware_device(&context.config.hardware, device_name, "serial") {
        Ok(device) => device,
        Err(error) => return hardware_error_result(error, context),
    };
    match serial_read(&context.config.hardware, device, max_bytes) {
        Ok(data) => hardware_json_result(
            serial_data(&data, encoding, Some(device_name), context),
            context,
        ),
        Err(error) => hardware_error_result(error, context),
    }
}

pub(crate) fn hardware_serial_write(
    args: &BTreeMap<String, String>,
    context: &ToolContext,
) -> ToolResult {
    let device_name = match required_hardware_string(args, "device") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let data = match required_hardware_string(args, "data") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let encoding = args.get("encoding").map(String::as_str).unwrap_or("text");
    let bytes = match encoding {
        "text" => data.as_bytes().to_vec(),
        "hex" => {
            let hex = match required_hardware_hex(args, context) {
                Ok(v) => v,
                Err(r) => return r,
            };
            hex.as_bytes()
                .chunks_exact(2)
                .map(|v| u8::from_str_radix(std::str::from_utf8(v).unwrap(), 16).unwrap())
                .collect()
        }
        _ => return ToolResult::error("invalid_arguments", "encoding must be text or hex"),
    };
    let device = match resolve_hardware_device(&context.config.hardware, device_name, "serial") {
        Ok(device) => device,
        Err(error) => return hardware_error_result(error, context),
    };
    match serial_write(&context.config.hardware, device, &bytes) {
        Ok(written) => hardware_json_result(
            serde_json::json!({"device":device_name,"bytes_written":written,"status":"transport_written","execution_confirmed":false,"note":"Bytes accepted by the local serial transport; remote execution is not confirmed."}),
            context,
        ),
        Err(error) => hardware_error_result(error, context),
    }
}

pub(crate) fn hardware_controller_tool(
    args: &BTreeMap<String, String>,
    context: &ToolContext,
    capability: &str,
    command: &str,
) -> ToolResult {
    let device_name = match required_hardware_string(args, "device") {
        Ok(value) => value,
        Err(result) => return result,
    };
    let arguments = match hardware_controller_arguments(args, context, command) {
        Ok(value) => value,
        Err(result) => return result,
    };
    let device = match resolve_hardware_device(&context.config.hardware, device_name, capability) {
        Ok(device) => device,
        Err(error) => return hardware_error_result(error, context),
    };
    match serial_json_request(&context.config.hardware, device, command, arguments) {
        Ok(result) => hardware_json_result(
            serde_json::json!({"device":device_name,"result":result,"controller_acknowledged":true,"status":"controller_reported_success","physical_effect_verified":false}),
            context,
        ),
        Err(error) => hardware_error_result(error, context),
    }
}

fn hardware_controller_arguments(
    args: &BTreeMap<String, String>,
    context: &ToolContext,
    command: &str,
) -> Result<serde_json::Value, ToolResult> {
    let mut result = serde_json::Map::new();
    match command {
        "gpio_read" => insert_hardware_integer(&mut result, args, "pin", 0, 65535)?,
        "gpio_write" => {
            insert_hardware_integer(&mut result, args, "pin", 0, 65535)?;
            insert_hardware_integer(&mut result, args, "value", 0, 1)?;
        }
        "i2c_scan" => {}
        "i2c_read" => {
            insert_hardware_integer(&mut result, args, "address", 0, 127)?;
            if let Some(value) = args.get("register") {
                let value = hardware_integer(value, "register", 0, 255)?;
                result.insert("register".to_string(), serde_json::Value::from(value));
            }
            insert_hardware_integer(
                &mut result,
                args,
                "length",
                1,
                context.config.hardware.max_transfer_bytes as u64,
            )?;
        }
        "i2c_write" => {
            insert_hardware_integer(&mut result, args, "address", 0, 127)?;
            if let Some(value) = args.get("register") {
                let value = hardware_integer(value, "register", 0, 255)?;
                result.insert("register".to_string(), serde_json::Value::from(value));
            }
            let data = required_hardware_hex(args, context)?;
            result.insert("data".to_string(), serde_json::Value::String(data));
        }
        "spi_transfer" => {
            let data = required_hardware_hex(args, context)?;
            result.insert("data".to_string(), serde_json::Value::String(data));
        }
        _ => {
            return Err(ToolResult::error(
                "invalid_arguments",
                "Unsupported hardware controller command",
            ))
        }
    }
    Ok(serde_json::Value::Object(result))
}

fn required_hardware_string<'a>(
    args: &'a BTreeMap<String, String>,
    name: &str,
) -> Result<&'a str, ToolResult> {
    args.get(name)
        .filter(|value| !value.is_empty())
        .map(String::as_str)
        .ok_or_else(|| {
            ToolResult::error(
                "invalid_arguments",
                format!("{} must be a non-empty string", name),
            )
        })
}

fn hardware_integer(
    value: &str,
    name: &str,
    minimum: u64,
    maximum: u64,
) -> Result<u64, ToolResult> {
    value
        .parse::<u64>()
        .ok()
        .filter(|value| (minimum..=maximum).contains(value))
        .ok_or_else(|| {
            ToolResult::error(
                "invalid_arguments",
                format!(
                    "{} must be an integer between {} and {}",
                    name, minimum, maximum
                ),
            )
        })
}

fn insert_hardware_integer(
    result: &mut serde_json::Map<String, serde_json::Value>,
    args: &BTreeMap<String, String>,
    name: &str,
    minimum: u64,
    maximum: u64,
) -> Result<(), ToolResult> {
    let Some(value) = args.get(name) else {
        return Err(ToolResult::error(
            "invalid_arguments",
            format!(
                "{} must be an integer between {} and {}",
                name, minimum, maximum
            ),
        ));
    };
    let value = hardware_integer(value, name, minimum, maximum)?;
    result.insert(name.to_string(), serde_json::Value::from(value));
    Ok(())
}

fn required_hardware_hex(
    args: &BTreeMap<String, String>,
    context: &ToolContext,
) -> Result<String, ToolResult> {
    let value = required_hardware_string(args, "data")?;
    if value.len() % 2 != 0
        || value.len() / 2 > context.config.hardware.max_transfer_bytes
        || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
    {
        return Err(ToolResult::error(
            "invalid_arguments",
            if value.len() / 2 > context.config.hardware.max_transfer_bytes {
                "data exceeds the configured transfer limit"
            } else {
                "data must be a non-empty even-length hexadecimal string"
            },
        ));
    }
    Ok(value.to_ascii_lowercase())
}

fn serial_data(
    data: &[u8],
    requested: &str,
    device: Option<&str>,
    context: &ToolContext,
) -> serde_json::Value {
    let mut redactor = crate::secrets::StreamRedactor::new(crate::secrets::values(&context.config));
    let safe = redactor.feed(data, true);
    let utf8 = std::str::from_utf8(&safe).ok();
    let is_text = requested != "hex"
        && utf8.is_some()
        && (requested == "text"
            || !safe
                .iter()
                .any(|b| *b < 32 && !matches!(*b, b'\n' | b'\r' | b'\t')));
    let body = if is_text {
        utf8.unwrap().to_string()
    } else {
        safe.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    serde_json::json!({"device":device,"bytes_read":data.len(),"data_bytes":safe.len(),"encoding":if is_text{"text"}else{"hex"},"requested_encoding":requested,"redacted":safe!=data,"data":body,"text_decode_error":requested=="text" && utf8.is_none(),"note":"These bytes have already been consumed from the device. Inspect the returned data/archive instead of rereading the serial port to recover it."})
}

fn hardware_json_result(value: serde_json::Value, context: &ToolContext) -> ToolResult {
    let (mut text, truncated) = crate::tool_archive::project(&value, "hardware_result", context);
    if truncated {
        let mut projected: serde_json::Value = serde_json::from_str(&text).unwrap();
        for key in [
            "device",
            "bytes_read",
            "bytes_written",
            "status",
            "execution_confirmed",
            "controller_acknowledged",
            "outcome_unknown",
            "physical_effect_verified",
            "redacted",
            "encoding",
            "is_error",
        ] {
            if let Some(v) = value.get(key) {
                projected[key] = v.clone();
            }
        }
        text = projected.to_string();
        crate::secrets::redact_tool_output(&context.config, &mut text);
    }
    let mut result = ToolResult::ok(text);
    result.truncated = truncated;
    result
}

fn hardware_error_result(error: HardwareError, context: &ToolContext) -> ToolResult {
    let value = serde_json::json!({"is_error":true,"error":error.message,"error_type":error.error_type,"bytes_written":error.bytes_written,
        "outcome_unknown":error.outcome_unknown,"partial_read":if error.partial_read.is_empty(){None}else{Some(serial_data(&error.partial_read,"auto",None,context))},
        "note":if error.outcome_unknown{"The operation may have been applied. Verify its effects before retrying; do not automatically resend."}else{"No rollback is implied. A transport write is not confirmation of a physical effect."}});
    let mut result = hardware_json_result(value, context);
    result.ok = false;
    result.error_type = Some(error.error_type);
    result
}

pub(crate) fn is_hardware_tool(name: &str) -> bool {
    matches!(
        name,
        "hardware.probe"
            | "hardware.devices"
            | "serial.read"
            | "serial.write"
            | "gpio.read"
            | "gpio.write"
            | "i2c.scan"
            | "i2c.read"
            | "i2c.write"
            | "spi.transfer"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn raw_data_encoding_redacts_before_hex_and_keeps_error_state() {
        let scratch = crate::config_io::Scratch::new(&std::env::temp_dir()).unwrap();
        let mut config = crate::config::AgentConfig::default();
        config.model.api_key = "known-secret".into();
        let context =
            ToolContext::new(config, scratch.0.clone()).with_runtime_home(scratch.0.clone());
        let v = serial_data(b"known-secret\xff", "hex", Some("test"), &context);
        assert_eq!(v["redacted"], true);
        assert_eq!(v["bytes_read"], 13);
        assert_eq!(v["data"], "3c72656461637465643eff");
        let v = serial_data(b"\xff", "text", None, &context);
        assert_eq!(v["encoding"], "hex");
        assert_eq!(v["text_decode_error"], true);
        let e = HardwareError {
            error_type: "timeout".into(),
            message: "response unavailable".into(),
            partial_read: b"partial".to_vec(),
            bytes_written: Some(12),
            outcome_unknown: true,
        };
        let r = hardware_error_result(e, &context);
        assert!(!r.ok);
        let v: serde_json::Value = serde_json::from_str(&r.text).unwrap();
        assert_eq!(v["outcome_unknown"], true);
        assert_eq!(v["partial_read"]["data"], "partial");
    }
}
