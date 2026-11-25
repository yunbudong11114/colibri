#pragma once
#include <nlohmann/json.hpp>
#include <algorithm>
#include <string>
#include <string_view>
#include <vector>

// Wire framing has no process, UI or device dependencies.
class JsonLineDecoder {
public:
    explicit JsonLineDecoder(std::size_t max_frame = 1024 * 1024) : max_frame_(max_frame) {}
    void feed(std::string_view chunk, std::vector<nlohmann::json>& messages) {
        while (!chunk.empty()) {
            const auto newline = chunk.find('\n');
            const auto size = newline == std::string_view::npos ? chunk.size() : newline;
            if (!discarding_) {
                if (size > max_frame_ - pending_.size()) {
                    std::string().swap(pending_);
                    discarding_ = true;
                    messages.push_back({{"type", "error"}, {"text", "运行时消息超过传输上限"}});
                } else pending_.append(chunk.data(), size);
            }
            if (newline == std::string_view::npos) return;
            if (!discarding_) {
                auto value = nlohmann::json::parse(pending_, nullptr, false);
                if (value.is_object()) messages.push_back(std::move(value));
                else messages.push_back({{"type", "error"}, {"text", "运行时消息格式错误"}});
            }
            pending_.clear();
            if (pending_.capacity() > 65536) std::string().swap(pending_);
            discarding_ = false;
            chunk.remove_prefix(newline + 1);
        }
    }
    std::size_t buffered_bytes() const { return pending_.size(); }
private:
    std::size_t max_frame_;
    bool discarding_ = false;
    std::string pending_;
};
