# Colibri

Colibri 是一个面向 CardputerZero 这类小内存 Linux 设备的轻量级 Python Agent 运行时。

它可以在无图形界面的服务器上运行，支持本地 CLI、SSH 会话，也支持通过 gateway 接入微信等外部聊天渠道。

[English README](README.md)

## 特性

- 纯命令行/SSH 友好，不依赖 GUI、浏览器、TUI、音频设备或桌面环境。
- 运行时只使用 Python 标准库，开发测试使用 `pytest`。
- 支持 OpenAI-compatible 模型接口，也内置确定性的 fake model 用于测试。
- 有边界的 agent tool loop。
- 内置文件、Shell、网页搜索、记忆、技能工具，以及默认关闭的只读硬件探测。
- 动态权限确认，使用数字选项，支持单次、session、可执行文件 session、用户级命令和用户级可执行文件授权。
- Markdown 文件记忆系统，支持自动 recall。
- 本地 skill，使用渐进式披露加载。
- 上下文压缩，支持模型摘要和本地 fallback。
- CLI、REPL、gateway 和掌机 App 共用 JSONL transcript。
- 微信 channel，基于腾讯 iLink。
- gateway 支持 `run/start/stop/restart/status`。

## 架构图

```mermaid
flowchart TD
    User["用户 / 外部渠道"] --> CLI["colibri.cli"]
    CLI --> Commands["ask / repl / diagnostics / hardware / auth / gateway"]
    Commands --> Session["AgentSession"]
    Commands --> GatewayProc["GatewayProcessManager"]
    GatewayProc --> GatewayRun["gateway run"]
    GatewayRun --> Gateway["GatewayRunner"]
    Gateway --> Channels["Channel workers"]
    Channels --> Weixin["微信 iLink"]
    Gateway --> Session
    Session --> Model["ModelClient"]
    Model --> Fake["Fake model"]
    Model --> OpenAI["OpenAI-compatible API"]
    Session --> Registry["ToolRegistry"]
    Registry --> Tools["files / shell / memory / skill"]
    Session --> Permissions["PermissionPolicy"]
    Permissions --> UserPerms["~/.colibri/permissions.toml"]
    Session --> Memory["MemoryContext"]
    Memory --> MemoryFiles["~/.colibri/memory"]
    Session --> Skills["SkillIndex"]
    Skills --> SkillFiles["~/.colibri/skills"]
    Session --> Transcript["TranscriptWriter"]
    Transcript --> TranscriptFiles["~/.colibri/transcripts/*.jsonl"]
```

## 快速开始

```bash
uv run python -m pytest
uv run python -m colibri.cli ask "hello"
uv run python -m colibri.cli repl
uv run python -m colibri.cli diagnostics
```

如果不传 `--config`，Colibri 会读取：

```text
~/.colibri/config.toml
```

如果这个文件不存在，就使用内置默认配置。显式传入的 `--config` 优先级最高。

## Rust 运行时

`colibri-rust/` 的同一份核心供 CLI、REPL、gateway 和掌机 worker 使用，采用同步 Rust HTTP 客户端和小型 crate，不引入大型异步运行时。

```bash
cargo test --manifest-path colibri-rust/Cargo.toml
cargo build --release --manifest-path colibri-rust/Cargo.toml
./colibri-rust/target/release/colibri configure
./colibri-rust/target/release/colibri repl
./colibri-rust/target/release/colibri diagnostics
```

用户配置统一为 `~/.colibri/config.toml`（或 `$COLIBRI_HOME/config.toml`），可用 `--config PATH` 指定。未配置模型时明确提示，不再自动进入 fake 模型。运行 `colibri configure` 或 `colibri --config PATH configure` 逐步配置，密钥不回显，确认后保存。显式设置 `model.provider="fake"` 仍可用于测试。

核心包含 OpenAI 兼容模型、记忆、历史恢复与压缩、持久任务证据、后台命令、文件范围读取/搜索/局部修改、技能、标准 MCP 发现与调用、搜索、图片理解、文件媒体发送和微信认证/网关。应用层保持完全访问，不提供工具赋权或权限审批层；危险操作由系统提示要求 Agent 先向用户确认。操作系统与外部服务自身的权限和凭据仍然有效。

