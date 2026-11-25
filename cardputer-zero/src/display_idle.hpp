#pragma once
#include <cstdint>
#include <filesystem>
#include <set>

namespace colibri_ui {
// Only physical input refreshes idle time. Progress and animation do not.
class IdlePolicy {
public:
    explicit IdlePolicy(std::uint32_t now) : last_input_(now) {}
    bool input(unsigned code, int value, std::uint32_t now) {
        last_input_ = now;
        if (value) held_.insert(code); else held_.erase(code);
        if (sleeping_ && value) { sleeping_ = false; swallow_ = true; }
        const bool consumed = swallow_;
        if (held_.empty()) swallow_ = false;
        return consumed;
    }
    bool due(std::uint32_t now, unsigned seconds) const {
        return !sleeping_ && held_.empty() && seconds &&
            std::uint32_t(now - last_input_) >= seconds * 1000u;
    }
    bool sleeping() const { return sleeping_; }
    void sleep() { sleeping_ = true; }
    void wake(std::uint32_t now) { sleeping_ = false; last_input_ = now; }
private:
    std::uint32_t last_input_;
    std::set<unsigned> held_;
    bool sleeping_ = false, swallow_ = false;
};

class DisplayIdle {
public:
    static std::filesystem::path system_config_path();
    explicit DisplayIdle(std::uint32_t now, bool enabled = true,
        std::filesystem::path config = system_config_path(),
        std::filesystem::path backlight = "/sys/class/backlight/backlight");
    ~DisplayIdle();
    DisplayIdle(const DisplayIdle&) = delete;
    DisplayIdle& operator=(const DisplayIdle&) = delete;
    bool input(unsigned code, int value, std::uint32_t now);
    void update(std::uint32_t now);
    bool sleeping() const { return policy_.sleeping(); }
private:
    void reload();
    bool restore();
    int fallback_brightness() const;
    IdlePolicy policy_;
    bool enabled_;
    std::filesystem::path config_, backlight_;
    std::uint32_t checked_at_;
    unsigned timeout_ = 30;
    int saved_brightness_ = -1, configured_brightness_ = -1;
};
} // namespace colibri_ui
