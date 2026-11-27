//! Small durable task ledger. Evidence references real completed tool calls.
use crate::{
    messages::{ToolCall, ToolResult},
    tools::ToolContext,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Read,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Step {
    pub text: String,
    pub status: String,
    pub evidence: Vec<String>,
}
#[derive(Serialize, Deserialize, Clone)]
struct Evidence {
    id: String,
    tool: String,
    ok: bool,
}
#[derive(Serialize, Deserialize, Clone)]
pub(crate) struct Task {
    pub id: String,
    pub objective: String,
    pub acceptance: Vec<String>,
    pub steps: Vec<Step>,
    pub status: String,
    pub next_action: String,
    pub blocker: String,
    pub cwd: PathBuf,
    evidence: Vec<Evidence>,
    sequence: u64,
}
fn path(c: &ToolContext) -> PathBuf {
    let key = format!("{:x}", Sha256::digest(c.scope.as_bytes()));
    c.runtime_home
        .join("tasks")
        .join(format!("{}.json", &key[..24]))
}
pub(crate) fn read(c: &ToolContext) -> Result<Option<Task>, String> {
    let p = path(c);
    let file = match fs::File::open(p) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.to_string()),
    };
    let mut bytes = Vec::new();
    file.take(65537)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() > 65536 {
        return Err("Task state exceeds 64 KiB".into());
    }
    serde_json::from_slice(&bytes)
        .map(Some)
        .map_err(|e| e.to_string())
}
fn save(c: &ToolContext, t: &Task) -> Result<(), String> {
    let b = serde_json::to_vec(t).map_err(|e| e.to_string())?;
    if b.len() > 65536 {
        return Err("Task state exceeds 64 KiB".into());
    }
    crate::config_io::atomic_write(&path(c), &b).map_err(|e| e.to_string())
}
fn edit(
    c: &ToolContext,
    f: impl FnOnce(Option<Task>) -> Result<Task, String>,
) -> Result<Task, String> {
    let p = path(c);
    fs::create_dir_all(p.parent().unwrap()).map_err(|e| e.to_string())?;
    let _lock = crate::config_io::lock(&p.with_extension("lock")).map_err(|e| e.to_string())?;
    let t = f(read(c)?)?;
    save(c, &t)?;
    Ok(t)
}
fn text(v: &Value, key: &str, max: usize) -> Result<String, String> {
    let s = v[key].as_str().ok_or_else(|| format!("Missing {key}"))?;
    if s.trim().is_empty() || s.len() > max {
        return Err(format!("{key} is empty or too long"));
    }
    Ok(s.to_string())
}
pub(crate) fn snapshot(c: &ToolContext) -> Result<Value, String> {
    read(c).and_then(|t| serde_json::to_value(t).map_err(|e| e.to_string()))
}
pub(crate) fn active_id(c: &ToolContext) -> Option<String> {
    read(c)
        .ok()
        .flatten()
        .filter(|t| t.status == "running")
        .map(|t| t.id)
}
pub(crate) fn incomplete(c: &ToolContext) -> Option<String> {
    read(c).ok().flatten().filter(|t|t.status=="running").map(|t|format!("Task is still incomplete: {}. Next: {}. Continue work, or record a concrete blocker with task.update. Do not report completion without checks.",t.objective,t.next_action))
}
pub(crate) fn summary(c: &ToolContext) -> String {
    match read(c){Ok(Some(t))=>{let done=t.steps.iter().filter(|s|s.status=="completed").count();format!("Current task {}: {} ({done}/{} steps, {}). Next: {}. Blocker: {}. Use task.inspect for recorded steps/evidence. A new user goal may explicitly replace this task.",t.id,t.objective,t.steps.len(),t.status,t.next_action,t.blocker)},Ok(None)=>String::new(),Err(e)=>format!("Task state could not be recovered: {e}. Inspect and report the problem; do not assume completion.")}
}

pub(crate) fn cancel(c: &ToolContext) -> Result<String, String> {
    let Some(old) = read(c)? else {
        return Ok("尚未创建任务计划。".into());
    };
    if old.status == "completed" || old.status == "cancelled" {
        return Ok(format!("最近任务：{}（{}）", old.objective, old.status));
    }
    let t = edit(c, |old| {
        let mut t = old.ok_or("No task")?;
        t.status = "cancelled".into();
        for step in &mut t.steps {
            if step.status == "running" {
                step.status = "cancelled".into();
            }
        }
        Ok(t)
    })?;
    let mut lines = vec![format!("任务：{}", t.objective)];
    for step in t.steps {
        lines.push(format!(
            "{}：{}",
            if step.status == "completed" {
                "已完成"
            } else {
                "未完成"
            },
            step.text
        ));
    }
    if !t.next_action.is_empty() {
        lines.push(format!("原下一步：{}", t.next_action));
    }
    Ok(lines.join("\n"))
}

