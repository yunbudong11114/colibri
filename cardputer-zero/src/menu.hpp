#pragma once
#include <nlohmann/json.hpp>
#include <string>
#include <vector>

namespace colibri_ui {
inline const nlohmann::json SettingLabels{
    {"model.provider", "模型 · 提供方"}, {"model.base_url", "模型 · API 地址"},
    {"model.model", "模型 · 名称"}, {"model.api_key", "模型 · API 密钥"},
    {"vision.model", "视觉模型名称"}, {"vision.base_url", "视觉 API 地址"},
    {"vision.api_key", "视觉 API 密钥"}, {"vision.timeout_seconds", "超时（秒）"},
    {"vision.max_image_bytes", "图片上限（字节）"},
    {"session.max_context_memory_mib", "上下文内存上限（MiB）"},
    {"session.transcript", "历史 · 保存记录"}, {"session.restore_transcript", "历史 · 启动恢复"},
    {"session.transcript_retention_days", "历史 · 保留天数"},
    {"session.transcript_max_total_bytes", "历史 · 容量（字节）"},
    {"memory.enabled", "启用长期记忆"}, {"memory.root", "记忆目录"},
};
inline const std::vector<std::string> CatalogKinds{"MCP", "技能"};
enum class MenuAction { Settings, Help, Catalog, Task, Memory, Quit };
struct MenuEntry { const char* label; MenuAction action; };
inline constexpr MenuEntry MainMenu[]{
    {"任务进度", MenuAction::Task}, {"记忆", MenuAction::Memory}, {"配置", MenuAction::Settings},
    {"帮助", MenuAction::Help}, {"插件", MenuAction::Catalog},
    {"退出 Colibri", MenuAction::Quit},
};
inline const std::vector<std::string> HelpSections{"快捷键", "斜杠命令"};
inline constexpr const char* HelpTexts[]{
    "Tab   切换英文 / 拼音\n"
    "Enter   发送 / 打开\n"
    "Esc   设置 / 返回\n"
    "长按 Esc 1.5 秒   退出\n"
    "Fn+空格   开始 / 结束录音\n"
    "录音时 Esc   取消\n"
    "↑ / ↓   滚动消息\n"
    "Fn+L / Fn+M   最早 / 最新消息\n"
    "Fn+K / Fn+N   输入行首 / 行尾\n"
    "← / →   拼音候选翻页\n"
    "1–6   选候选，空格 / Enter 选首项\n"
    "设置 F/X 选择，Z 返回，C 打开",
    "/clear   清理屏幕，保留历史和上下文\n"
    "/compact   压缩 Agent 上下文\n"
    "/stop   停止当前任务并报告阶段性结论\n"
    "以上命令在聊天输入框发送。"
};
inline const std::vector<std::string> CategoryNames{"模型", "图片理解", "历史与上下文", "长期记忆"};
inline const std::vector<std::string> CategoryPrefixes{"model.", "vision.", "session.", "memory."};
inline constexpr std::size_t MemoryCategory = 4;
inline constexpr std::size_t TaskMenuIndex = 0;
inline constexpr std::size_t MemoryMenuIndex = 1;
inline constexpr std::size_t HelpMenuIndex = 3;
inline constexpr std::size_t SettingsMenuIndex = 2;
inline constexpr std::size_t CatalogMenuIndex = 4;
} // namespace colibri_ui