Rust 回归覆盖共享逻辑和各入口流程；Python 参考测试用于覆盖映射与部分确定性对比。模型引导、完全访问等最新需求按 Rust 运行时单独验证。

示例配置在：

```text
configs/agent.example.toml
configs/openai.example.toml
configs/glm.example.toml
```

私密 API key 应保存在用户自己的配置文件或环境变量中，不要提交到仓库。

## 模型配置

Python 参考实现的默认模型是本地 fake model，适合测试；Rust 正式入口需要先运行 `colibri configure`，模拟模型需显式选择：

```bash
uv run python -m colibri.cli ask "hello"
```

OpenAI-compatible 接口示例：

```toml
[model]
provider = "openai_compatible"
base_url = "https://your-openai-compatible-api.example/v1"
model = "your-model"
api_key = ""
```

`model.api_key` 优先。如果为空，Colibri 会读取 `COLIBRI_API_KEY`。

## 网页搜索

网络搜索可通过单独安装的本地技能使用。

## CLI 命令

```bash
uv run python -m colibri.cli ask "hello"
uv run python -m colibri.cli repl
uv run python -m colibri.cli diagnostics
uv run python -m colibri.cli hardware probe
uv run python -m colibri.cli hardware simulate
uv run python -m colibri.cli auth weixin
```

- `ask`：执行一次请求后退出。
- `repl`：本地多轮对话。长任务执行中，TTY 下可直接输入一行新指令**改方向**（跳过剩余工具并注入文本），无需等 `colibri>` 提示符；
- `diagnostics`：查看环境、路径、RSS、上下文限制等诊断信息。
- `hardware probe`：列出主机标准硬件设备节点，不打开设备，也不启动后台监听。
- `hardware simulate`：以前台 stdin/stdout NDJSON 方式运行 GPIO/I2C/SPI 协议模拟器，供无真机开发和测试使用。
- `auth weixin`：启动微信 iLink 二维码登录，并把 token 写入当前配置文件。

要向模型暴露硬件工具，需要同时打开两个开关，并显式配置设备白名单：

```toml
[tools]
enabled = ["shell", "files", "image", "memory", "skills", "hardware"]

[hardware]
enabled = true
discovery = "on_demand"
operation_timeout_seconds = 2.0
max_transfer_bytes = 4096

[[hardware.devices]]
name = "controller"
path = "/dev/ttyACM0"
transport = "serial_json"
baud_rate = 115200
capabilities = ["serial", "gpio", "i2c", "spi"]
allow_write = false
```

模型只能使用 `name` 别名，不能提交任意设备路径。当前工具包括 `hardware.probe`、`hardware.devices`、`serial.read/write`、`gpio.read/write`、`i2c.scan/read/write` 和 `spi.transfer`。GPIO/I2C/SPI 通过有界的 newline-delimited JSON 串口控制器协议执行；原生 Linux 总线驱动留到真机接口确认后接入。

带副作用的操作必须同时满足 `allow_write = true` 和权限确认。硬件权限按设备提供 `once`、`session-device`、`user-device`、`deny`，持久授权写入 `permissions.toml` 的 `[hardware].devices`。任何授权都不能绕过设备白名单、能力、超时和传输大小限制。所有设备均按操作打开并立即关闭，不常驻扫描或持有设备句柄。

## Gateway

Gateway 用于接入外部聊天渠道：

```bash
uv run python -m colibri.cli gateway run
uv run python -m colibri.cli gateway start
uv run python -m colibri.cli gateway stop
uv run python -m colibri.cli gateway restart
uv run python -m colibri.cli gateway status
```

- `gateway run`：前台运行，适合调试或 systemd/supervisor 托管。
- `gateway start`：后台启动，命令立即返回。
- `gateway stop`：停止后台 gateway。
- `gateway restart`：重启后台 gateway。
- `gateway status`：查看运行状态、PID、RSS、配置路径、日志路径等。
- `gateway status` 还会显示 `agent_status=healthy|unhealthy`；进程是否运行与 Agent 是否健康相互独立。
- 运行中的 Gateway 和 REPL 会在下一轮对话前热加载配置中的 `[model]` 和 `[vision]`。无效修改会被拒绝并继续使用上一份可用配置；其他配置项需要重启后生效。

