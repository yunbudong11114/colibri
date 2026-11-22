# colibri · Cardputer Zero

用于 Cardputer Zero 的独立应用。C++/LVGL 图形界面和 REPL 共用 Rust AgentSession、本机设置、MCP 和历史策略。

## 安装和使用

在 Cardputer Zero 的 Debian 13（ARM64）上安装 release 包：

```sh
sudo apt install ./Colibri_0.1.60_m5stack1_arm64.deb
```

从 APPLaunch 打开 **Colibri**。首次使用进入 **Esc → 设置 → 模型**，选择「服务商预设」或「自定义连接」。预设包含 OpenAI、DeepSeek、OpenRouter 和 Ollama。表单集中填写 Base URL、API Key 和模型 ID，选「保存连接」才生效。自定义支持兼容 Chat Completions 的服务，无鉴权的本地服务可以留空 API Key。Ollama 跑在另一台机器时，把 Base URL 换成设备可达的地址。REPL 可用 `/set model.provider fake` 做离线输入检查。

| 按键 | 操作 |
| --- | --- |
| Enter | 发送草稿，或打开当前菜单项 |
| Esc | 短按松开打开菜单或返回，长按 1.5 秒退出 |
| Page Up（Fn+L） / Page Down（Fn+M） | 跳到第一条 / 最后一条可见消息 |
| Home（Fn+K） / End（Fn+N） | 草稿光标跳到行首 / 行尾 |
| Fn+空格 | 在聊天中开始或结束语音录音。Esc 取消录音 |
| Tab | 切换英语和拼音。在设置编辑页清空当前字段 |
| ↑ / ↓ | 滚动聊天或选择菜单项 |
| F / X / Z / C | 菜单和设置列表可直接用 F/X 选择、Z 返回、C 打开 |
| ← / →，1–6 | 翻页或选择拼音候选。Space、Enter 选当前页第一项 |

**在聊天页按 Fn+空格**直接开始录音，按第二次结束并转写，Esc 取消。单段最多 60 秒，转写进入草稿后可编辑，再按 Enter 发送。设备默认通过系统 PipeWire 使用板载 ES8389 麦克风。用 `COLIBRI_CAPTURE_DEVICE` 可以切换 ALSA 输入。独立转写地址使用其自己的密钥，留空时共用模型服务地址与密钥。

语音转写方式可设为 `offline`（设备本机中英双语）、`online`（在线 API）或 `auto`。包含离线模型的安装包在未单独配置语音 API 地址时，自动优先本机转写。离线模式使用 Sherpa-NCNN 小型中英双语 Zipformer，无需网络或密钥。中英混说与字母缩写可能需要在草稿里修正。

**终端 REPL**使用 `colibri --repl`。支持 `/new`、`/voice`、`/set KEY VALUE`、`/mcp` 和 `/quit`。图形界面的聊天和记忆编辑支持离线连续全拼，例如输入 `nihao` 后选「你好」，终端由终端输入法提供中文输入。

## 图片理解

在 **Esc → 设置 → 图片理解** 可以编辑视觉模型、API 地址、密钥、超时和图片大小上限。已有 `config.toml` 的 `[vision]` 配置会显示在菜单中，密钥以掩码显示。菜单保存的字段优先。清空模型、地址或密钥可沿用对应的聊天设置。

## 长期记忆

在 **Esc → 设置 → 记忆** 可以启用长期记忆或修改目录。它默认开启，从设备用户的 `~/.colibri/memory/` 加载 `SOUL.md`、`USER.md`、`MEMORY.md`。界面与设备 REPL 共用这些文件和设置。记忆菜单可阅读并编辑上述三个文件、`INDEX.md` 和 `topics/` 下已有的主题文件。阅读页按 C 或 Enter 编辑，Tab 切换中英文，Enter 换行，Esc 打开保存菜单。选「保存修改」后才写回设备文件。

主菜单的 **MCP 与技能** 可查看名称和说明。MCP 服务的说明取配置中的 `description`；技能说明来自 `SKILL.md` 的 `description`。两类列表都可离线查看。

启用长期记忆后，可以在聊天或 REPL 里请 Colibri 记住一条信息。Agent 可以直接追加或覆盖记忆。

## 配置和 MCP

默认数据目录为 `~/.colibri/`，与原版共用 `COLIBRI_HOME`。设备设置存放在仅当前用户可读写的 `config.toml`，历史放在 `transcripts/`。首次启动会复制旧设备目录中的设置和历史，保留原文件与目标目录内已有文件。设备专用覆盖项 `COLIBRI_CARDPUTER_HOME` 仍可用。技能目录为数据根目录下的 `skills/`，技能声明的命令直接执行。历史策略支持保存开关、启动恢复、保留天数和目录容量，天数或容量设为 `0` 表示关闭该项限制。

搜索支持现有 Colibri 的 `baidu` 和 `aliyun_mcp` 适配器。填写对应的 API 地址和密钥后，可在对话中要求模型搜索。

在图形设置的 **MCP · 服务器 JSON** 输入服务器数组，也可在 REPL 使用如下示例：

```text
/set mcp.servers [{"name":"catalog","url":"https://mcp.example.org/mcp","token":"TOKEN","read_only":["lookup"]}]
/mcp catalog list_tools
/mcp catalog list_resources
/mcp catalog read_resource fixture://sample
/mcp catalog list_prompts
/mcp catalog get_prompt greet {"name":"Mina"}
```

本地服务器使用 `{"name":"local","command":"/absolute/path/server","args":[],"env":{}}`。MCP 和内置工具调用直接执行。文件和 Shell 按 Linux 应用账号权限运行。可用数据目录的 `config.toml` 补充 Shell、视觉和硬件设置。

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
