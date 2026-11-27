# colibri · Cardputer Zero

用于 Cardputer Zero 的独立应用。C++/LVGL 图形界面和 REPL 共用 Rust AgentSession、本机设置、MCP 和历史策略。

## 安装和使用

Debian 包名、终端命令均为 `colibri`，显示名为 **Colibri**。程序安装到 `/usr/bin/colibri`，私有 worker 和语音模型位于 `/usr/lib/colibri/`，字体与词库位于 `/usr/share/colibri/`，启动器入口为 `colibri.desktop`。配置仍使用 `~/.colibri/`。

运行 `colibri version`（或 `colibri --version`）查看编译版本；`colibri help`（或 `colibri --help`）查看命令用法。这些命令无需有效配置，也不会启动 Agent。

App、REPL、gateway 启动时共用 Rust memory 初始化：开启记忆后，在当前 `memory.root` 下补齐默认文件，已有内容不覆盖，不依赖 API Key 或首条消息。关闭记忆以及运行版本、帮助、状态查询时，不创建 memory。初始化失败会明确提示；可在设置里修正目录。

微信登录使用 `colibri auth weixin`：普通终端显示紧凑半块二维码；掌机原生 CLI 会临时打开小屏扫码页，扫码完成或按 Esc 取消后回到 CLI。无需添加配置。原始登录链接不再默认输出，二维码仍是临时登录凭据，请勿公开分享。


在 Cardputer Zero 的 Debian 13（ARM64）上安装 release 包：

```sh
sudo apt install ./Colibri_1.0.2_m5stack1_arm64.deb
```

从 APPLaunch 打开 **Colibri**。首次使用进入 **Esc → 设置 → 配置 → 模型**，选择「服务商预设」或「自定义连接」。预设包含 OpenAI、DeepSeek、OpenRouter 和 Ollama。表单集中填写 Base URL、API Key 和模型 ID，选「保存连接」才生效。自定义支持兼容 Chat Completions 的服务，无鉴权的本地服务可以留空 API Key。Ollama 跑在另一台机器时，把 Base URL 换成设备可达的地址。REPL 可用 `/set model.provider fake` 做离线输入检查。

| 按键 | 操作 |
| --- | --- |
| Enter | 发送草稿，或打开当前菜单项 |
| Esc | 短按松开打开设置或返回，长按 1.5 秒退出 |
| Page Up（Fn+L） / Page Down（Fn+M） | 跳到第一条 / 最后一条可见消息 |
| Home（Fn+K） / End（Fn+N） | 草稿光标跳到行首 / 行尾 |
| Fn+空格 | 在聊天中开始或结束语音录音。Esc 取消录音 |
| Tab | 切换英语和拼音。在设置编辑页清空当前字段 |
| ↑ / ↓ | 滚动聊天或选择菜单项 |
| F / X / Z / C | 菜单和设置列表可直接用 F/X 选择、Z 返回、C 打开 |
| ← / →，1–6 | 翻页或选择拼音候选。Space、Enter 选当前页第一项 |

**在聊天页按 Fn+空格**直接开始录音，按第二次结束并转写，Esc 取消。单段最多 60 秒，转写进入草稿后可编辑，再按 Enter 发送。设备默认通过系统 PipeWire 使用板载 ES8389 麦克风。用 `COLIBRI_CAPTURE_DEVICE` 可以切换 ALSA 输入。录音和转写均在掌机本地完成。

当前发行仅提供离线语音，使用 Sherpa-NCNN 小型中英双语 Zipformer，无需网络或密钥。API 转写入口及配置暂不释出，旧的语音服务配置不会触发在线回退。模型缺失时会明确提示。转写时输入框显示三个依次跳动的圆点，中英混说与字母缩写可能需要在草稿里修正。

**终端 REPL**使用 `colibri --repl`。支持 `/new`、`/voice`、`/set KEY VALUE`、`/mcp` 和 `/quit`。图形界面的聊天和记忆编辑支持离线连续全拼，例如输入 `nihao` 后选「你好」，终端由终端输入法提供中文输入。

## 自动熄屏

应用沿用系统 `Settings → Screen → DarkTime`，未设置时默认 30 秒，0 表示关闭自动熄屏。Colibri 不提供额外配置项。首次按键只唤醒屏幕，松开后再操作；熄屏期间 Agent、网关和转写继续运行。唤醒或正常退出恢复原亮度，不修改系统亮度设置。

