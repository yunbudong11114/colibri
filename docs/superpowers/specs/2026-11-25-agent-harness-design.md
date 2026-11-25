# 通用 Rust Harness 能力增强（0.3.0）

## 目标和边界

基于当前 0.2.4 执行循环与已审阅的 DeepSeek Harness、Codex unified_exec 和 Anthropic 长任务实践，改进可观察执行、任务接续、文件操作和工具发现。保持完全访问，危险操作由系统提示要求确认；不增加权限审批层或设备特有的核心逻辑。Mac CLI、REPL、gateway 和掌机协议共用同一实现。

核心只知道操作系统、工作目录、运行时数据目录、作用域和工具；Cardputer 的屏幕、键盘、中文标签和布局在 adapter/C++ UI 中。去掉系统提示词写死 CardputerZero 的身份。运行环境事实稳定加入 runtime_context，仅变化时追加，保留已有缓存策略。

## 1. 命令作业

保留低成本同步 shell.run，增加 background、cwd、timeout_seconds 参数；长任务由模型选择后台模式。新增 shell.poll / shell.cancel，统一返回 job_id、status、exit_code、增量输出位置、截断标记。后台命令由同一 Rust 可执行程序的私有 supervisor 模式管理，跨 App/CLI/gateway 退出仍可查询。取消通过控制文件交给实际持有 Child 的 supervisor，不依据外部 PID 盲目杀进程。

每个作业单独进程组，超时/取消清理整组。默认每个运行时至多一个后台命令，输出按字节有界；原始进程输出经已知凭据的流式脱敏后落盘，私有文件 0600。大输出保留受限前段与尾段，读取时再次脱敏，超出范围明确提示。执行命令和用于脱敏的凭据通过匿名管道传入 supervisor，不在作业元数据保存明文凭据。失去 supervisor 时标记 interrupted/unknown，不宣称命令完成，不自动重放有副作用的命令。

不引入 PTY 交互或新的常驻服务；当前工具约定非交互命令。持久化 metadata、进程身份和状态，清理已结束旧作业，限制总目录占用。

## 2. 当前任务和证据

增加 task.start / task.update / task.inspect。按已有逻辑会话作用域持久化一个当前任务，不增加用户多会话界面。记录目标、步骤、验收条件、下一步、阻塞原因和真实工具结果引用；已有任务被替换需显式 replace。状态文件原子写入并加锁，限制尺寸和证据条数。

执行工具成功后，harness 为当前任务产生 evidence_id；后台任务开始不等于完成，需查询其终态。已完成步骤必须引用当前任务已有的成功结果。证据存在只证明工具实际执行，不替代验收条件的语义判断。模型须运行适合任务的外部检查。

无工具调用但任务仍 pending 时最多提醒模型一次；若仍结束，返回明确的 task_incomplete 状态和未完成说明，不假报完成、不恢复旧的固定工具轮数上限。重复相同失败三次时提醒改变策略。仅复杂执行任务使用任务工具，普通问答不强制生成计划。重启/压缩后按需恢复当前任务摘要；任务状态与长期 memory 分开。

## 3. 文件工具

files.read 改为流式有界读取，范围扫描不分配整文件，超长单行也有界；保持 UTF-8 和凭据脱敏。补充 files.search（字面搜索、行号、显式扫描/输出边界）和 files.patch（唯一旧文本匹配、可选预期哈希、原子替换、保留权限）。局部编辑拒绝超过内部安全内存界限的文件，并建议使用流式系统工具。文件工具操作仍是完全访问，不引入路径授权策略。

## 4. MCP 渐进发现

小规模工具保持直接暴露。工具超过阈值时只暴露 mcp.search 和近期按需展开的工具定义；搜索返回名字、说明与输入 schema，并在下一轮加载相应定义。保持稳定排序和有界暴露数量，不改变实际执行权限。技能已有目录+skill.read 路径，继续复用。

## 5. App Launcher

增加只读“任务进度”菜单页，复用现有 Markdown 阅读布局、字体和返回/滚动规则；展示目标、步骤、下一步和后台作业状态，不显示原始工具大输出。新增执行/等待/规划 activity 使用已有 A 动效及相同间距。核心事件用通用标识，中文由掌机适配层映射。

扫码展示选择从核心里的设备路径识别移到掌机适配层；通用认证 presenter 仅使用宿主显式提供的 viewer。掌机原生 CLI 环境可注入 viewer，SSH 继续字符版。

## 6. 评测、验证、发布

保留 0.2.4 二进制作为基线。开发评测脚本仅用隔离目录和固定任务；支持用同一配置对基线/新二进制进行真实模型评测，关闭个人 memory、MCP、技能和会话日志，不把凭据放入报告。区分确定性工具/恢复测试与真实模型评测，不用单次结果宣称成功率提升。

覆盖后台启动/增量读取/取消/超时/重启查询/输出上限与脱敏；任务进度恢复及无证据完成拒绝；文件大行与范围读取、陈旧编辑、搜索截断；MCP 搜索展开；适配层任务页与活动映射。测试后编译部署 Mac 和掌机，版本 0.3.0，核对已有配置和 memory 未覆盖，重启运行中的 gateway 并验证连接。实际包体积和内存边界用实测记录。

## 实现边界补充

宿主通过 `with_runtime_home` 和 `with_working_directory` 分别提供运行时数据目录与命令工作目录，两者不混用；模型容量缓存跟随运行时目录。作业和任务模块不依赖 App Launcher。掌机中文进度格式集中于 `cardputer/task_view.rs`，activity 使用通用标识并在适配层映射。任务进度菜单无需配置模型即可读取，退出回到原菜单项，阅读区不预留输入框。新增执行/等待状态使用现有三点动画。

参考的上游工程：
- [DeepSeek Harness jobs](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/subsystems/jobs.md)
- [DeepSeek Harness shell](https://github.com/deepseek-ai/deepseek-harness/blob/master/docs/subsystems/shell.md)
- [Anthropic 长任务 harness](https://www.anthropic.com/engineering/effective-harnesses-for-long-running-agents)

实测报告见 `outputs/2026-11-25-harness-eval/`。三个真实模型任务仅作冒烟对比，不据此推断成功率提升。

真机视觉校验修订：任务阅读页当前采用 LVGL 文本阅读控件，因此适配层输出紧凑纯文本，不输出 Markdown 标记；顶部目标与完成比例相邻，步骤单行分隔，省略空的下一步/阻塞段落，让首屏尽量覆盖完整步骤。

接续补充：runtime_context 保留最近三个作业的 ID 与状态（包括完成/失败），以便进程重启后仍能 poll 验证已结束命令；不注入大输出、动态字节数或轮询时间。排序按创建时间和 ID 稳定排列，避免同秒作业引起缓存抖动。
