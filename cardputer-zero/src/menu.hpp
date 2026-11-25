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
    {"web_search.engine", "搜索 · 引擎"},
    {"web_search.endpoint", "搜索 · API 地址"}, {"web_search.api_key", "搜索 · API 密钥"},
    {"session.max_context_memory_mib", "上下文内存上限（MiB）"},
    {"session.transcript", "历史 · 保存记录"}, {"session.restore_transcript", "历史 · 启动恢复"},
    {"session.transcript_retention_days", "历史 · 保留天数"},
    {"session.transcript_max_total_bytes", "历史 · 容量（字节）"},
    {"memory.enabled", "启用长期记忆"}, {"memory.root", "记忆目录"},
    {"mcp.servers", "MCP · 服务器 JSON"},
};
enum class MenuAction { Catalog, Settings, Compact, Clear, Shortcuts, Quit };
struct MenuEntry { const char* label; MenuAction action; };
inline constexpr MenuEntry MainMenu[]{
    {"MCP 与技能", MenuAction::Catalog}, {"设置", MenuAction::Settings},
    {"压缩上下文", MenuAction::Compact}, {"清理屏幕", MenuAction::Clear},
    {"快捷键", MenuAction::Shortcuts}, {"退出 Colibri", MenuAction::Quit},
};
inline const std::vector<std::string> CategoryNames{"模型", "图片理解", "搜索", "历史与上下文", "记忆", "MCP 配置", "显示"};
inline const std::vector<std::string> CategoryPrefixes{"model.", "vision.", "web_search.", "session.", "memory.", "mcp.", "display."};
inline constexpr std::size_t MemoryCategory = 4;
inline constexpr std::size_t DisplayCategory = 6;
inline constexpr std::size_t SettingsMenuIndex = 1;
inline constexpr std::size_t CatalogMenuIndex = 0;
} // namespace colibri_ui