## 图片理解

在 **Esc → 设置 → 配置 → 图片理解** 可以编辑视觉模型、API 地址、密钥、超时和图片大小上限。已有 `config.toml` 的 `[vision]` 配置会显示在菜单中，密钥以掩码显示。菜单保存的字段优先。清空模型、地址或密钥可沿用对应的聊天设置。

## 长期记忆

在 **Esc → 设置 → 配置 → 长期记忆** 可以启用长期记忆或修改目录。一级 **设置 → 记忆** 用于查看文件。它默认开启，从设备用户的 `~/.colibri/memory/` 加载 `SOUL.md`、`USER.md`、`MEMORY.md`。界面与设备 REPL 共用这些文件和设置。记忆菜单只读展示上述三个文件、`INDEX.md` 和 `topics/` 下已有的主题文件。F/X 滚动，C 或 Enter 刷新，Esc 返回。文件内容更新交给 Agent 的 `memory.write` 或通用文件工具，菜单没有编辑或保存入口。记忆开关和目录配置仍可修改。

在聊天输入 `/compact` 可压缩上下文，执行和压缩状态在消息区的同一条临时 Colibri 消息中更新，输入框保持可用；录音/转写保留输入区动效。执行时可以继续发送文字或语音补充（steering）、查看设置。发送 `/stop` 会停止后续执行并保存阶段性结论；退出 App 使用相同停止流程，不影响独立 gateway。`/clear` 用于清理屏幕，只隐藏旧消息，不删除历史，也不清除 Agent 上下文。可选上下文内存预算位于设置的“历史与上下文”，0 表示未设置。

“设置 → 插件”中的 **MCP / 技能** 可查看名称和说明。MCP 服务的说明取配置中的 `description`；技能说明来自 `SKILL.md` 的 `description`。两类列表都可离线查看。

启用长期记忆后，可以在聊天或 REPL 里请 Colibri 记住一条信息。Agent 可以直接追加或覆盖记忆。

## 配置和 MCP

默认数据目录为 `~/.colibri/`，与原版共用 `COLIBRI_HOME`。设备设置存放在仅当前用户可读写的 `config.toml`，历史与 CLI、REPL、gateway 共用 `transcripts/YYYY-MM-DD.jsonl`，按北京时间跨天轮转，不再生成 App 专属时间戳文件。旧日志仍可恢复；`/clear` 仅隐藏此前消息。首次启动会复制旧设备目录中的设置和历史，保留原文件与目标目录内已有文件。设备专用覆盖项 `COLIBRI_CARDPUTER_HOME` 仍可用。技能目录为数据根目录下的 `skills/`，技能声明的命令直接执行。历史策略支持保存开关、启动恢复、保留天数和目录容量，天数或容量设为 `0` 表示关闭该项限制。

网络搜索通过独立安装的本地技能提供。

MCP 服务器列表默认写在 `~/.colibri/mcp.json`。可在 `config.toml` 用 `[mcp]` 下的 `config_path` 指定其他文件，相对路径以主配置所在目录为基准。旧的 `[[mcp.servers]]` 首次读取时自动迁移。独立文件包含鉴权信息时应使用 `chmod 600`。

```json
[
  {"name":"catalog","url":"https://mcp.example.org/mcp","token":"TOKEN"},
  {"name":"local","command":"/absolute/path/server","args":[],"env":{}}
]
```

App 的 **设置 → 插件 → MCP** 可查看服务，REPL 可用 `/mcp catalog list_tools` 验证连接。MCP 配置修改后，在下一轮对话加载。

## 构建

桌面模拟器需要 Rust/Cargo、CMake ≥3.31、C++17 工具链、SDL2 和 FreeType。Linux 建议安装 Noto CJK 字体，也可用 `COLIBRI_FONT` 指定字体文件。桌面使用 Alt+空格测试录音。

```sh
cmake -S cardputer-zero -B cardputer-zero/build/desktop -DCMAKE_BUILD_TYPE=Release
cmake --build cardputer-zero/build/desktop --parallel
cardputer-zero/build/desktop/colibri
cargo test --manifest-path colibri-rust/Cargo.toml
```

