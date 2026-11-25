# 共用 memory 初始化收口（0.2.4）

## 目标

App Launcher、REPL、gateway 使用同一个 Rust memory::initialize(config)。开启 memory 时，在进入运行状态前补齐 SOUL.md、USER.md、MEMORY.md、INDEX.md、topics/sample.md；完全使用解析后的 memory.root。已有文件（包括空文件）不覆盖，不再因为任意核心文件存在就跳过其他文件。关闭 memory 时不创建目录。

不依赖模型配置、网络或首条消息。version/help/diagnostics/gateway status/stop/auth 等非 Agent 运行入口不触发 memory 初始化。gateway start 的子进程 run 执行初始化。

## 并发与错误

使用 memory 根目录下的进程间文件锁串行化补齐操作；已齐全时快速返回。缺失模板先写同文件系统临时文件、设置仅用户读写、sync，再以 hard_link 原子发布，AlreadyExists 时保留目标，确保不会覆盖并发写入的用户文件。正常结束和错误路径清理临时文件。底层不支持硬链接或目录不可写时明确返回带文件路径的初始化错误，不能偷偷回退为可能覆盖的写入。默认 Mac APFS 与掌机 ext4 支持此操作。

移除 MemoryContext::load 中忽略错误的 bootstrap；保留调用共用 initialize 的幂等保护，并传播错误。App 协议入口发出 error 事件但保留菜单修复配置的能力，REPL/gateway 明确报错。运行中修改 memory.root 时，菜单读取/会话提交及热重载继续调用相同初始化函数。

## 入口

- 普通 CLI ask/repl：解析配置后、创建模型之前初始化。
- gateway run：开始创建通道/模型之前初始化。
- 掌机协议启动：设置解析后初始化，即使模型未配置也能看到默认 memory；失败通过协议 error 提示。
- 掌机 REPL：进入交互前初始化。
- 掌机 memory 菜单和新会话模型构造：使用当前配置调用共用函数。
- 热重载：候选配置初始化成功后才采用新运行时。

修正掌机 Settings 的默认 memory.root，以实际数据 home 为基准；显式配置仍覆盖。避免 --home 测试/隔离实例误写真实用户目录。

## 验证与发布

验证首次 App 启动无 API Key、首次 REPL（不发消息）、gateway 未启用通道启动、部分文件缺失、现有内容保留、关闭 memory、错误提示、多进程并发、查询命令无副作用以及显式根目录。使用隔离目录，不替换个人 memory。版本同步为 0.2.4（Debian 0.2.4-m5stack1），测试后编译打包部署 Mac 和掌机，确认个人配置与原有 memory 内容未被覆盖。

## 验收记录

225 项 Rust 测试全部通过（测试 HOME 隔离）；Mac release REPL 和 ARM 真机协议入口分别验证在第一条消息之前创建默认文件，ARM 与 Mac 均完成 8 进程并发验证。关闭 memory、查询不创建、部分补齐、空文件保留、错误报告及设置修复已覆盖。部署前后 Mac 7 个、掌机 8 个原有 memory 文件和配置校验和一致。新包路径检查、已知凭据比对通过。
