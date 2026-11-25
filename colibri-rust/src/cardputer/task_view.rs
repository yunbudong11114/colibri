//! Small-screen labels and progress formatting live only in the device adapter.

pub(super) fn render(c: &crate::tools::ToolContext) -> String {
    let mut out = match crate::tasks::read(c) {
        Ok(Some(t)) => {
            let done = t
                .steps
                .iter()
                .filter(|step| step.status == "completed")
                .count();
            let mut s = format!(
                "{}\n{} · {done}/{}",
                t.objective,
                status(&t.status),
                t.steps.len()
            );
            for (i, step) in t.steps.iter().enumerate() {
                s += &format!(
                    "\n{}. [{}] {}",
                    i + 1,
                    if step.status == "completed" {
                        "完成"
                    } else if step.status == "running" {
                        "进行"
                    } else {
                        "待办"
                    },
                    step.text
                )
            }
            if !t.next_action.is_empty() {
                s += &format!("\n下一步：{}\n", t.next_action);
            }
            if !t.blocker.is_empty() {
                s += &format!("\n阻塞：{}\n", t.blocker);
            }
            s += "\n验收：\n";
            for a in t.acceptance {
                s += &format!("- {a}\n")
            }
            s
        }
        Ok(None) => "当前没有记录的任务。\n".into(),
        Err(e) => format!("任务状态读取失败：{e}\n"),
    };
    for j in crate::jobs::list(c) {
        out += &format!(
            "\n命令 {}：{}\n",
            &j.job_id[..j.job_id.len().min(8)],
            status(&j.status)
        );
    }
    out
}

fn status(value: &str) -> &str {
    match value {
        "pending" => "未开始",
        "running" => "进行中",
        "completed" => "已完成",
        "blocked" => "等待处理",
        "failed" => "失败",
        "timed_out" => "超时",
        "killed" => "已取消",
        "interrupted" => "已中断",
        _ => value,
    }
}
