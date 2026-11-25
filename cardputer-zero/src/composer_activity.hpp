#pragma once
#include <cstdint>
#include <initializer_list>
#include <string_view>

namespace colibri_ui {
enum class ComposerActivity { Idle, Recording, Thinking, Transcribing, Compacting };
inline ComposerActivity composer_activity(bool chat, bool busy, bool recording, std::string_view status) {
    if (!chat) return ComposerActivity::Idle;
    if (recording) return ComposerActivity::Recording;
    if (!busy) return ComposerActivity::Idle;
    if (status == "压缩上下文") return ComposerActivity::Compacting;
    if (status == "正在转写") return ComposerActivity::Transcribing;
    for (auto stage : {"思考中", "执行命令", "等待命令", "整理任务", "处理文件", "调用工具"})
        if (status == stage) return ComposerActivity::Thinking;
    return ComposerActivity::Idle;
}
inline bool has_activity_dots(ComposerActivity activity) {
    return activity == ComposerActivity::Thinking || activity == ComposerActivity::Transcribing || activity == ComposerActivity::Compacting;
}
struct ActivityDot { int offset_y; std::uint8_t opacity; };
inline ActivityDot activity_dot(ComposerActivity, std::uint32_t tick, unsigned dot) {
    const auto active = (tick / 180) % 3 == dot;
    return {active ? -3 : 0, 255};
}
} // namespace colibri_ui
