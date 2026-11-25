#include "display_idle.hpp"
#include <nlohmann/json.hpp>
#include <algorithm>
#include <charconv>
#include <cstdlib>
#include <fstream>
#include <string>

namespace colibri_ui {
namespace {
int read_int(const std::filesystem::path& path, int fallback) {
    int value;
    return std::ifstream(path) >> value ? value : fallback;
}
bool write_int(const std::filesystem::path& path, int value) {
    std::ofstream file(path);
    file << value << '\n';
    file.flush();
    return file.good();
}
int integer(const nlohmann::json& object, const char* key, int fallback) {
    const auto it = object.find(key);
    if (it == object.end()) return fallback;
    if (it->is_number_integer()) {
        const auto number = it->get<std::int64_t>();
        return number >= 0 && number <= 86400 ? int(number) : fallback;
    }
    if (it->is_string()) {
        const auto& text = it->get_ref<const std::string&>();
        int number;
        const auto parsed = std::from_chars(text.data(), text.data() + text.size(), number);
        if (parsed.ec == std::errc{} && parsed.ptr == text.data() + text.size() && number >= 0 && number <= 86400)
            return number;
    }
    return fallback;
}
}

std::filesystem::path DisplayIdle::system_config_path() {
    const char* home = std::getenv("HOME");
    return home ? std::filesystem::path(home) / ".config/cardputerzero/config.json" : std::filesystem::path{};
}
DisplayIdle::DisplayIdle(std::uint32_t now, bool enabled, std::filesystem::path config,
                         std::filesystem::path backlight)
    : policy_(now), enabled_(enabled), config_(std::move(config)),
      backlight_(std::move(backlight)), checked_at_(now) {
    if (!enabled_) return;
    reload();
    if (read_int(backlight_ / "brightness", -1) == 0)
        write_int(backlight_ / "brightness", fallback_brightness());
}
DisplayIdle::~DisplayIdle() { if (enabled_) restore(); }
void DisplayIdle::reload() {
    std::ifstream file(config_, std::ios::binary);
    std::string bytes(65537, '\0');
    file.read(bytes.data(), bytes.size());
    bytes.resize(static_cast<std::size_t>(file.gcount()));
    const auto config = bytes.size() <= 65536 ? nlohmann::json::parse(bytes, nullptr, false) : nlohmann::json();
    timeout_ = unsigned(integer(config, "dark_time", 30));
    configured_brightness_ = integer(config, "brightness", -1);
}
int DisplayIdle::fallback_brightness() const {
    const int maximum = std::max(1, read_int(backlight_ / "max_brightness", 100));
    return configured_brightness_ > 0 ? std::min(maximum, configured_brightness_) : maximum;
}
bool DisplayIdle::restore() {
    if (saved_brightness_ < 0) return true;
    if (!write_int(backlight_ / "brightness", saved_brightness_)) return false;
    saved_brightness_ = -1;
    return true;
}
bool DisplayIdle::input(unsigned code, int value, std::uint32_t now) {
    const bool was_sleeping = policy_.sleeping();
    const bool consumed = policy_.input(code, value, now);
    if (was_sleeping && !policy_.sleeping() && !restore()) policy_.sleep();
    return consumed;
}
void DisplayIdle::update(std::uint32_t now) {
    if (!enabled_) return;
    if (std::uint32_t(now - checked_at_) >= 5000) {
        checked_at_ = now;
        reload();
        if (!timeout_ && policy_.sleeping() && restore()) policy_.wake(now);
    }
    if (!policy_.due(now, timeout_)) return;
    const int current = read_int(backlight_ / "brightness", -1);
    if (current < 0 || !write_int(backlight_ / "brightness", 0)) return;
    saved_brightness_ = current > 0 ? current : fallback_brightness();
    policy_.sleep();
}
} // namespace colibri_ui
