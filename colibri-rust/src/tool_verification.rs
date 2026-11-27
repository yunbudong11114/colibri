//! Explicit edit-and-check composition; failed checks never imply the edit was rolled back.
use crate::{messages::ToolResult, tools::ToolContext};
use std::collections::BTreeMap;

pub(crate) fn validate(
    args: &BTreeMap<String, String>,
    context: &ToolContext,
) -> Result<(), ToolResult> {
    let Some(command) = args.get("verify_command") else {
        return if args.contains_key("verify_cwd") || args.contains_key("verify_timeout_seconds") {
            Err(ToolResult::error(
                "invalid_arguments",
                "Verification options require verify_command",
            ))
        } else {
            Ok(())
        };
    };
    if command.trim().is_empty() || shell_words::split(command).is_err() {
        return Err(ToolResult::error(
            "invalid_arguments",
            "Invalid verify_command",
        ));
    }
    if let Some(seconds) = args.get("verify_timeout_seconds") {
        if !seconds
            .parse::<f64>()
            .is_ok_and(|n| n.is_finite() && n > 0.0 && n <= 86400.0)
        {
            return Err(ToolResult::error(
                "invalid_arguments",
                "Invalid verify_timeout_seconds",
            ));
        }
    }
    if let Some(raw) = args.get("verify_cwd") {
        let p = crate::config::expand_user_path(raw);
        let p = if p.is_absolute() {
            p
        } else {
            context.cwd.join(p)
        };
        if !p.is_dir() {
            return Err(ToolResult::error(
                "invalid_arguments",
                "verify_cwd must be an existing directory",
            ));
        }
    }
    Ok(())
}

pub(crate) fn after_edit(
    edit: ToolResult,
    args: &BTreeMap<String, String>,
    context: &ToolContext,
) -> ToolResult {
    let Some(command) = args.get("verify_command") else {
        return edit;
    };
    if !edit.ok {
        return edit;
    }
    let mut check = BTreeMap::from([("command".into(), command.clone())]);
    if let Some(v) = args.get("verify_cwd") {
        check.insert("cwd".into(), v.clone());
    }
    if let Some(v) = args.get("verify_timeout_seconds") {
        check.insert("timeout_seconds".into(), v.clone());
    }
    let verified = crate::tools::shell_run(&check, context);
    let state = if verified.ok {
        "passed"
    } else {
        match verified.error_type.as_deref() {
            Some("cancelled") => "cancelled",
            Some("timeout") => "timeout",
            _ => "failed",
        }
    };
    let text = serde_json::json!({"edit":"applied","edit_result":edit.text,"verification":{
        "status":state,"command":command,"ok":verified.ok,"error_type":verified.error_type,"output":verified.text,"truncated":verified.truncated
    },"note":"The edit remains applied even if verification fails or is cancelled. Do not reapply the edit."}).to_string();
    let mut result = if verified.ok {
        ToolResult::ok(text)
    } else {
        ToolResult::error(
            verified
                .error_type
                .as_deref()
                .unwrap_or("verification_failed"),
            text,
        )
    };
    result.truncated = verified.truncated;
    result
}