Debian 13 ARM64 构建使用 Rust/Cargo、CMake、Ninja、C++ 编译器和 FreeType。运行 `./cardputer-zero/package.sh`。可用 `COLIBRI_RUST_WORKER` 指定事先构建的 ARM64 Rust worker。离线语音使用原有 `COLIBRI_OFFLINE_SPEECH_PAYLOAD` 模型目录。安装包生成在 `cardputer-zero/dist/`。

## 微信通道

在掌机终端用 `colibri auth weixin` 登录，用 `colibri gateway run` 或 `colibri gateway start` 启动，用 `colibri gateway status` 查看状态。可在子命令前传 `--config PATH`。掌机菜单、REPL、Rust CLI 与微信网关都使用同一份 `~/.colibri/config.toml`。菜单修改会写入该文件。旧 `settings.json` 首次启动自动迁移；`settings.json.legacy` 和 `config.toml.pre-cardputer-unification` 是迁移备份。Rust 应用入口统一使用完全访问，危险操作由系统提示要求先询问用户。安装后按原方式启动网关。 `colibri gateway status` 的 `channel.weixin` 显示连接状态。短暂断网会显示 `reconnecting` 并自动恢复；`auth_required` 时需要重新登录微信。各通道共用网关的退避机制。

聊天正文支持 Markdown 标题、强调、列表、引用、代码及表格。表格在窄屏上按行展开成纵向条目，链接显示文字。消息原文仍保存在历史记录中。

聊天正文使用 Noto Emoji 单色字形补充常见符号，国旗以地区代码显示。

启动器和商店采用 M3「振翅」薄荷绿像素蜂鸟图标。商店显示名为 **Colibri**，清单位于 `app-builder.json`。

参考 [Cardputer Zero 官方模板](https://github.com/CardputerZero/Template) 和 [AppBuilder](https://github.com/CardputerZero/AppBuilder)。

首次上架前需确认发布许可证，填写唯一的分享码，并按 AppBuilder 要求在首次提交中附上真机运行视频。当前清单已填写显示名、分类、截图及应用使用的系统权限。

### 上下文管理

在聊天输入框或 REPL 输入 `/compact` 可主动压缩旧上下文，完整聊天日志仍保留。
自动压缩按模型 token 预算触发。可在历史设置中填写可选的上下文 MiB 预算，默认不设置，0 表示清除额外限制。无需配置消息条数、近期保留数或摘要长度。
上下文容量默认自动识别，`input_context_tokens` 仅供自定义部署可选覆盖，`max_output_tokens` 控制输出预算。旧 session 压缩选项需要从 config.toml 删除。

模型容量在首次实际请求时联网获取，最多等待5秒。能力缓存有效期10分钟，过期后活动请求触发后台刷新；失败保留最近成功值。缓存位于 Colibri home 的 `cache/model-capabilities`，线上读取值不会写进 `config.toml` 或 App 配置页。启动界面和打开设置直接使用本地状态。

## 设置与首次模型配置

首页 Esc 打开“设置”，一级顺序为任务进度、记忆、配置、帮助、插件、退出 Colibri。配置包含模型、图片理解、搜索、历史与上下文、长期记忆和 MCP 配置。帮助包含快捷键、斜杠命令两个只读文本页。应用只保留深色风格，不提供主题切换。

模型未配置或字段不完整时，各入口统一提示“未配置 AI 模型”。App 可按 Enter 进入模型配置，或走“设置 → 配置 → 模型”。终端运行 `colibri configure`，按步骤选择服务、填写 API 地址、模型 ID 和密钥；可用 `colibri --config PATH configure` 指定文件。密钥不回显，确认后才保存；输入 `:cancel`、EOF 或拒绝保存不会改变配置。切换端点不会携带原密钥。向导只检查配置格式，不发联网认证请求。REPL 和 gateway 的提示均给出配置命令，gateway 在启动子进程前预检，后台不会等待配置输入。

未显式指定模型的用户配置不再回落到模拟模型；只有明确设置 `model.provider="fake"` 才使用模拟模型。

无输入框或底部提示栏的设置、配置、帮助和插件页面统一使用完整正文高度；聊天、编辑和实际显示提示的表单才预留底部区域。插件分类统一显示为 MCP、技能。