后台状态和日志：

```text
~/.colibri/run/gateway.json
~/.colibri/logs/gateway.log
```

裸命令 `colibri gateway` 不再启动阻塞服务，只显示可用动作。

## 微信 Channel

私有配置示例：

```toml
[gateway]
enabled_channels = ["weixin"]
max_sessions = 4
session_idle_seconds = 600
max_pending_inbound = 8
max_concurrent_turns = 1

[channels.weixin]
enabled = true
token = "..."
base_url = "https://ilinkai.weixin.qq.com/"
allow_from = []
```

登录：

```bash
uv run python -m colibri.cli auth weixin
```

Gateway 会按微信用户维护独立的 `AgentSession`，key 类似：

```text
weixin:<sender_id>
```

工具权限确认会通过微信文本发给用户，使用数字回复。

工具执行过程中，同一用户再发一条消息可**改方向**：跳过剩余工具并注入新指令，Colibri 会立即回复简短中文确认（如 `已改方向，跳过剩余 N 个工具`）。

## 内置工具

- `files.list`：有界、排序的目录分页；使用 `next_after` 继续。
- `files.read`：按行范围或 `offset_chars` 读取 UTF-8 文本；截断时返回准确的字符续读位置，偏移针对脱敏后的文本。
- `shell.run`：执行 shell 命令；前台长输出返回有限头尾、退出码和脱敏归档路径，可按需回读，无需重复执行命令。
- `memory.list`：列出 always-on 记忆文件和 topic 文件。
- `memory.read`：读取 `SOUL.md`、`USER.md`、`MEMORY.md`、`INDEX.md` 或 topic 文件。
- `memory.search`：搜索 `INDEX.md` 目录行；详细 topic 需要再单独读取。
- `memory.write`：追加或替换记忆文件。
- `skill.read`：按 catalog 中的 name 读取完整 `SKILL.md`。
- `skill.run`：运行本地 skill 中配置的命令。

工具输出受 `tools.max_result_chars` 约束；列表、读取和后台查询另有有界分页窗口。

## 访问模式

各入口统一使用完全访问模式，不进行工具权限审批或持久授权。系统提示词要求 Agent 在危险操作前向用户确认；Linux 用户和外部服务自身的权限仍然有效。

## 记忆

持久记忆是 Markdown 文件：

```text
~/.colibri/memory/
  SOUL.md
  USER.md
  MEMORY.md
  INDEX.md
  topics/
    system-info.md
    colibri-design.md
```

当 `memory.enabled = true` 时，Colibri 会把 `SOUL.md`、`USER.md` 和 `MEMORY.md` 作为有界 always-on 上下文注入模型。详细记忆检索交给模型判断：模型先用 `memory.search` 搜索 `INDEX.md`，再用 `memory.read` 读取关联的 `topics/*.md` 文件。自动注入受 `memory.max_recall_chars` 限制。

如果 memory 目录不存在，或目录中没有任何文件，Colibri 会在首次加载 memory 时创建样例 `SOUL.md`、`USER.md`、`MEMORY.md`、`INDEX.md` 和 `topics/sample.md`。已有 memory 文件不会被覆盖。

`SOUL.md` 应控制在 400 字符以内，`USER.md` 应控制在 400 字符以内，`MEMORY.md` 应控制在 1200 字符以内。如果某次 `memory.write` 写入后导致文件超限，工具结果会提醒模型合并整理，并用 `mode="replace"` 重写。

## 本地 Skills

Skills 统一放在单一目录：

```text
~/.colibri/skills/<name>/SKILL.md
~/.colibri/skills/<name>/scripts/...  # 可选
```

每个 `SKILL.md` 都必须以 YAML frontmatter 开头，其中的 name 必须与目录名一致：

```markdown
---
name: example
description: 当用户需要示例工作流时使用。
commands:
  - name: check
    description: 运行本地检查。
    command: python
    args: [scripts/check.py]
    read_only: true
---

# Example Skill
```

