# Pi Agent 对 Colibri Rust 的可借鉴点

日期：2026-08-20
Pi 源码：`/Users/ybd/pi`
Pi 提交：`b7bb00b936dbe21b8e160b3e89efdec361846699`
Colibri 源码：`/Users/ybd/cardputer/colibri/colibri-rust`
状态：源码调研记录，不代表已经进入实现计划。

## 结论

Pi 值得借鉴，但不适合整体移植。它最有价值的部分不是 TypeScript 扩展生态、TUI 或多 provider 目录，而是以下运行时不变量：

1. 模型输出被截断时，不执行参数可能不完整的工具调用。
2. 从输出产生时就限制 shell 等工具的内存占用，而不是结束后再截断。
3. 保存真实模型 usage，并用它辅助上下文压缩与 prompt cache 诊断。
4. 上下文超限时采用一次有边界的压缩重试。
5. 把 steering 和普通 follow-up 消息分成不同队列。
6. 让持久会话有明确身份和边界，不从全局日志模糊恢复。
7. 将持久对话与本次模型请求视图分开，动态信息只在请求边界注入。

Colibri 在以下方面更适合自己的目标，不应该向 Pi 看齐：

- shell、file、tool、hardware 的权限确认与持久授权。
- 用户目录内固定 skill 根目录，不执行项目随附的任意扩展代码。
- 较少依赖、较小二进制、顺序工具执行和明确资源上限。
- 面向 CardputerZero 与无人值守 channel 的保守安全模型。

## Pi 当前存在的两套运行时

阅读源码时需要区分两条路径，不能把 Harness 文档描述的全部能力都当作默认终端 Agent 已经使用的行为。

### 传统 Coding Agent 路径

默认终端产品仍以 `AgentSession`、`agent-loop.ts` 和 `SessionManager` 为核心：

- `packages/agent/src/agent-loop.ts`
- `packages/agent/src/agent.ts`
- `packages/coding-agent/src/core/agent-session.ts`
- `packages/coding-agent/src/core/session-manager.ts`

这条路径已经具备事件化 agent loop、steering/follow-up、模型 usage、上下文压缩、JSONL 会话树和 provider cache 支持。

### Durable Harness 路径

Pi 同时在发展新的 `AgentHarness`：

- `packages/agent/src/harness/`
- `packages/agent/docs/harness.md`
- `packages/coding-agent/src/server/create-harness.ts`

该路径包含 lane、不可变 entry tree、可变 register、usage ledger、原子事务、`op.state` 程序计数器和 effect sandwich。Coding Agent 的 server 适配已经可以创建这套 Harness，但不能据此认为所有终端执行都已经迁移完成。

对 Colibri 的正确态度是借鉴其故障处理原则，不整体复制存储与并发模型。

## 优先级概览

| 优先级 | 项目 | Colibri 当前问题 | 建议 |
|---|---|---|---|
| P0 | 有界命令输出 | `shell.run` 无界收集；`skill.run` 可能因管道写满而阻塞 | 共用增量、有界的输出累积器 |
| P0 | 模型停止原因与 usage | 忽略 `finish_reason` 和 usage；可能执行截断工具调用 | 扩展 `ModelResponse` 并阻止 length 工具执行 |
| P1 | 压缩预算与超限恢复 | 全量按字节除以 4 估算；固定 80%；真实 overflow 不恢复 | 真实 usage + 尾部估算 + 输出预留 + 一次压缩重试 |
| P1 | 历史 session 边界 | 新 session 会恢复所有来源的最近完整轮次 | loader 按目标 `session_key` 恢复 |
| P1 | steering/follow-up | 活跃 turn 中所有纯文本都尝试作为 steer | 增加独立 follow-up 队列和 channel 路由语义 |
| P2 | typed agent events | session 内直接承担状态、日志和工具生命周期 | 轻量同步事件回调，不引入完整事件总线 |
| P2 | cache 可观测性 | 有稳定前缀和本地缓存，但不知道 provider cache 实际命中 | transcript 记录 cache read/write 和 miss 指标 |
| P3 | 崩溃恢复 | transcript 有 tool_call/tool_result，但恢复器忽略未完成 turn | 小型 turn checkpoint；不复制完整 Durable Harness |

## P0-1：有界命令输出

### Colibri 当前行为

`shell.run` 会启动两个线程：

```text
stdout thread -> read_to_end(Vec<u8>)
stderr thread -> read_to_end(Vec<u8>)
主线程 -> 等子进程退出
退出后 -> 拼接 stdout/stderr -> truncate(max_result_chars)
```

`max_result_chars = 32000` 只限制最终返回给模型的文本，不限制子进程运行期间的内存。

例如：

```sh
yes
```