pub(crate) fn run(name: &str, v: &Value, c: &ToolContext) -> ToolResult {
    let mut sanitized = v.clone();
    fn scrub(v: &mut Value, config: &crate::config::AgentConfig) {
        match v {
            Value::String(s) => crate::secrets::redact_tool_output(config, s),
            Value::Array(values) => values.iter_mut().for_each(|v| scrub(v, config)),
            Value::Object(values) => values.values_mut().for_each(|v| scrub(v, config)),
            _ => {}
        }
    }
    scrub(&mut sanitized, &c.config);
    let v = &sanitized;
    let result = (|| -> Result<Value, String> {
        match name {
            "task.inspect" => snapshot(c),
            "task.start" => {
                let objective = text(v, "objective", 2048)?;
                let values = v["steps"].as_array().ok_or("steps must be an array")?;
                if values.is_empty() || values.len() > 16 {
                    return Err("Need 1..16 steps".into());
                }
                let mut steps = Vec::new();
                for value in values {
                    let s = value
                        .as_str()
                        .filter(|s| !s.is_empty() && s.len() <= 1024)
                        .ok_or("Invalid step")?;
                    steps.push(Step {
                        text: s.into(),
                        status: "pending".into(),
                        evidence: vec![],
                    });
                }
                let acceptance = v["acceptance"]
                    .as_array()
                    .filter(|a| !a.is_empty() && a.len() <= 16)
                    .ok_or("Provide 1..16 acceptance checks")?
                    .iter()
                    .map(|v| {
                        v.as_str()
                            .filter(|s| !s.is_empty() && s.len() <= 1024)
                            .map(str::to_string)
                            .ok_or("Invalid acceptance check")
                    })
                    .collect::<Result<Vec<_>, _>>()?;
                let t = edit(c, |old| {
                    if old.as_ref().is_some_and(|t| t.status == "running")
                        && v["replace"].as_bool() != Some(true)
                    {
                        return Err("A task is already active. Continue it or use replace:true only for a new user objective".into());
                    }
                    Ok(Task {
                        id: format!(
                            "task-{}",
                            SystemTime::now()
                                .duration_since(UNIX_EPOCH)
                                .unwrap_or_default()
                                .as_nanos()
                        ),
                        objective,
                        acceptance,
                        steps,
                        status: "running".into(),
                        next_action: String::new(),
                        blocker: String::new(),
                        cwd: c.cwd.clone(),
                        evidence: vec![],
                        sequence: 0,
                    })
                })?;
                serde_json::to_value(t).map_err(|e| e.to_string())
            }
            "task.update" => {
                let t = edit(c, |old| {
                    let mut t = old.ok_or("No active task")?;
                    if let Some(next) = v["next_action"].as_str() {
                        if next.len() > 2048 {
                            return Err("next_action too long".into());
                        }
                        t.next_action = next.into()
                    }
                    if let Some(index) = v["step"].as_u64() {
                        let status = v["status"].as_str().unwrap_or("running");
                        if !["pending", "running", "completed"].contains(&status) {
                            return Err("Invalid step status".into());
                        }
                        let ids = v["evidence_ids"]
                            .as_array()
                            .map(|a| {
                                a.iter()
                                    .filter_map(Value::as_str)
                                    .map(str::to_string)
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        if ids.len() > 16 {
                            return Err("Too many evidence ids".into());
                        }
                        if status == "completed"
                            && (ids.is_empty()
                                || ids
                                    .iter()
                                    .any(|id| !t.evidence.iter().any(|e| &e.id == id && e.ok)))
                        {
                            return Err("Completed steps require existing successful evidence_ids from this task; run the relevant verification first".into());
                        }
                        let s = t
                            .steps
                            .get_mut(index.checked_sub(1).ok_or("Step is 1-based")? as usize)
                            .ok_or("Invalid step")?;
                        s.status = status.into();
                        s.evidence = ids;
                    }
                    if let Some(blocker) = v["blocker"].as_str() {
                        if blocker.len() > 2048 {
                            return Err("blocker too long".into());
                        }
                        t.blocker = blocker.into();
                    }
                    t.status = if t.steps.iter().all(|s| s.status == "completed") {
                        "completed"
                    } else if !t.blocker.is_empty() {
                        "blocked"
                    } else {
                        "running"
                    }
                    .into();
                    Ok(t)
                })?;
                serde_json::to_value(t).map_err(|e| e.to_string())
            }
            _ => Err("Unknown task tool".into()),
        }
    })();
    match result {
        Ok(v) => ToolResult::ok(v.to_string()),
        Err(e) => ToolResult::error("task_error", e),
    }
}

pub(crate) fn record(
    c: &ToolContext,
    task_id: &str,
    call: &ToolCall,
    result: &ToolResult,
) -> Result<Option<String>, String> {
    if call.name.starts_with("task.") {
        return Ok(None);
    }
    let mut id = None;
    edit(c, |old| {
        let mut t = old.ok_or("No task")?;
        if t.id != task_id {
            return Ok(t);
        }
        let job = serde_json::from_str::<Value>(&result.text).ok();
        let unsettled = job.as_ref().is_some_and(|v| {
            v.get("job_id").is_some() && (v["status"] != "completed" || v["exit_code"] != 0)
        });
        t.sequence += 1;
        let eid = format!("ev-{}", t.sequence);
        id = Some(eid.clone());
        t.evidence.push(Evidence {
            id: eid,
            tool: call.name.clone(),
            ok: result.ok && !unsettled,
        });
        if t.evidence.len() > 128 {
            let used = t
                .steps
                .iter()
                .flat_map(|s| s.evidence.iter())
                .cloned()
                .collect::<std::collections::BTreeSet<_>>();
            if let Some(i) = t.evidence.iter().position(|e| !used.contains(&e.id)) {
                t.evidence.remove(i);
            } else {
                return Err("Evidence storage full; complete or replace the task".into());
            }
        }
        Ok(t)
    })?;
    Ok(id)
}