每轮只注入有界 skill catalog（name / description / command names / path）。模型需要完整说明时调用 `skill.read`；当已配置的 command 与用户请求匹配时，应使用 `skill.run`，而不是通过 `shell.run` 直接执行底层命令。

内置 `create-colibri-skill` 仍是指导 skill，不从磁盘扫描。

## 模型网络恢复

临时模型故障（网络/DNS 错误、超时、HTTP 408/429 和 5xx）会进行有界指数退避重试。默认重试两次，等待 500ms 和 1000ms：

```toml
[model]
max_retries = 2
retry_backoff_ms = 500
```

重试耗尽后只结束当前 turn。REPL 会重新显示提示符，gateway session 仍可处理后续消息。Channel adapter 只负责传输，不包含模型重试策略。

## 上下文与内存相关默认值

```toml
[model]
max_output_tokens = 16384
timeout_seconds = 60
input_context_tokens = 48000
max_retries = 2
retry_backoff_ms = 500

[session]
max_tool_rounds = 32
trigger_message_limit = 96
recent_message_limit = 12
summary_max_chars = 12000
model_compact = true
transcript = true

[tools]
max_result_chars = 32000

[gateway]
max_sessions = 4
session_idle_seconds = 600
max_pending_inbound = 8
max_concurrent_turns = 1
```

当 session 达到 `trigger_message_limit` 条消息，或估算模型输入 token 达到 `model.input_context_tokens` 的 80% 时，Colibri 会把当前消息缓冲压缩进滚动摘要，并保留最近 `recent_message_limit` 条消息。如果运行时没有 tokenizer，则按 UTF-8 字节数除以 4 估算 token。它不会再额外通过裁剪旧消息来控制输入大小。

## Transcript

执行中的新消息通过公共 steering 在安全处理节点补充当前任务。精确发送 `/stop` 会优先取消后续执行，保存已完成步骤与实际执行记录组成的阶段性结论；已发出的远端操作和已经发生的修改不会自动撤销。掌机退出复用此停止路径，菜单、拼音和录音在 Agent 执行期间仍可使用。

当 `session.transcript = true` 时，Colibri 写入 JSONL：

```text
~/.colibri/transcripts/YYYY-MM-DD.jsonl
```

CLI、REPL、gateway 和掌机 App 共用按天日志；每次写入前检查日期，跨北京时间午夜自动切换文件，保留策略由同一公共 writer 执行。App 的 `/clear` 仅隐藏此前的显示记录，不删除日志。Gateway 事件会额外包含 `channel`、`sender_id`、`session_key` 等 metadata。

## 状态与诊断

当 `console.status = true` 时，状态行写到 `stderr`：

```text
[colibri] ready model=fake-colibri-model
[colibri] thinking
[colibri] tool files.read ok chars=1284
```

模型回答保留在 `stdout`。默认 `console.plain_answer = true`，会对本地 `ask`/`repl` 的最终回答做小屏友好去噪（去掉粗体/代码标记、表格改成普通行）；设为 `false` 可恢复原始 markdown。微信 gateway 发出的消息不受影响。

诊断命令：

```bash
uv run python -m colibri.cli diagnostics
```

## Systemd

示例服务文件：

```text
deploy/systemd/colibri-repl.service
deploy/systemd/colibri-gateway.service
```

Gateway 开机自启用 `colibri-gateway.service`：前台执行 `gateway run`（不要用 `gateway start`）。  
微信 channel 不监听本地端口，只要设备能出网访问微信 API，且 `channels.weixin` 已配置 token，即可在 systemd 下正常收发。

模型历史恢复与界面历史显示分开：重启时逐行读取保留的 transcript，原样重放当前会话最新模型流中的摘要、消息、工具结果和运行时上下文位置；不再提供 `restore_message_limit`、`restore_char_limit`、`restore_scan_bytes`。界面加载窗口不裁剪模型历史，后续压缩统一按模型上下文预算及可选的 `session.max_context_memory_mib` 控制。掌机聊天历史首屏显示最新一页，滚动到边界时按需加载相邻窗口；Fn+L/Fn+M 直接跳到最早/最新消息，`/clear` 仍只设置显示边界。