如果运行 30 秒，命令可能产生数百 MB 或更多输出。最终虽然只返回 32KB，但进程运行期间两个 `Vec<u8>` 会持续增长，可能先触发 OOM。

`skill.run` 的情况更危险：父进程只在子进程退出后读取 stdout/stderr。如果子进程输出超过操作系统管道容量，子进程会等待父进程读取，父进程又在等待子进程退出，形成阻塞，最终只能靠超时结束。

### Pi 的做法

Pi 的 `OutputAccumulator` 在收到每个输出 chunk 时：

- 使用流式 UTF-8 decoder，避免切断多字节字符。
- 统计累计字节数和行数。
- 内存中只保留固定大小尾部。
- 需要保留完整输出时才写临时文件。
- 最终返回截断信息、总字节数和总行数。

### Colibri 建议

新增一个 Rust 内部共用组件，例如：

```text
BoundedOutputAccumulator
  max_chars / max_bytes
  total_bytes
  truncated
  rolling tail buffer
```

`shell.run` 和 `skill.run` 都必须在子进程运行期间持续消费 stdout/stderr。

低内存设备上建议与 Pi 有一点不同：

- 默认不把完整输出写入临时文件。
- 只保留有界尾部和统计信息。
- 如以后需要完整输出，再增加显式配置或专门工具。

目标不是让返回内容更短。当前返回已经会截断。目标是让“生成输出的全过程”都保持有界内存，并消除 skill 管道阻塞。

## P0-2：模型停止原因与 usage

### Colibri 当前行为

OpenAI-compatible 响应通常包含：

```json
{
  "choices": [
    {
      "finish_reason": "stop | tool_calls | length",
      "message": { "content": "...", "tool_calls": [] }
    }
  ],
  "usage": {
    "prompt_tokens": 1000,
    "completion_tokens": 200,
    "total_tokens": 1200
  }
}
```

Colibri 当前只解析 `message.content` 和 `message.tool_calls`。`ModelResponse` 没有 `finish_reason` 或 usage 字段。

这会产生两个问题。

#### 问题一：可能执行被截断的工具参数

模型可能生成：

```json
{"command":"curl ... | sh ..."}
```

当输出 token 达到上限时，provider 可能仍返回一个经过容错修复、能够解析的工具调用，但参数尾部已经丢失。仅做 JSON/schema 校验无法证明参数完整。

Pi 的规则是：只要 assistant 的停止原因是 `length`，该 assistant 消息里的所有工具调用都不执行，而是写入明确的错误 tool result，让模型下一轮重新发出完整调用。

#### 问题二：压缩和缓存判断只能猜

Colibri 当前按消息 UTF-8 字节数除以 4 估算 token。这对英文、中文、JSON、工具参数的误差不同，也不包含 provider 对 system/tool schema 的真实计数方式。

如果保存 usage，就可以：

- 用上一轮真实总上下文 token 作为基准。
- 只估算上一轮之后新增的消息。
- 更准确地触发压缩。
- 记录 prompt cache 的 read/write token。
- 判断稳定前缀优化是否真的有效。

### Colibri 建议

将模型结果扩展为类似：

```text
ModelResponse
  text
  tool_calls
  stop_reason
  usage

ModelUsage
  input_tokens
  output_tokens
  cache_read_tokens
  cache_write_tokens
  total_tokens
```

其中 provider 没有返回的字段使用 `None` 或 0，不能伪造精确值。

工具执行规则：

```text
stop_reason == length && tool_calls 非空
  -> 不执行工具
  -> 为每个 call 写 error tool result
  -> 下一轮让模型重新生成
```

正常的 `tool_calls` / `stop` 行为保持不变。

usage 应写入 transcript，但不必把完整 usage 长期复制到每一条内存消息。为了低内存，session 可只保存压缩判断所需的最后一次有效 usage。

## P1：压缩策略

Pi 的高价值做法包括：

- 优先使用最后一次有效 assistant usage。
- 只估算该 usage 之后的消息。
- 用 `context_window - reserve_tokens` 作为触发线，给下一次回答预留空间。
- 按 token 预算保留最近上下文，而不是固定消息数量。
- 真实 context overflow 后删除失败响应，压缩并只重试一次。
- 更新既有结构化摘要，而不是不断简单拼接摘要。

Colibri 已经正确按 message group 保留 assistant tool call 和对应结果，不需要改变这一不变量。

## P1：历史 session 边界

Colibri 的 `TranscriptHistoryLoader` 会按 `session_key` 分别配对 user/assistant，避免并发 channel 的问答错配；但是选择最近轮次时不再按目标来源过滤。Gateway 给每个新 session 注入的又是同一个无参数 loader。

因此当前行为更接近“恢复整个个人 Agent 的全局最近对话”，而不是“恢复这个 channel/sender 的会话”。

