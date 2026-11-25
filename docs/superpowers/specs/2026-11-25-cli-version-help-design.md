# CLI 版本与帮助入口

Mac 和掌机复用 Rust CLI，支持 `colibri version`、`colibri --version`，输出 `Colibri <Cargo 包版本>`。版本号使用 `env!("CARGO_PKG_VERSION")`，不重复维护字符串。

同时提供 `colibri help`、`colibri --help`，列出现有主要命令及配置路径参数，包含微信认证、网关、REPL、诊断、硬件开发入口。无参数继续返回用法错误，不改变现有调用行为。

版本和帮助在读取配置前处理，配置不存在或损坏时仍能查询，且不读取凭证、不联网、不启动 Agent。显式 --config 路径可出现在入口参数前。

验证两类别名在不存在/损坏配置下工作，然后编译原生 macOS CLI 和 ARM64 worker。保留用户配置，部署后验证两端版本及帮助输出。掌机 deb 同步重打包，版本保持本次要求的 0.2.1。
