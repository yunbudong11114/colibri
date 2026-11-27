//! Stable execution stages shared by adapters and saved stop conclusions.
pub(crate) fn for_tool(name: &str) -> &'static str {
    match name {
        "shell.run" => "running_command",
        "shell.poll" => "waiting_command",
        "shell.cancel" => "stopping_command",
        n if n.starts_with("files.") => "working_files",
        n if n.starts_with("memory.") => "working_memory",
        n if n.starts_with("skill.") => "using_skill",
        n if n.starts_with("image.") => "understanding_image",
        n if n.starts_with("task.") => "planning_task",
        n if n.starts_with("mcp.") => "calling_mcp",
        n if ["hardware.", "serial.", "gpio.", "i2c.", "spi."]
            .iter()
            .any(|prefix| n.starts_with(prefix)) =>
        {
            "accessing_hardware"
        }
        _ => "calling_tool",
    }
}

pub(crate) fn label(stage: &str) -> &str {
    match stage {
        "thinking" => "思考中",
        "compacting" => "压缩上下文",
        "running_command" => "执行命令",
        "waiting_command" => "等待命令",
        "stopping_command" => "停止命令",
        "working_files" => "处理文件",
        "working_memory" => "处理记忆",
        "using_skill" => "使用技能",
        "understanding_image" => "理解图片",
        "planning_task" => "整理任务",
        "accessing_hardware" => "访问硬件",
        "calling_mcp" => "调用 MCP",
        "calling_tool" => "调用工具",
        _ => stage,
    }
}
