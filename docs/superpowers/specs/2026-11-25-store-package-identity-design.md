# 官方商店包名与安装路径统一

## 用户确认

应用、命令和安装包统一使用 colibri，界面显示名 Colibri，发布版本 0.2.1。商店路径检查以 Debian Package 为前缀，原来的 Package=colibri-cardputer 与 usr/bin/colibri 不匹配。保留命令 colibri，修改包身份，避免给用户引入新命令。

## 安装布局

- Debian Package 与 CMake project/target：colibri。
- /usr/bin/colibri：图形入口及 CLI/REPL 转发。
- /usr/lib/colibri/worker：共享 Rust 适配入口，构建目标 colibri-worker。
- /usr/lib/colibri/offline-speech：唯一离线转写引擎与模型。
- /usr/share/colibri/fonts 与 pinyin：字体与词库。
- /usr/share/doc/colibri：文档及第三方许可证。
- /usr/share/APPLaunch/applications/colibri.desktop，Exec=/usr/bin/colibri。
- /usr/share/APPLaunch/share/images/colibri.png，desktop Icon 使用绝对路径。

所有运行时资源路径同步更新，MCP clientInfo 名称 colibri，版本采用 Cargo 编译版本。用户配置、Memory、技能与日志继续使用 ~/.colibri，不动内容。产品名与安装身份统一，设备适配源码目录 cardputer-zero 与 cardputer 模块按职责保留。

## 包替换与验证

新包声明 Conflicts/Replaces: colibri-cardputer，交给 Debian 包管理器处理旧包文件所有权，避免覆盖同名 /usr/bin/colibri 失败。这只负责软件包替换，不引入运行时旧路径回退。安装包名改为 Colibri_0.2.1_m5stack1_arm64.deb，Rust crate 版本与锁文件同步为 0.2.1。

增加安装清单检查脚本，按用户给出的路径白名单、规范化前缀、文件类型、文件权限、udev/systemd限制检查，同时验证必须的 desktop、Exec、Icon 与包内文件对应。测试明确拒绝旧包名下 usr/bin/colibri，接受新包名相同命令。完成 Rust 测试、桌面与 ARM64 编译、新 deb 完整清单验证，并在设备通过 apt 本地安装实现替换，验证旧包已移除、新命令与词库资源工作、用户数据保持不变。旧构建缓存内 COLIBRI_RUST_WORKER 参数必须改指新的 Rust 构建目标，避免误用旧 worker。

## 完成情况

已生成 Colibri_0.2.1_m5stack1_arm64.deb，Debian Package=colibri，Version=0.2.1-m5stack1；Rust crate 与 MCP clientInfo 同步 0.2.1，worker 构建名 colibri-worker。安装资源及桌面入口全部使用新路径。

验证：最终 214 项 Rust 测试、3 项路径规则测试、原生菜单/动效/熄屏组件测试、桌面和 ARM64 Release 构建通过。新包解包后完整清单通过按用户规则实现的检查器，desktop Exec/Icon 指向包内实际绝对路径，新 worker 与构建产物校验一致。没有以此声称通过全部官方上架审核，许可证和商店截图仍需另行准备。

设备替换：APT 模拟安装确认只移除 colibri-cardputer、安装 colibri。首次实际调用额外加了 --no-download，APT 在移除旧包后报本地路径错误，随后使用 dpkg 完成新包安装；标准不带 --no-download 的 APT 复核输出 Setting up colibri (0.2.1-m5stack1)。USB 连接中断并恢复后，核对已安装版本及 dpkg --audit 均正常。该问题不通过改名命令或制造 /usr/bin 非法别名规避。

真机安装后的 CLI、REPL、整句拼音、隐藏语音 API 设置、图形界面启动及正常长按退出均通过，另以 colibri 普通用户重复验证通过。旧私有目录、旧 desktop 与旧图标均已移除。替换前后 ~/.colibri/config.toml、Memory、技能及系统 config.json 的 SHA256 完全一致。安装后的临时包及测试夹具清理，仅本地保留 deb 供用户重新提交。