### 工具输出与编辑验证

前台 `shell.run`、`skill.run` 的模型可见预览最多 4096 字符（同时受 `tools.max_result_chars` 约束），退出状态和归档元数据另行附带。长输出按 stdout/stderr 分别保存在运行目录的 `artifacts/tool-results/`，小输出无需写盘。归档前移除当前配置中的已知凭据；不保证识别未知秘密。每流最多保存 16 MiB，总预算 64 MiB；空间或 I/O 异常会明确提示无法归档，原命令成功与否仍按真实执行结果报告。`complete=false` 表示保存的输出不完整。第一阶段不会自动删除可能仍被历史引用的归档；预算耗尽时退化到预览。

通过 `files.read` 的 `start_line` / `end_line`，或归档专用的 `offset_chars` / `max_chars` 读取必要片段。字符分页返回 `next_offset_chars` 与 `has_more`，支持普通文本和没有换行的长输出。原始 stdout/stderr 的交错时间顺序不予重建。

`files.patch`、`files.write` 可显式指定 `verify_command`，以及可选 `verify_cwd`、`verify_timeout_seconds`。工具先修改，再执行前台验证；返回 `edit=applied` 和独立的验证状态。验证失败、超时或取消不会撤销已经发生的修改，不应因此重复应用同一补丁。未指定验证命令时保持普通文件操作；不会自动猜测测试脚本。

### 查询与大结果续读

- `files.list` 默认每页 50 项，最多 200 项；按返回的 `next_after` 设置 `after` 继续。目录变动不保证跨页快照一致。
- `files.read` 短行范围读取仍返回文本；发生截断或指定 `offset_chars` 时返回 JSON，包括 `text`、`next_offset_chars`、`has_more` 和续读参数。字符偏移针对脱敏后的文本；不要与行范围混用。
- `files.search` 返回命中附近的片段；达到结果上限时用 `continuation.after_path`、`continuation.start_line` 继续。扫描预算、不可读路径或超长行导致不完整时会说明限制，不保证能继续；应缩小范围或使用 Shell 流式搜索。
- 大型 MCP 结果使用脱敏归档。MCP 返回合法 JSON，保留错误标志和内容类型，不代表非文本内容已经被理解。
- `shell.poll` 的 `page_has_more` 只表示当前日志页尚未读完；`truncated` 表示采集截断或采集异常。`output_path` 是保留的头部/尾部日志位置，不能当成完整原始输出。命令退出码与日志采集错误分开报告。

### Memory 与硬件工具

`memory.read` 支持行范围、脱敏字符分页和可选 `include_version=true`；返回的 SHA-256 可用于 `memory.write.expected_sha256`，新文件可指定 `missing`。`memory.list` 小列表仍一次返回，大列表使用 `after` 续查；`memory.search` 仅搜索 INDEX.md，返回明确错误、停止原因与续查参数，不会把索引读取失败当成没有记忆。

`memory.write` 在 Memory 根目录加跨进程锁，追加和替换都使用原子文件提交。Topic 可通过 `index_description` 显式一起更新索引；`expected_index_sha256` 可校验索引版本。Topic 与索引是两个原子提交，不是跨文件事务：若 `applied=true` 但索引失败，Topic 已保存，只补索引，不要重复追加。Memory 自动加载规则与 App 菜单只读行为保持不变。

`serial.read` 支持 `encoding=auto|text|hex`，默认根据数据选择文本或十六进制；`bytes_read` 表示实际消费的字节数，已知凭据先脱敏再编码，`redacted` 标记数据是否因此变化。已消费数据从结果或归档回读，不应再读一次设备来恢复。`serial.write` 支持文本与 hex 输入，成功仅表示本地传输接受字节，不能当作物理执行已验证。控制请求超时、取消或响应异常时，`outcome_unknown` 和 `bytes_written` 帮助判断是否可能已经执行；不会自动重发。设备 FD 的 advisory lock 协调 Colibri 的跨进程访问，不能约束不遵循该锁的外部程序。