如果产品明确永远只有一个用户，这只是注意力策略问题；如果未来允许多个 sender 或 channel，则可能发生跨会话内容混入。

建议明确分层：

- `memory/*`：全局、长期、跨 channel 的用户与事实记忆。
- session transcript：当前 `session_key` 的短期对话连续性。

不需要增加 Pi 的 branch/tree UI，只需要让 history loader 接收目标 session key。

## P1：steering 与 follow-up

Pi 有两类队列：

- steering：当前任务还在执行时改变方向。
- follow-up：当前任务自然结束后再处理。

Colibri 目前在 gateway 中将活跃 session 收到的所有纯文本都优先尝试作为 steering。这样做响应快，但普通连续消息也可能跳过剩余工具。

建议先在 session 内建立两种语义，channel 如何分类可以后续决定。不要把“增加 follow-up”理解成取消现有 steering。

## P2：事件与请求视图

Pi 的 canonical conversation 不直接被 provider 特有转换或动态上下文污染。每次真正调用模型前才执行 `transformContext` 和 `convertToLlm`。

Colibri 当前时间上下文改动已经采用同一原则：构造本次 `model_messages` 后，把当前时间追加到最新 user message 的请求副本，不写回持久 `self.messages`。这个方向正确。

后续可以加入轻量 typed events：

- model request start/end
- assistant message
- tool start/end
- compact start/end
- turn end

transcript、REPL 状态和 channel 通知订阅这些事件即可。实现应保持同步回调和有界数据，不引入大型异步事件框架。

## P2：provider cache

Pi 会根据 provider capability 决定是否发送：

- prompt cache key
- session affinity
- cache retention
- Anthropic 风格 cache-control marker

同时统一记录 cache read/write token，并识别明显 cache miss。

Colibri 已经具备稳定 prompt 前缀、HTTP 连接复用、memory cache、skill scan cache 和 tool spec cache。下一步应先增加 usage 可观测性，验证真实 provider 是否命中，再决定是否增加 provider 专用请求字段。

不建议复制 Pi 的大型模型 catalog。Colibri 可用小型 capability flags 或配置覆盖完成需要的兼容。

## P3：崩溃恢复

Pi Durable Harness 的 effect sandwich 会在外部调用前持久化 intent，调用后持久化 settlement。崩溃恢复时可以区分：

- 尚未开始。
- 可能已经发生但没有结果。
- 已经完成。

Colibri transcript 已经在工具执行前写 `tool_call`，执行后写 `tool_result`，实际上已有最小 intent/result 记录。缺少的是恢复器对未配对工具调用的理解。

若未来需要恢复未完成任务，可以先实现轻量规则：

- 扫描最后一个 session turn。
- 找出没有 tool result 的 tool call。
- 将其标记为 `interrupted`，绝不自动重放写操作。
- 让模型根据 synthetic result 决定下一步。

不需要引入 lane、register、SQLite 或完整操作状态机。

## 不建议照搬

### 任意扩展执行

Pi 明确说明没有内置 sandbox，扩展和内置工具以启动用户的完整系统权限运行。Colibri 的权限确认和远程 channel 门禁更适合无人值守设备，不能为扩展便利削弱它。

### 完整 Durable Harness

完整 Harness 适合多 lane、服务端并发、崩溃精确恢复和成本审计，但会增加：

- 状态类型和恢复分支。
- 持久化写入频率。
- 内存索引。
- 二进制体积与测试面。

Colibri 当前只需要吸收 intent/result、停止原因和一次恢复边界。

### 默认并行工具

Colibri 工具包含文件写入、memory、shell 和硬件操作。并行执行会使权限、steering、副作用顺序和结果关联更复杂。保持顺序执行更符合设备资源和安全目标。

### 大型 provider/model catalog

Pi 为大量 provider 维护复杂兼容能力和价格目录。Colibri 当前 OpenAI-compatible 定位不需要同等规模，应采用少量能力开关并保持默认请求最小化。

## 推荐落地顺序

1. P0：为 `ModelResponse` 增加 stop reason 和 usage；禁止执行 `length` 响应中的工具调用。
2. P0：实现共用有界输出累积器，同时替换 `shell.run` 与 `skill.run` 的读取逻辑。
3. P1：用真实 usage 改进压缩触发，加入 context overflow 的一次压缩重试。
4. P1：按目标 `session_key` 恢复历史。
5. P1：内部区分 steering 与 follow-up。
6. P2：补 cache 指标和轻量 typed events。
7. P3：只有出现明确恢复需求时，再增加最小 turn checkpoint。

每一项进入实现前，都应先单独更新对应设计文档，并同步设计 Python/Rust 的一致行为与测试映射。
