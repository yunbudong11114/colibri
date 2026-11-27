#include "input.hpp"
#include "qr_view.hpp"
#include "markdown.hpp"
#include "worker_link.hpp"
#include "menu.hpp"
#include "composer_activity.hpp"
#include "display_idle.hpp"
#include <lvgl.h>
#if !COLIBRI_DESKTOP && defined(__linux__)
#include <ft2build.h>
#include FT_FREETYPE_H
#endif
#if COLIBRI_DESKTOP
#include "src/drivers/sdl/lv_sdl_window.h"
#include <SDL.h>
#else
#include "src/drivers/display/fb/lv_linux_fbdev.h"
#endif
#include <chrono>
#include <algorithm>
#include <array>
#include <cstdio>
#include <deque>
#include <cstdlib>
#include <filesystem>
#include <fstream>
#include <sstream>
#include <ctime>
#include <optional>
#include <thread>
#include <string_view>
#include <unistd.h>

#if !COLIBRI_DESKTOP && defined(__linux__)
// LVGL opens TTC files at face zero. Debian's Noto Sans CJK collection stores SC at face two.
extern "C" FT_Error __real_FT_New_Face(FT_Library, const char*, FT_Long, FT_Face*);
extern "C" FT_Error __wrap_FT_New_Face(FT_Library library, const char* path,
                                      FT_Long index, FT_Face* face) {
    if (index == 0 && path &&
        std::string_view(path) == "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc") {
        if (__real_FT_New_Face(library, path, 2, face) == 0) return 0;
    }
    return __real_FT_New_Face(library, path, index, face);
}
#endif

using nlohmann::json;
using namespace colibri_ui;

namespace {
constexpr int Width = 320;
constexpr int Height = 170;
constexpr int PageSize = 6;
constexpr int ChatWidth = 290;
constexpr int CodePadding = 2;
constexpr int CodeBorder = 1;
constexpr std::size_t MaxSpelling = 64;
enum class Screen { Task, Chat, Menu, HelpMenu, Help, Settings, Fields, ModelMode, Providers, ModelForm, Edit, MemoryFiles, MemoryTopics, MemoryView, CatalogMenu, CatalogList, CatalogItem };

const std::vector<std::string> MemoryFiles{"SOUL.md", "USER.md", "MEMORY.md", "INDEX.md"};
const std::vector<std::string> ModelModes{"服务商预设", "自定义连接"};

struct ProviderPreset {
    const char* name;
    const char* url;
    const char* model;
};

constexpr ProviderPreset Presets[]{
    {"OpenAI", "https://api.openai.com/v1", "gpt-4.1-mini"},
    {"DeepSeek", "https://api.deepseek.com", "deepseek-flash"},
    {"OpenRouter", "https://openrouter.ai/api/v1", "openrouter/auto"},
    {"Ollama", "http://127.0.0.1:11434/v1", ""},
};

bool is_list(Screen screen) {
    return screen == Screen::Menu || screen == Screen::HelpMenu || screen == Screen::Settings || screen == Screen::Fields ||
           screen == Screen::ModelMode || screen == Screen::Providers || screen == Screen::ModelForm ||
           screen == Screen::MemoryFiles || screen == Screen::MemoryTopics ||
           screen == Screen::CatalogMenu || screen == Screen::CatalogList;
}

std::size_t utf8_end(const std::string& text, std::size_t start) {
    auto end = start + 1;
    while (end < text.size() && (static_cast<unsigned char>(text[end]) & 0xc0) == 0x80) ++end;
    return end;
}

std::size_t utf8_previous(const std::string& text, std::size_t cursor) {
    if (!cursor) return 0;
    auto start = cursor - 1;
    while (start && (static_cast<unsigned char>(text[start]) & 0xc0) == 0x80) --start;
    return start;
}

void erase_last(std::string& text) {
    text.resize(utf8_previous(text, text.size()));
}

// LVGL looks up individual code points, so show regional-indicator pairs as ISO codes.
std::string display_chat_text(std::string_view text) {
    auto region = [](std::string_view character) -> char {
        if (character.size() < 4 || character.substr(0, 3) != "\xf0\x9f\x87") return 0;
        const auto byte = static_cast<unsigned char>(character[3]);
        return byte >= 0xa6 && byte <= 0xbf ? static_cast<char>('A' + byte - 0xa6) : 0;
    };
    std::string result;
    result.reserve(text.size());
    while (!text.empty()) {
        if (text.size() >= 8) {
            const char first = region(text);
            const char second = region(text.substr(4));
            if (first && second) {
                result += '[';
                result += first;
                result += second;
                result += ']';
                text.remove_prefix(8);
                continue;
            }
        }
        if (text.substr(0, 3) == "\xef\xb8\x8e" || text.substr(0, 3) == "\xef\xb8\x8f") {
            text.remove_prefix(3);
            continue;
        }
        result += text.front();
        text.remove_prefix(1);
    }
    return result;
}

const lv_font_t* load_font(lv_freetype_font_style_t style = LV_FREETYPE_FONT_STYLE_NORMAL,
                           std::uint16_t size = 14, bool medium = false) {
    const char* custom = std::getenv("COLIBRI_FONT");
    const std::vector<std::string> paths{
        custom ? custom : "",
        medium ? "/usr/share/fonts/opentype/noto/NotoSansCJK-Medium.ttc" : "",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
    };
    for (const auto& path : paths) {
        if (path.empty() || !std::filesystem::exists(path)) continue;
        if (auto* font = lv_freetype_font_create(path.c_str(), LV_FREETYPE_FONT_RENDER_MODE_BITMAP,
                                                 size, style)) {
            static const auto* emoji = lv_freetype_font_create(
                COLIBRI_EMOJI_FONT_PATH, LV_FREETYPE_FONT_RENDER_MODE_BITMAP, 12, LV_FREETYPE_FONT_STYLE_NORMAL);
            font->fallback = emoji;
            return font;
        }
    }
    return LV_FONT_DEFAULT;
}

const lv_font_t* code_font(const lv_font_t* fallback) {
    for (auto path : {"/usr/share/fonts/truetype/dejavu/DejaVuSansMono.ttf",
                      "/System/Library/Fonts/Monaco.ttf"}) {
        if (!std::filesystem::exists(path)) continue;
        if (auto* font = lv_freetype_font_create(path, LV_FREETYPE_FONT_RENDER_MODE_BITMAP,
                                                 12, LV_FREETYPE_FONT_STYLE_NORMAL)) {
            font->fallback = fallback;
            return font;
        }
    }
    return fallback;
}

class View {
public:
    explicit View(WorkerLink& worker) : worker_(worker) {
        auto* root = lv_screen_active();
        lv_obj_remove_style_all(root);
        const auto* normal = load_font(LV_FREETYPE_FONT_STYLE_NORMAL, 13);
        lv_obj_set_style_text_font(root, normal, 0);
        bold_ = load_font(LV_FREETYPE_FONT_STYLE_BOLD, 13);
        italic_ = load_font(LV_FREETYPE_FONT_STYLE_ITALIC, 13);
        heading_ = bold_;
        chat_font_ = load_font(LV_FREETYPE_FONT_STYLE_NORMAL, 13);
        sender_font_ = load_font(LV_FREETYPE_FONT_STYLE_BOLD, 12);
        code_ = code_font(normal);

        const auto* chrome_font = load_font(LV_FREETYPE_FONT_STYLE_NORMAL, 12);
        // Alpha mask sampled from the launcher hummingbird.
        static const uint8_t pixels[] = {
            0, 18, 7, 12, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 6, 15, 18, 0,
            9, 14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 7, 9,
            29, 0, 0, 0, 0, 0, 0, 0, 7, 36, 6, 0, 0, 0, 0, 0, 0, 15,
            0, 0, 0, 0, 0, 0, 0, 0, 51, 255, 142, 9, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 22, 175, 255, 73, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 182, 240, 85, 182, 158, 73, 0, 0,
            0, 0, 0, 0, 0, 7, 43, 170, 145, 119, 255, 255, 220, 238, 107, 85, 0, 0,
            0, 0, 0, 37, 95, 194, 255, 255, 255, 255, 255, 255, 231, 44, 0, 0, 0, 0,
            0, 0, 0, 43, 85, 85, 162, 227, 255, 255, 241, 111, 36, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 6, 124, 206, 219, 117, 49, 0, 0, 0, 0, 0, 0, 0,
            0, 0, 0, 0, 0, 122, 182, 73, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            9, 0, 0, 0, 0, 21, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
            22, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 22,
            9, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 9,
            0, 12, 15, 18, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 12, 0, 6, 0,
        };
        static lv_image_dsc_t bird{};
        bird.header.magic = LV_IMAGE_HEADER_MAGIC;
        bird.header.cf = LV_COLOR_FORMAT_A8;
        bird.header.w = bird.header.stride = 18;
        bird.header.h = 15;
        bird.data_size = sizeof(pixels);
        bird.data = pixels;
        bird_ = lv_image_create(root);
        lv_image_set_src(bird_, &bird);
        lv_image_set_pivot(bird_, 0, 0);
        lv_image_set_scale(bird_, 341);
        lv_obj_set_pos(bird_, 10, 3);
        const auto* title_font = load_font(LV_FREETYPE_FONT_STYLE_BOLD, 16);
        title_ = label(root, 39, std::max(0, (26 - static_cast<int>(title_font->line_height)) / 2),
                       159, LV_LABEL_LONG_DOT);
        lv_obj_set_style_text_font(title_, title_font, 0);
        for (size_t i = 0; i < signal_bars_.size(); ++i) {
            auto* bar = signal_bars_[i] = lv_obj_create(root);
            lv_obj_remove_style_all(bar);
            const int height = 3 + 3 * i;
            lv_obj_set_size(bar, 3, height);
            lv_obj_set_pos(bar, 205 + 5 * i, 18 - height);
            lv_obj_set_style_bg_opa(bar, LV_OPA_COVER, 0);
            lv_obj_set_style_radius(bar, 1, 0);
        }
        clock_label_ = label(root, 226, 4, 42);
        battery_label_ = label(root, 272, 4, 41);
        for (auto* badge : {clock_label_, battery_label_}) {
            lv_obj_set_style_text_align(badge, LV_TEXT_ALIGN_CENTER, 0);
            lv_obj_set_style_bg_opa(badge, LV_OPA_COVER, 0);
            lv_obj_set_style_text_color(badge, lv_color_hex(0xffffff), 0);
            lv_obj_set_style_radius(badge, 2, 0);
        }
        update_device_status();
        panel_ = lv_obj_create(root);
        lv_obj_remove_style_all(panel_);
        lv_obj_set_pos(panel_, 7, 26);
        lv_obj_set_size(panel_, 306, 96);
        lv_obj_set_style_radius(panel_, 7, 0);
        lv_obj_set_scroll_dir(panel_, LV_DIR_VER);
        lv_obj_remove_flag(panel_, LV_OBJ_FLAG_SCROLL_ELASTIC);
        lv_obj_set_scrollbar_mode(panel_, LV_SCROLLBAR_MODE_AUTO);
        body_ = label(panel_, 8, 4, ChatWidth, LV_LABEL_LONG_WRAP);
        lv_obj_set_style_text_line_space(body_, 2, 0);
        chat_thread_ = column(panel_, 8, 4, 8);
        chat_hint_ = label(chat_thread_, 0, 0, ChatWidth, LV_LABEL_LONG_WRAP);
        lv_obj_add_flag(chat_hint_, LV_OBJ_FLAG_HIDDEN);
        activity_message_ = column(chat_thread_, 0, 0, 0);
        auto* activity_sender = label(activity_message_, 0, 0, LV_SIZE_CONTENT);
        lv_label_set_text(activity_sender, "Colibri");
        lv_obj_set_style_text_font(activity_sender, sender_font_, 0);
        lv_obj_set_style_text_color(activity_sender, lv_color_hex(0xa3efbe), 0);
        auto* activity_line = lv_obj_create(activity_message_);
        lv_obj_remove_style_all(activity_line);
        lv_obj_remove_flag(activity_line, LV_OBJ_FLAG_SCROLLABLE);
        lv_obj_set_size(activity_line, ChatWidth, chat_font_->line_height + 2);
        activity_label_ = label(activity_line, 0, 0, ChatWidth - 25);
        lv_obj_set_style_text_font(activity_label_, chat_font_, 0);
        lv_obj_set_style_text_color(activity_label_, lv_color_hex(0x9cb6ad), 0);
        for (auto& dot : activity_dots_) {
            dot = lv_obj_create(activity_line);
            lv_obj_remove_style_all(dot);
            lv_obj_add_flag(dot, LV_OBJ_FLAG_IGNORE_LAYOUT);
            lv_obj_set_size(dot, 3, 3);
            lv_obj_set_style_radius(dot, 2, 0);
            lv_obj_set_style_bg_color(dot, lv_color_hex(0x91cdb4), 0);
            lv_obj_set_style_bg_opa(dot, LV_OPA_COVER, 0);
        }
        lv_obj_add_flag(activity_message_, LV_OBJ_FLAG_HIDDEN);
        for (std::size_t i = 0; i < menu_rows_.size(); ++i) {
            auto* row = label(panel_, 8, 3 + 22 * i, ChatWidth, LV_LABEL_LONG_DOT);
            lv_obj_set_height(row, 21);
            lv_obj_set_style_pad_hor(row, 7, 0);
            lv_obj_set_style_pad_top(row, 2, 0);
            lv_obj_set_style_radius(row, 4, 0);
            lv_obj_set_style_border_side(row, LV_BORDER_SIDE_LEFT, 0);
            lv_obj_set_style_border_width(row, 2, 0);
            menu_rows_[i] = row;
        }
        constexpr int composer_y = 125;
        constexpr int composer_height = 24;
        const int composer_padding = std::max(0, (composer_height - static_cast<int>(normal->line_height)) / 2);
        composer_ = label(root, 7, composer_y, 306);
        lv_obj_set_height(composer_, composer_height);
        lv_obj_set_style_pad_left(composer_, 8, 0);
        lv_obj_set_style_pad_top(composer_, composer_padding, 0);
        lv_obj_set_style_radius(composer_, 6, 0);
        for (size_t i = 0; i < voice_bars_.size(); ++i) {
            auto* bar = voice_bars_[i] = lv_obj_create(root);
            lv_obj_remove_style_all(bar);
            lv_obj_set_x(bar, 84 + 5 * i);
            lv_obj_set_style_radius(bar, 2, 0);
            lv_obj_set_style_bg_opa(bar, LV_OPA_COVER, 0);
            lv_obj_add_flag(bar, LV_OBJ_FLAG_HIDDEN);
        }
        status_ = label(root, 268, composer_y + composer_padding, 37);
        lv_obj_set_style_text_align(status_, LV_TEXT_ALIGN_RIGHT, 0);
        voice_time_ = label(root, 220, composer_y + composer_padding, 85);
        lv_obj_set_style_text_align(voice_time_, LV_TEXT_ALIGN_RIGHT, 0);
        candidates_ = label(root, 15, 152, 290);
        lv_obj_set_style_text_font(candidates_, chrome_font, 0);
        footer_ = lv_obj_create(root);
        lv_obj_remove_style_all(footer_);
        lv_obj_remove_flag(footer_, LV_OBJ_FLAG_SCROLLABLE);
        lv_obj_set_pos(footer_, 7, 150);
        lv_obj_set_size(footer_, 306, 20);
        lv_obj_set_style_text_font(footer_, load_font(LV_FREETYPE_FONT_STYLE_NORMAL, 11), 0);
        lv_obj_set_flex_flow(footer_, LV_FLEX_FLOW_ROW);
        lv_obj_set_flex_align(footer_, LV_FLEX_ALIGN_START, LV_FLEX_ALIGN_CENTER, LV_FLEX_ALIGN_CENTER);
        lv_obj_set_style_pad_column(footer_, 4, 0);
        for (size_t i = 0; i < footer_parts_.size(); ++i) {
            auto* part = footer_parts_[i] = label(footer_, 0, 0, LV_SIZE_CONTENT);
            if (i % 2 == 0) {
                lv_obj_set_style_radius(part, 3, 0);
                lv_obj_set_style_pad_hor(part, 4, 0);
                lv_obj_set_style_bg_opa(part, LV_OPA_COVER, 0);
            } else lv_obj_set_style_pad_right(part, 4, 0);
        }
        send({{"type", "model_status"}}, "检查配置");
        starting_ = busy_;
        refresh();
    }

    void update_device_status() {
        const auto now = std::time(nullptr);
        if (now == status_second_) return;
        const bool read_device = status_second_ == 0 || now / 5 != status_second_ / 5;
        status_second_ = now;
        char clock_text[8] = "--:--";
        std::tm local{};
        if (localtime_r(&now, &local)) std::strftime(clock_text, sizeof(clock_text), "%H:%M", &local);
        lv_label_set_text(clock_label_, clock_text);
        if (!read_device) return;
        int charge;
        if (std::ifstream("/sys/class/power_supply/bq27220-0/capacity") >> charge)
            lv_label_set_text_fmt(battery_label_, "%d%%", charge);
        else lv_label_set_text(battery_label_, "--%");
        std::ifstream wireless("/proc/net/wireless");
        int level = 0;
        std::string line;
        while (std::getline(wireless, line)) {
            int quality;
            if (std::sscanf(line.c_str(), " wlan0: %*x %d", &quality) == 1)
                level = std::clamp((quality + 17) / 18, 0, 4);
        }
        signal_level_ = level;
        color_signal();


    }

    bool running = true;
    bool restoring_history() const { return starting_; }
    void request_exit() {
        if (closing_) return;
        closing_ = true;
        if (agent_busy_) set_agent_activity("正在停止");
        try { worker_.send({{"type", "shutdown"}}); }
        catch (const std::exception&) { running = false; }
    }
    void set_agent_activity(const std::string& text) {
        agent_busy_ = true;
        agent_status_ = text;
        lv_label_set_text(activity_label_, text.c_str());
        lv_obj_remove_flag(activity_message_, LV_OBJ_FLAG_HIDDEN);
        lv_obj_move_to_index(activity_message_, -1);
    }


    ComposerActivity activity() const {
        return composer_activity(screen_ == Screen::Chat, transcribing_, recording_, transcribing_ ? "正在转写" : "");
    }

    void update_composer_animation() {
        if (agent_busy_ && screen_ == Screen::Chat) {
            lv_point_t measured{};
            lv_text_get_size(&measured, agent_status_.c_str(), chat_font_, 0, 0, LV_COORD_MAX, LV_TEXT_FLAG_NONE);
            const int start = std::min<int>(ChatWidth - 30, measured.x + 12);
            for (unsigned i=0; i<3; ++i) {
                const auto dot=activity_dot(ComposerActivity::Thinking,lv_tick_get(),i);
                lv_obj_set_pos(activity_dots_[i],start+5*i,static_cast<int>(chat_font_->line_height)/2-1+dot.offset_y);
            }
        }
        const auto state = activity();
        if (has_activity_dots(state)) {
            lv_point_t measured{};
            const auto* font = lv_obj_get_style_text_font(composer_, LV_PART_MAIN);
            const auto caption = status_text_ + "一";
            lv_text_get_size(&measured, caption.c_str(), font, 0, 0, LV_COORD_MAX, LV_TEXT_FLAG_NONE);
            const int start = lv_obj_get_x(composer_) +
                lv_obj_get_style_pad_left(composer_, LV_PART_MAIN) + measured.x;
            const int center = lv_obj_get_y(composer_) + lv_obj_get_height(composer_) / 2;
            for (unsigned i = 0; i < 3; ++i) {
                const auto dot = activity_dot(state, lv_tick_get(), i);
                lv_obj_set_pos(voice_bars_[i], start + 5 * i, center - 1 + dot.offset_y);
                lv_obj_set_size(voice_bars_[i], 3, 3);
                lv_obj_set_style_bg_opa(voice_bars_[i], dot.opacity, 0);
            }
            voice_phase_ = 99;
            return;
        }
        if (!recording_ || screen_ != Screen::Chat) return;
        const unsigned phase = (lv_tick_elaps(recording_start_) / 110) % 8;
        if (phase != voice_phase_) {
            constexpr int heights[] = {4, 6, 10, 14, 12, 8, 6, 4};
            for (size_t i = 0; i < voice_bars_.size(); ++i) {
                const int h = heights[(phase + 3 * i) % 8];
                lv_obj_set_x(voice_bars_[i], 84 + 5 * i);
                lv_obj_set_size(voice_bars_[i], 3, h);
                lv_obj_set_style_bg_opa(voice_bars_[i], LV_OPA_COVER, 0);
                lv_obj_set_y(voice_bars_[i], 137 - h / 2);
            }
            voice_phase_ = phase;
        }
        const unsigned seconds = std::min<unsigned>(60, lv_tick_elaps(recording_start_) / 1000);
        if (seconds == recording_seconds_) return;
        recording_seconds_ = seconds;
        lv_label_set_text(voice_time_, recording_caption().c_str());
    }

    void receive(const json& event) {
        const auto type = event.value("type", "");
        const auto text = event.value("text", "");
        if (type == "pinyin") {
            if (event.value("spelling", "") == spelling_) {
                choices_.clear();
                if (event.contains("candidates")) {
                    for (const auto& candidate : event.at("candidates")) {
                        auto consumed = candidate.value("consumed", std::size_t{0});
                        auto label = candidate.value("text", "");
                        if (!label.empty() && consumed > 0 && consumed <= spelling_.size())
                            choices_.push_back({std::move(label), consumed});
                    }
                } else {
                    for (std::size_t i = 0; i < text.size();) {
                        const auto end = utf8_end(text, i);
                        choices_.push_back({text.substr(i, end - i), spelling_.size()});
                        i = end;
                    }
                }
                paginate_choices();
                pinyin_pending_ = false;
                if (pending_choice_) {
                    auto slot = *pending_choice_;
                    pending_choice_.reset();
                    choose(slot);
                }
            }
            refresh();
            return;
        }
        if (type == "history_page") {
            if (event.value("request_id", std::uint64_t(0)) != history_request_) return;
            history_loading_ = false;
            if (!history_pending_.empty()) {
                auto next = history_pending_;
                history_pending_.clear();
                request_history(next);
                return;
            }
            if (event.contains("error")) { append("系统", "历史加载失败，请重试"); refresh(); return; }
            std::string anchor;
            int anchor_y = 0;
            if (!chat_.empty()) lv_obj_update_layout(panel_);
            const auto incoming = event.value("history", json::array());
            for (const auto& item : chat_) {
                if (!item.history_id.empty() && std::any_of(incoming.begin(), incoming.end(), [&](const json& m) {
                    return m.value("id", "") == item.history_id;
                })) { anchor = item.history_id; anchor_y = lv_obj_get_y(item.block); break; }
            }
            const int old_scroll = lv_obj_get_scroll_y(panel_);
            for (const auto& item : chat_) lv_obj_delete(item.block);
            chat_.clear(); chat_bytes_ = 0;
            history_installing_ = true;
            for (const auto& m : incoming) {
                append(m.value("role", "") == "user" ? "我" : "Colibri", m.value("text", ""));
                chat_.back().history_id = m.value("id", "");
            }
            if (history_direction_ == "last" && agent_busy_ && !active_question_.empty() &&
                std::none_of(incoming.begin(), incoming.end(), [&](const json& m) {
                    return m.value("role", "") == "user" && m.value("text", "") == active_question_;
                })) append("我", active_question_);
            if (agent_busy_) lv_obj_move_to_index(activity_message_, -1);
            history_installing_ = false;
            lv_obj_move_to_index(chat_hint_, -1);
            if (agent_busy_) lv_obj_move_to_index(activity_message_, -1);
            history_start_ = event.value("start", std::size_t(0));
            history_before_ = event.value("has_before", false);
            history_after_ = event.value("has_after", false);
            refresh();
            lv_obj_update_layout(panel_);
            if (history_direction_ == "last") {
                lv_obj_scroll_by_bounded(panel_, 0, -lv_obj_get_scroll_bottom(panel_), LV_ANIM_OFF);
            } else if (history_direction_ == "first") lv_obj_scroll_to_y(panel_, 0, LV_ANIM_OFF);
            else {
                bool restored = false;
                for (const auto& item : chat_) if (item.history_id == anchor && !anchor.empty()) {
                    lv_obj_scroll_to_y(panel_, old_scroll + lv_obj_get_y(item.block) - anchor_y, LV_ANIM_OFF);
                    restored = true; break;
                }
                if (!restored) lv_obj_scroll_to_y(panel_, 0, LV_ANIM_OFF);
            }
            return;
        }
        if (type == "shutdown") { running = false; return; }
        if (type == "activity" || type == "steering_received" || type == "steering_applied") {
            set_agent_activity(type == "steering_applied" ? "已接收补充指令" : text);
            refresh();
            return;
        }
        if (type == "status") {
            append("系统", text);
            refresh(true);
            return;
        }
        busy_ = false;
        status_text_ = "";
        if (type == "recording" && !recording_) {
            recording_start_ = lv_tick_get();
            recording_seconds_ = 0;
        }
        if (type == "recording") recording_ = true;
        else if (type == "draft" || type == "recording_cancelled") recording_ = false;

        if (type == "task_status") {
            task_text_ = text;
            screen_ = Screen::Task;
        } else if (type == "model_status") {
            starting_ = false;
            model_hint_ = text;
            history_direction_ = "last";
            auto page = event;
            page["type"] = "history_page";
            page["request_id"] = history_request_;
            receive(page);
            return;
        } else if (type == "settings") {
            fields_ = event.at("fields");
            model_hint_ = event.value("model_hint", "");
            refresh_group();
        } else if (type == "saved") {
            screen_ = edit_parent_;
            send({{"type", "settings"}}, "读取设置");
        } else if (type == "catalog_items") {
            catalog_items_ = event.at("items");
            selection_ = 0;
        } else if (type == "memory_topics") {
            memory_topics_ = event.at("files").get<std::vector<std::string>>();
            selection_ = 0;
        } else if (type == "memory_file") {
            memory_file_ = event.at("file").get<std::string>();
            memory_text_ = text;
            screen_ = Screen::MemoryView;
            error_.clear();
        } else if (type == "model_saved") {
            fields_ = event.at("fields");
            model_hint_.clear();
            model_url_ = saved_model("model.base_url");
            model_id_ = saved_model("model.model");
            model_key_ = nullptr;
            error_ = "已保存";
        } else if (type == "draft") {
            transcribing_ = false;
            if (!insert_draft(text)) append("系统", "草稿已满，语音未插入");
        } else if (type == "chat_cleared") {
            ++history_request_; history_loading_ = history_before_ = history_after_ = false; history_start_ = 0; history_pending_.clear();
            for (const auto& message : chat_) lv_obj_delete(message.block);
            chat_.clear();
            chat_bytes_ = 0;
            refresh();
            lv_obj_update_layout(panel_);
            lv_obj_scroll_to_y(panel_, 0, LV_ANIM_OFF);
            return;
        } else if (type == "reply") {
            agent_busy_ = false;
            lv_obj_add_flag(activity_message_, LV_OBJ_FLAG_HIDDEN);
            receive_chat_reply(text);
            return;
        } else if (type == "error") {
            if (event.value("voice", false)) transcribing_ = false;
            if (event.value("agent", false)) {agent_busy_=false;lv_obj_add_flag(activity_message_,LV_OBJ_FLAG_HIDDEN);}
            starting_ = false;
            if (screen_ == Screen::Edit || screen_ == Screen::ModelForm ||
                screen_ == Screen::MemoryFiles || screen_ == Screen::MemoryTopics || screen_ == Screen::MemoryView || screen_ == Screen::CatalogList || (screen_ == Screen::Fields && category_ == MemoryCategory))
                error_ = text;
            else append("系统", text);
        }
        refresh(true);
    }

    void key(Key key) {
        if (key == Key::ChatTop) {
            if (screen_ == Screen::Chat) request_history("first");
            return;
        }
        if (key == Key::ChatBottom) {
            if (screen_ == Screen::Chat) request_history("last");
            return;
        }
        if (closing_) return;
        if (recording_) {
            if (key == Key::Voice || key == Key::Enter || key == Key::Escape) {
                recording_ = false;
                transcribing_ = key != Key::Escape;
                send({{"type", key == Key::Escape ? "voice_cancel" : "voice_stop"}},
                     key == Key::Escape ? "取消录音" : "正在转写");
                refresh();
            }
            return;
        }
        if (key == Key::Voice && screen_ == Screen::Chat) {
            clear_pinyin();
            send({{"type", "voice_start"}}, "启动录音");
            refresh();
            return;
        }
        if (screen_ == Screen::Help) {
            if (key == Key::Left || key == Key::Escape) {
                screen_ = Screen::HelpMenu;
                selection_ = help_kind_;
            } else if (key == Key::Up || key == Key::Down) scroll(key);
            refresh();
            return;
        }
        if (screen_ == Screen::Task) {
            if (key == Key::Left || key == Key::Escape) { screen_ = Screen::Menu; selection_ = TaskMenuIndex; }
            else if (key == Key::Enter || key == Key::Right) send({{"type", "task_status"}}, "读取任务");
            else if (key == Key::Up || key == Key::Down) scroll(key);
            refresh(); return;
        }
        if (screen_ == Screen::CatalogItem) {
            if (key == Key::Left || key == Key::Escape) screen_ = Screen::CatalogList;
            else if (key == Key::Up || key == Key::Down) scroll(key);
            refresh();
            return;
        }
        if (screen_ == Screen::MemoryView) {
            memory_view_key(key);
            refresh();
            return;
        }
        if (is_list(screen_)) {
            navigate(key);
            return;
        }
        if (key == Key::Escape) {
            if (!spelling_.empty()) clear_pinyin();
            else {
                screen_ = screen_ == Screen::Edit ? edit_parent_ : Screen::Menu;
                if (screen_ == Screen::Menu) selection_ = 0;
            }
        } else if (key == Key::Tab && screen_ == Screen::Chat) {
            chinese_ = !chinese_;
            clear_pinyin();
        } else if (key == Key::Tab && screen_ == Screen::Edit) {
            edited_.clear();
        } else if (key == Key::Backspace) {
            if (!spelling_.empty()) {
                spelling_.pop_back();
                query_pinyin();
            } else if (screen_ == Screen::Edit) erase_last(edited_);
            else if (draft_cursor_ > 0) {
                const auto previous = utf8_previous(draft_, draft_cursor_);
                draft_.erase(previous, draft_cursor_ - previous);
                draft_cursor_ = previous;
            }
        } else if (key == Key::Enter) {
            if (!spelling_.empty()) choose(0);
            else if (screen_ == Screen::Edit) {
                error_.clear();
                if (edit_parent_ == Screen::ModelForm) {
                    if (selection_ == 0) model_url_ = edited_;
                    else if (selection_ == 1) model_key_ = edited_;
                    else model_id_ = edited_;
                    screen_ = edit_parent_;
                } else {
                    send({{"type", "save"}, {"key", edit_field_.at("key")}, {"value", edited_}}, "保存");
                }
            } else if (screen_ == Screen::Chat && !model_hint_.empty() && draft_ != ClearCommand) {
                screen_ = Screen::ModelMode;
                category_ = 0;
                selection_ = 0;
                send({{"type", "settings"}}, "读取配置");
                refresh();
                return;
            } else if (draft_ == ClearCommand) {
                draft_.clear();
                draft_cursor_ = draft_view_start_ = 0;
                ++history_request_; history_loading_ = false; history_pending_.clear();
                send({{"type", "clear_chat"}}, "清理屏幕");
                refresh();
                return;
            } else if (draft_.find_first_not_of(" \t\n") != std::string::npos) {
                ++history_request_; history_loading_ = false; history_pending_.clear();
                if (history_after_) {
                    for (const auto& item : chat_) lv_obj_delete(item.block);
                    chat_.clear(); chat_bytes_ = 0;
                    history_after_ = false;
                    request_history("last");
                }
                active_question_ = draft_;
                append("我", draft_);
                const bool stop = draft_ == "/stop";
                send({{"type", "submit"}, {"text", draft_}}, "");
                set_agent_activity(stop ? "正在停止" : (agent_busy_ ? "已收到补充，等待处理" : "思考中"));
                draft_.clear();
                draft_cursor_ = draft_view_start_ = 0;
                refresh(true);
                return;
            }
        } else if (screen_ == Screen::Chat && (key == Key::DraftStart || key == Key::DraftEnd)) {
            draft_cursor_ = key == Key::DraftStart ? 0 : draft_.size();
        } else if (key == Key::Left && page_ > 0) {
            --page_;
        } else if (key == Key::Right && page_ + 1 < candidate_pages_.size()) {
            ++page_;
        } else if (screen_ == Screen::Chat && spelling_.empty()) {
            if (key == Key::Left) draft_cursor_ = utf8_previous(draft_, draft_cursor_);
            else if (key == Key::Right && draft_cursor_ < draft_.size()) draft_cursor_ = utf8_end(draft_, draft_cursor_);
            else if (key == Key::Up || key == Key::Down) scroll(key);
        } else if (key == Key::Up || key == Key::Down) {
            scroll(key);
        }
        refresh();
    }

    void text(const std::string& text) {
        if (is_list(screen_) || screen_ == Screen::Help || screen_ == Screen::MemoryView || screen_ == Screen::CatalogItem || screen_ == Screen::Task) {
            if (text == "f" || text == "F") key(Key::Up);
            else if (text == "x" || text == "X") key(Key::Down);
            else if (text == "z" || text == "Z") key(Key::Left);
            else if (text == "c" || text == "C") key(Key::Right);
            return;
        }
        if (recording_ || closing_) return;
        if (screen_ == Screen::Edit) {
            if (edited_.size() + text.size() <= 4096) edited_ += text;
        } else if (screen_ == Screen::Chat && spelling_.empty() && text.size() == 1 &&
                   !draft_.empty() && draft_.front() == '/') {
            insert_draft(text);
        } else if (chinese_ && text.size() == 1 &&
                   ((text[0] >= 'a' && text[0] <= 'z') ||
                    (text == "'" && !spelling_.empty() && spelling_.back() != '\''))) {
            if (spelling_.size() >= MaxSpelling) return;
            spelling_ += text;
            query_pinyin();
        } else if (!spelling_.empty() && text == " ") {
            choose(0);
        } else if (!spelling_.empty() && text.size() == 1 && text[0] >= '1' && text[0] <= '6') {
            choose(text[0] - '1');
        } else if (spelling_.empty()) {
            insert_draft(text);
        }
        refresh();
    }

private:
    static constexpr std::string_view ClearCommand = "/clear";
    WorkerLink& worker_;
    Screen screen_ = Screen::Chat, edit_parent_ = Screen::Fields, model_parent_ = Screen::ModelMode;
    lv_obj_t *title_, *status_, *panel_, *body_, *composer_, *candidates_;
    lv_obj_t *chat_thread_, *chat_hint_;
    lv_obj_t *activity_message_ = nullptr, *activity_label_ = nullptr;
    std::array<lv_obj_t*,3> activity_dots_{};
    bool agent_busy_ = false, closing_ = false, transcribing_ = false;
    std::string agent_status_;
    std::uint64_t history_request_ = 0;
    std::size_t history_start_ = 0;
    bool history_loading_ = false, history_before_ = false, history_after_ = false;
    std::string history_direction_, history_pending_, active_question_;
    std::string task_text_;
    std::array<lv_obj_t*, 6> menu_rows_{};
    const lv_font_t *bold_, *italic_, *heading_, *code_;
    enum class Speaker { User, Agent, System };
    struct ChatMessage {
        lv_obj_t* block;
        lv_obj_t* tag;
        lv_obj_t* body;
        Speaker speaker;
        std::size_t bytes;
        std::string history_id;
    };
    std::deque<ChatMessage> chat_;
    bool history_installing_ = false;
    std::size_t chat_bytes_ = 0;
    Screen memory_parent_ = Screen::Fields;
    std::size_t memory_selection_ = 0;
    std::string memory_file_, memory_text_;
    std::vector<std::string> memory_topics_;
    json catalog_items_ = json::array();
    std::size_t catalog_kind_ = 0;
    std::array<lv_obj_t*, 5> voice_bars_{};
    const lv_font_t *chat_font_ = nullptr, *sender_font_ = nullptr;
    lv_obj_t* voice_time_ = nullptr;
    std::array<lv_obj_t*, 4> signal_bars_{};
    lv_obj_t* clock_label_ = nullptr;
    int signal_level_ = 0;

    void color_signal() {
        const unsigned on = 0xb6c9c4;
        const unsigned off = 0x40514e;
        for (int i = 0; i < 4; ++i)
            lv_obj_set_style_bg_color(signal_bars_[i], lv_color_hex(i < signal_level_ ? on : off), 0);
    }

    lv_obj_t *battery_label_ = nullptr;
    std::time_t status_second_ = 0;
    lv_obj_t *bird_ = nullptr, *footer_ = nullptr;
    std::array<lv_obj_t*, 6> footer_parts_{};
    unsigned voice_phase_ = 11;
    uint32_t recording_start_ = 0;
    unsigned recording_seconds_ = 0;

    std::string recording_caption() const {
        char time[32];
        std::snprintf(time, sizeof(time), "%u:%02u / 1:00", recording_seconds_ / 60, recording_seconds_ % 60);
        return time;
    }

    std::size_t help_kind_ = 0;
    bool busy_ = false, recording_ = false, chinese_ = true;
    bool starting_ = false;
    std::string draft_, edited_, error_, status_text_, spelling_, model_hint_;
    std::size_t draft_cursor_ = 0, draft_view_start_ = 0;
    json fields_ = json::array(), group_fields_ = json::array(), edit_field_, model_key_;
    std::string model_url_, model_id_, model_title_;
    std::size_t category_ = 0, preset_ = 0;
    struct Candidate {
        std::string text;
        std::size_t consumed;
    };
    struct CandidatePage {
        std::size_t first, count;
        std::string text;
    };
    std::vector<Candidate> choices_;
    std::vector<CandidatePage> candidate_pages_;
    bool pinyin_pending_ = false;
    std::optional<std::size_t> pending_choice_;
    std::size_t selection_ = 0, page_ = 0;

    static lv_obj_t* label(lv_obj_t* parent, int x, int y, int width,
                           lv_label_long_mode_t mode = LV_LABEL_LONG_CLIP) {
        auto* item = lv_label_create(parent);
        lv_obj_set_pos(item, x, y);
        lv_obj_set_width(item, width);
        lv_label_set_long_mode(item, mode);
        return item;
    }

    void send(const json& request, const char* status) {
        try {
            worker_.send(request);
            busy_ = true;
            status_text_ = status;
        } catch (const std::exception&) {
            busy_ = false;
            if (screen_ == Screen::Edit || screen_ == Screen::ModelForm) error_ = "运行时不可用";
            else append("系统", "运行时不可用");
        }
    }

    static lv_obj_t* column(lv_obj_t* parent, int x, int y, int gap) {
        auto* item = lv_obj_create(parent);
        lv_obj_remove_style_all(item);
        lv_obj_remove_flag(item, LV_OBJ_FLAG_SCROLLABLE);
        lv_obj_set_pos(item, x, y);
        lv_obj_set_size(item, ChatWidth, LV_SIZE_CONTENT);
        lv_obj_set_flex_flow(item, LV_FLEX_FLOW_COLUMN);
        lv_obj_set_style_pad_row(item, gap, 0);
        return item;
    }

    void color_chat_message(const ChatMessage& message) const {
        unsigned text, background;
        switch (message.speaker) {
        case Speaker::User:
            text = 0xa1c9ec;
            background = 0x284358;
            break;
        case Speaker::Agent:
            text = 0xa3efbe;
            background = 0x23493f;
            break;
        case Speaker::System:
            text = 0xd2dade;
            background = 0x34434b;
            break;
        }
        lv_obj_set_style_text_color(message.tag, lv_color_hex(text), 0);
        lv_obj_set_style_bg_color(message.tag, lv_color_hex(background), 0);
        lv_obj_set_style_text_color(message.body, lv_color_hex(0xe7f4ed), 0);
        for (uint32_t i = 0; i < lv_obj_get_child_count(message.body); ++i) {
            auto* item = lv_obj_get_child(message.body, i);
            if (lv_obj_has_flag(item, LV_OBJ_FLAG_USER_3)) {
                lv_obj_set_style_bg_color(item, lv_color_hex(0x354c61), 0);
                lv_obj_set_style_border_color(item, lv_color_hex(0x89adc6), 0);
            } else if (lv_obj_has_flag(item, LV_OBJ_FLAG_USER_1)) {
                lv_obj_set_style_bg_color(item, lv_color_hex(0x213333), 0);
                lv_obj_set_style_border_color(item, lv_color_hex(0x72aa96), 0);
            } else if (lv_obj_has_flag(item, LV_OBJ_FLAG_USER_2)) {
                lv_obj_set_style_bg_color(item, lv_color_hex(0x558476), 0);
            }
        }
    }

    std::string wrap_code(const std::string& source) const {
        constexpr int width = ChatWidth - 2 * (CodePadding + CodeBorder);
        std::string result, line;
        result.reserve(source.size());
        for (std::size_t start = 0; start < source.size();) {
            auto end = utf8_end(source, start);
            if (source[start] == '\n') {
                result += line + '\n';
                line.clear();
            } else {
                auto character = source.substr(start, end - start);
                if (character == "\t") character = "    ";
                line += character;
                lv_point_t measured{};
                lv_text_get_size(&measured, line.c_str(), code_, 0, 0, LV_COORD_MAX, LV_TEXT_FLAG_NONE);
                if (measured.x > width) {
                    result.append(line, 0, line.size() - character.size());
                    result += '\n';
                    line = std::move(character);
                }
            }
            start = end;
        }
        return result + line;
    }

    void render_markdown(lv_obj_t* body, const std::string& content, bool parse) {
        const auto blocks = parse ? markdown::parse(content) : std::vector<markdown::Block>{};
        if (blocks.empty()) {
            auto* line = label(body, 0, 0, ChatWidth, LV_LABEL_LONG_WRAP);
            lv_obj_set_style_text_line_space(line, 2, 0);
            lv_label_set_text(line, content.c_str());
            return;
        }
        using markdown::BlockKind;
        std::optional<BlockKind> previous;
        for (const auto& block : blocks) {
            if (block.runs.empty() && block.kind != BlockKind::Rule) continue;
            int gap = 4;
            if (!previous) gap = 0;
            else if (block.kind == BlockKind::Heading) gap = 6;
            else if (previous == BlockKind::Heading ||
                     (previous == BlockKind::ListItem && block.kind == BlockKind::ListItem)) gap = 1;
            previous = block.kind;
            if (block.kind == BlockKind::Rule) {
                auto* rule = lv_obj_create(body);
                lv_obj_remove_style_all(rule);
                lv_obj_remove_flag(rule, LV_OBJ_FLAG_SCROLLABLE);
                lv_obj_add_flag(rule, LV_OBJ_FLAG_USER_2);
                lv_obj_set_size(rule, ChatWidth, 1);
                lv_obj_set_style_margin_top(rule, gap, 0);
                lv_obj_set_style_bg_opa(rule, LV_OPA_COVER, 0);
                continue;
            }
            if (block.kind == BlockKind::Text && std::all_of(block.runs.begin(),block.runs.end(),[](const auto& run){return run.style==0;})) {
                std::string text;
                for (const auto& run:block.runs) text+=run.text;
                auto* line=label(body,0,0,ChatWidth,LV_LABEL_LONG_WRAP);
                lv_obj_set_style_margin_top(line,gap,0);
                lv_obj_set_style_text_line_space(line,2,0);
                lv_label_set_text(line,text.c_str());
                continue;
            }
            auto* group = lv_spangroup_create(body);
            lv_obj_set_style_margin_top(group, gap, 0);
            lv_obj_set_size(group, ChatWidth, LV_SIZE_CONTENT);
            lv_obj_remove_flag(group, LV_OBJ_FLAG_SCROLLABLE);
            lv_obj_set_style_text_line_space(group, 2, 0);
            if (block.kind == BlockKind::Heading) {
                lv_obj_set_style_text_font(group, heading_, 0);
            } else if (block.kind == BlockKind::Code || block.kind == BlockKind::Quote || block.kind == BlockKind::Table) {
                lv_obj_add_flag(group, block.kind == BlockKind::Code ? LV_OBJ_FLAG_USER_3 : LV_OBJ_FLAG_USER_1);
                lv_obj_set_style_bg_opa(group, LV_OPA_COVER, 0);
                lv_obj_set_style_pad_hor(group, block.kind == BlockKind::Code ? CodePadding : 6, 0);
                lv_obj_set_style_pad_ver(group, 3, 0);
                lv_obj_set_style_radius(group, 3, 0);
                if (block.kind == BlockKind::Code) {
                    lv_obj_set_style_text_font(group, code_, 0);
                    lv_obj_set_style_border_width(group, CodeBorder, 0);
                    lv_obj_set_style_border_side(group, LV_BORDER_SIDE_FULL, 0);
                }
                if (block.kind == BlockKind::Quote) {
                    lv_obj_set_style_border_width(group, 3, 0);
                    lv_obj_set_style_border_side(group, LV_BORDER_SIDE_LEFT, 0);
                }
            }
            for (const auto& run : block.runs) {
                auto* span = lv_spangroup_new_span(group);
                if (block.kind == BlockKind::Code) lv_span_set_text(span, wrap_code(run.text).c_str());
                else lv_span_set_text(span, run.text.c_str());
                auto* style = lv_span_get_style(span);
                if (run.style & markdown::InlineCode) lv_style_set_text_font(style, code_);
                else if (block.kind != BlockKind::Heading && (run.style & markdown::Bold)) lv_style_set_text_font(style, bold_);
                else if (block.kind != BlockKind::Heading && (run.style & markdown::Italic)) lv_style_set_text_font(style, italic_);
                if (run.style & markdown::Link) lv_style_set_text_decor(style, LV_TEXT_DECOR_UNDERLINE);
                else if (run.style & markdown::Strike) lv_style_set_text_decor(style, LV_TEXT_DECOR_STRIKETHROUGH);
            }
            lv_spangroup_refresh(group);
        }
    }

    void append(const std::string& speaker, const std::string& text) {
        std::string content = display_chat_text(text);
        if (content.size() > 12000) content.resize(utf8_end(content, 11999));
        auto* block = column(chat_thread_, 0, 0, 0);
        auto* tag = label(block, 0, 0, LV_SIZE_CONTENT);
        lv_obj_set_style_text_font(tag, sender_font_, 0);
        lv_label_set_text(tag, speaker.c_str());
        lv_obj_set_style_bg_opa(tag, LV_OPA_TRANSP, 0);
        lv_obj_set_style_radius(tag, 4, 0);
        lv_obj_set_style_pad_left(tag, 0, 0);
        lv_obj_set_style_pad_right(tag, 5, 0);
        auto* body = column(block, 0, 0, 0);
        lv_obj_set_style_text_font(body, chat_font_, 0);
        render_markdown(body, content, speaker != "系统");
        const auto role = speaker == "我" ? Speaker::User :
                          speaker == "Colibri" ? Speaker::Agent : Speaker::System;
        const auto bytes = content.size() + speaker.size() + 3;
        chat_.push_back({block, tag, body, role, bytes});
        color_chat_message(chat_.back());
        chat_bytes_ += bytes;
        while (!history_installing_ && chat_.size() > 8) {
            chat_bytes_ -= chat_.front().bytes;
            lv_obj_delete(chat_.front().block);
            chat_.pop_front();
            ++history_start_;
            history_before_ = true;
        }
        if (!history_installing_) lv_obj_move_to_index(chat_hint_, -1);
    }

    void receive_chat_reply(const std::string& text) {
        lv_obj_update_layout(panel_);
        const bool follow = screen_ == Screen::Chat && lv_obj_get_scroll_bottom(panel_) <= 2;
        ++history_request_; history_loading_ = false; history_pending_.clear();
        active_question_.clear();
        if (history_after_) { refresh(); return; }
        append("Colibri", text);
        refresh();
        if (!follow) return;

        lv_obj_update_layout(panel_);
        auto* reply = chat_.back().block;
        int delta = -lv_obj_get_scroll_bottom(panel_);
        if (lv_obj_get_height(reply) > lv_obj_get_content_height(panel_)) {
            lv_area_t viewport{}, message{};
            lv_obj_get_content_coords(panel_, &viewport);
            lv_obj_get_coords(reply, &message);
            delta = viewport.y1 + 4 - message.y1;
        }
        lv_obj_scroll_by_bounded(panel_, 0, delta, LV_ANIM_OFF);
    }

    void query_pinyin() {
        page_ = 0;
        choices_.clear();
        candidate_pages_.clear();
        pending_choice_.reset();
        pinyin_pending_ = false;
        if (!spelling_.empty()) {
            try {
                worker_.send({{"type", "pinyin"}, {"spelling", spelling_}});
                pinyin_pending_ = true;
            }
            catch (const std::exception&) { append("系统", "拼音输入服务不可用"); }
        }
    }

    void clear_pinyin() {
        spelling_.clear();
        query_pinyin();
    }

    void choose(std::size_t slot) {
        if (pinyin_pending_) {
            pending_choice_ = slot;
            return;
        }
        if (page_ >= candidate_pages_.size() || slot >= candidate_pages_[page_].count) return;
        const auto& candidate = choices_[candidate_pages_[page_].first + slot];
        if (!insert_draft(candidate.text)) return;
        spelling_.erase(0, candidate.consumed);
        query_pinyin();
    }

    void paginate_choices() {
        constexpr int row_width = 267;
        const auto* font = lv_obj_get_style_text_font(candidates_, LV_PART_MAIN);
        auto width = [font](const std::string& label) {
            lv_point_t size{};
            lv_text_get_size(&size, label.c_str(), font, 0, 0, LV_COORD_MAX, LV_TEXT_FLAG_NONE);
            return size.x;
        };
        candidate_pages_.clear();
        page_ = 0;
        for (std::size_t i = 0; i < choices_.size();) {
            CandidatePage page{i, 0, ""};
            while (i < choices_.size() && page.count < PageSize) {
                const auto prefix = std::to_string(page.count + 1) + " ";
                const auto& text = choices_[i].text;
                auto label = prefix + text + "  ";
                if (width(page.text + label) > row_width) {
                    if (page.count) break;
                    std::size_t end = 0;
                    while (end < text.size()) {
                        const auto next = utf8_end(text, end);
                        if (width(prefix + text.substr(0, next) + "…  ") > row_width) break;
                        end = next;
                    }
                    label = prefix + text.substr(0, end) + "…  ";
                }
                page.text += label;
                ++page.count;
                ++i;
            }
            candidate_pages_.push_back(std::move(page));
        }
    }

    void request_history(const std::string& direction) {
        if (closing_) return;
        if (history_loading_) { history_pending_ = direction; return; }
        history_loading_ = true;
        history_direction_ = direction;
        try { worker_.send({{"type", "history_page"}, {"request_id", ++history_request_},
                           {"direction", direction}, {"start", history_start_}}); }
        catch (const std::exception&) { history_loading_ = false; }
    }

    void scroll(Key key) {
        if (screen_ == Screen::Chat && !history_loading_) {
            if (key == Key::Up && lv_obj_get_scroll_y(panel_) <= 0 && history_before_) request_history("before");
            else if (key == Key::Down && lv_obj_get_scroll_bottom(panel_) <= 2 && history_after_) request_history("after");
        }
        lv_obj_scroll_by_bounded(panel_, 0, key == Key::Up ? 25 : -25, LV_ANIM_OFF);
    }

    void open_memory(const std::string& name) {
        memory_parent_ = screen_;
        memory_selection_ = selection_;
        error_.clear();
        send({{"type", "memory_read"}, {"file", name}}, "读取文件");
    }

    void memory_view_key(Key key) {
        if (key == Key::Escape || key == Key::Left) {
            screen_ = memory_parent_;
            selection_ = memory_selection_;
            error_.clear();
        } else if (key == Key::Enter || key == Key::Right) {
            send({{"type", "memory_read"}, {"file", memory_file_}}, "刷新记忆");
        } else if (key == Key::Up || key == Key::Down) scroll(key);
    }

    void refresh_group() {
        group_fields_ = json::array();
        for (const auto& field : fields_) {
            if (field.value("key", "").find(CategoryPrefixes[category_]) == 0)
                group_fields_.push_back(field);
        }
    }

    std::string saved_model(const char* key) const {
        for (const auto& field : fields_)
            if (field.value("key", "") == key) return field.value("value", "");
        return "";
    }

    std::string key_indicator() const {
        if (!model_key_.is_null()) return model_key_.get<std::string>().empty() ? "" : "•••";
        return model_url_ == saved_model("model.base_url") ? saved_model("model.api_key") : "";
    }

    void open_model_form(const ProviderPreset* provider) {
        model_parent_ = screen_;
        model_title_ = provider ? provider->name : "自定义连接";
        const auto previous_url = saved_model("model.base_url");
        model_url_ = provider ? provider->url : previous_url;
        model_id_ = provider && model_url_ != previous_url ? provider->model : saved_model("model.model");
        model_key_ = nullptr;
        error_.clear();
        screen_ = Screen::ModelForm;
        selection_ = 0;
    }

    void open_editor(const json& field) {
        edit_parent_ = screen_;
        edit_field_ = field;
        edited_ = field.value("secret", false) ? "" : field.value("value", "");
        error_.clear();
        screen_ = Screen::Edit;
    }

    std::vector<std::string> list_rows() const {
        switch (screen_) {
        case Screen::Menu: {
            std::vector<std::string> rows;
            for (const auto& item : MainMenu) rows.emplace_back(item.label);
            return rows;
        }
        case Screen::HelpMenu: return HelpSections;
        case Screen::Settings: return CategoryNames;
        case Screen::ModelMode: return ModelModes;
        case Screen::Providers: {
            std::vector<std::string> rows;
            for (const auto& preset : Presets) rows.emplace_back(preset.name);
            return rows;
        }
        case Screen::ModelForm:
            return {"Base URL", key_indicator().empty() ? "API Key" : "API Key · 已填", "模型 ID", "保存连接"};
        case Screen::Fields: {
            std::vector<std::string> rows;
            for (const auto& field : group_fields_) {
                const auto key = field.value("key", "");
                auto label = SettingLabels.value(key, "");
                if (key == "memory.enabled") label += field.value("value", "") == "true" ? " · 开" : " · 关";
                if (key == "session.max_context_memory_mib") {
                    const auto value = field.value("value", "0");
                    label += value == "0" ? " · 未设置" : " · " + value;
                }
                rows.push_back(std::move(label));
            }
            return rows;
        }
        case Screen::MemoryFiles: {
            auto rows = MemoryFiles;
            rows.emplace_back("主题文件");
            return rows;
        }
        case Screen::MemoryTopics: {
            std::vector<std::string> rows;
            for (const auto& file : memory_topics_) rows.push_back(file.substr(7));
            return rows;
        }
        case Screen::CatalogMenu: return CatalogKinds;
        case Screen::CatalogList: {
            std::vector<std::string> rows;
            for (const auto& item : catalog_items_) rows.push_back(item.value("name", ""));
            return rows;
        }
        default: return {};
        }
    }

    void go_back() {
        switch (screen_) {
        case Screen::Menu: screen_ = Screen::Chat; request_history("last"); break;
        case Screen::HelpMenu: screen_ = Screen::Menu; selection_ = HelpMenuIndex; break;
        case Screen::Settings: screen_ = Screen::Menu; selection_ = SettingsMenuIndex; break;
        case Screen::CatalogMenu: screen_ = Screen::Menu; selection_ = CatalogMenuIndex; break;
        case Screen::CatalogList: screen_ = Screen::CatalogMenu; selection_ = catalog_kind_; break;
        case Screen::Fields:
        case Screen::ModelMode: screen_ = Screen::Settings; selection_ = category_; break;
        case Screen::Providers: screen_ = Screen::ModelMode; selection_ = 0; break;
        case Screen::MemoryFiles: screen_ = Screen::Menu; selection_ = MemoryMenuIndex; break;
        case Screen::MemoryTopics: screen_ = Screen::MemoryFiles; selection_ = MemoryFiles.size(); break;
        case Screen::ModelForm:
            screen_ = model_parent_;
            selection_ = screen_ == Screen::Providers ? preset_ : 1;
            model_key_ = nullptr;
            break;
        default: break;
        }
        error_.clear();
    }

    void open_item() {
        switch (screen_) {
        case Screen::Menu:
            switch (MainMenu[selection_].action) {
            case MenuAction::Memory:
                screen_ = Screen::MemoryFiles;
                selection_ = 0;
                break;
            case MenuAction::Task:
                screen_ = Screen::Task;
                task_text_ = "读取任务进度…";
                send({{"type", "task_status"}}, "读取任务");
                break;
            case MenuAction::Catalog:
                screen_ = Screen::CatalogMenu;
                selection_ = 0;
                break;
            case MenuAction::Settings:
                screen_ = Screen::Settings;
                selection_ = 0;
                send({{"type", "settings"}}, "读取设置");
                break;
            case MenuAction::Help: screen_ = Screen::HelpMenu; selection_ = 0; break;
            case MenuAction::Quit: request_exit(); break;
            }
            break;
        case Screen::HelpMenu:
            help_kind_ = selection_;
            screen_ = Screen::Help;
            break;
        case Screen::CatalogMenu:
            catalog_kind_ = selection_;
            catalog_items_.clear();
            screen_ = Screen::CatalogList;
            selection_ = 0;
            send({{"type", "catalog"}, {"kind", catalog_kind_ == 0 ? "mcp" : "skills"}}, "读取列表");
            break;
        case Screen::CatalogList: screen_ = Screen::CatalogItem; break;
        case Screen::Settings:
            category_ = selection_;
            screen_ = category_ == 0 ? Screen::ModelMode : Screen::Fields;
            selection_ = 0;
            refresh_group();
            break;
        case Screen::Fields:
            if (selection_ < group_fields_.size()) {
                const auto& field = group_fields_[selection_];
                if (field.value("key", "") == "memory.enabled") {
                    edit_parent_ = Screen::Fields;
                    send({{"type", "save"}, {"key", "memory.enabled"},
                          {"value", field.value("value", "") == "true" ? "false" : "true"}}, "保存");
                } else open_editor(field);
            }
            break;
        case Screen::MemoryFiles:
            if (selection_ < MemoryFiles.size()) open_memory(MemoryFiles[selection_]);
            else {
                memory_topics_.clear();
                screen_ = Screen::MemoryTopics;
                selection_ = 0;
                error_.clear();
                send({{"type", "memory_topics"}}, "读取主题");
            }
            break;
        case Screen::MemoryTopics: open_memory(memory_topics_[selection_]); break;
        case Screen::ModelMode:
            if (selection_ == 0) {
                screen_ = Screen::Providers;
                selection_ = 0;
            } else open_model_form(nullptr);
            break;
        case Screen::Providers:
            preset_ = selection_;
            open_model_form(&Presets[preset_]);
            break;
        case Screen::ModelForm:
            switch (selection_) {
            case 0: open_editor({{"label", "Base URL"}, {"value", model_url_}}); break;
            case 1: open_editor({{"label", "API Key"}, {"value", key_indicator()}, {"secret", true}}); break;
            case 2: open_editor({{"label", "模型 ID"}, {"value", model_id_}}); break;
            case 3:
                error_.clear();
                send({{"type", "save_model"}, {"base_url", model_url_}, {"model", model_id_},
                      {"api_key", model_key_}}, "保存");
                break;
            }
            break;
        default: break;
        }
    }

    void navigate(Key key) {
        const auto count = list_rows().size();
        if (key == Key::Escape || key == Key::Left) go_back();
        else if (key == Key::Up && selection_ > 0) --selection_;
        else if (key == Key::Down && selection_ + 1 < count) ++selection_;
        else if ((key == Key::Enter || key == Key::Right) && selection_ < count) open_item();
        if (running) refresh(screen_ == Screen::Chat);
    }

    std::string tail(std::string text) const {
        std::size_t start = 0;
        while (text.size() - start > 65) start = utf8_end(text, start);
        const auto* font = lv_obj_get_style_text_font(composer_, LV_PART_MAIN);
        lv_point_t size{};
        while (start < text.size()) {
            lv_text_get_size(&size, text.c_str() + start, font, 0, 0, LV_COORD_MAX, LV_TEXT_FLAG_NONE);
            if (size.x <= 213) break;
            start = utf8_end(text, start);
        }
        return text.substr(start);
    }

    bool insert_draft(const std::string& text) {
        if (draft_.size() + text.size() > 4096) return false;
        draft_.insert(draft_cursor_, text);
        draft_cursor_ += text.size();
        return true;
    }

    std::string draft_preview() {
        if (draft_.empty() && spelling_.empty()) return "› 输入消息";
        std::string line = draft_;
        for (auto& ch : line) {
            if (ch == '\n' || ch == '\r' || ch == '\t') ch = ' ';
        }
        std::string cursor = spelling_.empty() ? "│" : "[" + tail(spelling_) + "]│";
        auto fits = [this](const std::string& text) {
            lv_point_t size{};
            lv_text_get_size(&size, text.c_str(), lv_obj_get_style_text_font(composer_, LV_PART_MAIN),
                             0, 0, LV_COORD_MAX, LV_TEXT_FLAG_NONE);
            return size.x <= 244;
        };
        auto prefix = [&]() { return draft_view_start_ ? "› …" : "› "; };
        draft_view_start_ = std::min(draft_cursor_, draft_view_start_);
        while (draft_cursor_ - draft_view_start_ > 180)
            draft_view_start_ = utf8_end(line, draft_view_start_);
        auto preview = prefix() + line.substr(draft_view_start_, draft_cursor_ - draft_view_start_) + cursor;
        while (!fits(preview) && draft_view_start_ < draft_cursor_) {
            draft_view_start_ = utf8_end(line, draft_view_start_);
            preview = prefix() + line.substr(draft_view_start_, draft_cursor_ - draft_view_start_) + cursor;
        }
        for (auto end = draft_cursor_; end < line.size();) {
            const auto next = utf8_end(line, end);
            const auto character = line.substr(end, next - end);
            if (!fits(preview + character + (next < line.size() ? "…" : ""))) {
                if (fits(preview + "…")) preview += "…";
                break;
            }
            preview += character;
            end = next;
        }
        return preview;
    }

    void refresh(bool bottom = false) {
        const auto background = lv_color_hex(0x101b21);
        const auto surface = lv_color_hex(0x17262c);
        const auto foreground = lv_color_hex(0xe7f4ed);
        const auto accent = lv_color_hex(0x82dfab);
        lv_obj_set_style_bg_color(lv_screen_active(), background, 0);
        lv_obj_set_style_bg_opa(lv_screen_active(), LV_OPA_COVER, 0);
        lv_obj_set_style_bg_color(panel_, surface, 0);
        lv_obj_set_style_bg_opa(panel_, LV_OPA_COVER, 0);
        lv_obj_set_style_text_color(body_, foreground, 0);
        lv_obj_set_style_text_color(composer_, foreground, 0);
        lv_obj_set_style_bg_color(composer_, lv_color_hex(0x233a3e), 0);
        lv_obj_set_style_bg_opa(composer_, LV_OPA_COVER, 0);
        color_signal();
        const auto muted = lv_color_hex(0xb6c9c4);
        for (auto* badge : {clock_label_, battery_label_}) {
            lv_obj_set_style_bg_color(badge, surface, 0);
            lv_obj_set_style_text_color(badge, muted, 0);
        }
        lv_obj_set_style_text_color(title_, foreground, 0);
        for (auto* item : {status_, candidates_, voice_time_})
            lv_obj_set_style_text_color(item, accent, 0);

        std::string title = "对话";
        const bool show_chat = screen_ == Screen::Chat && (!chat_.empty() || agent_busy_);
        bool show_menu_rows = false;
        for (auto* row : menu_rows_) lv_obj_add_flag(row, LV_OBJ_FLAG_HIDDEN);
        std::string content;
        if (starting_) content = "正在加载对话…";
        else if (!show_chat) {
            content = model_hint_.empty() ?
                "输入问题开始对话。\n用 Tab 切换拼音，或用 Esc 打开配置。" : model_hint_;
        }
        std::string composer = screen_ == Screen::Chat && !starting_ ? draft_preview() : "";
        std::string hint = "Esc 设置  Tab 中/英  Fn+空格 语音";
        if (!spelling_.empty()) {
            hint = candidate_pages_.empty() ? "继续输入拼音" : candidate_pages_[page_].text;
            if (candidate_pages_.size() > 1) hint += " ‹ ›";
        }
        if (recording_) {
            composer = "正在录音";
            hint = "Fn+空格 结束   Esc 取消";
        }
        if (is_list(screen_)) {
            const auto rows = list_rows();
            switch (screen_) {
            case Screen::Menu: title = "设置"; break;
            case Screen::HelpMenu: title = "帮助"; break;
            case Screen::Settings: title = "配置"; break;
            case Screen::Fields: title = "配置  /  " + CategoryNames[category_]; break;
            case Screen::ModelMode: title = "配置  /  模型"; break;
            case Screen::Providers: title = "模型  /  服务商预设"; break;
            case Screen::ModelForm: title = "模型  /  " + model_title_; break;
            case Screen::MemoryFiles: title = "记忆"; break;
            case Screen::MemoryTopics: title = "记忆  /  主题文件"; break;
            case Screen::CatalogMenu: title = "插件"; break;
            case Screen::CatalogList: title = "查看  /  " + CatalogKinds[catalog_kind_]; break;
            default: break;
            }
            content.clear();
            show_menu_rows = !rows.empty();
            composer = screen_ == Screen::ModelForm && error_.empty() ? "OpenAI 兼容接口" : error_;
            const auto visible = screen_ == Screen::Menu ? menu_rows_.size() : (composer.empty() ? 5u : 4u);
            const int row_step = screen_ == Screen::Menu ? 20 : 22;
            const auto max_first = rows.size() > visible ? rows.size() - visible : 0;
            const auto first = std::min(selection_ > 2 ? selection_ - 2 : 0, max_first);
            for (std::size_t i = 0; i < visible && first + i < rows.size(); ++i) {
                auto* row = menu_rows_[i];
                const int row_height = row_step - 1;
                lv_obj_set_y(row, 3 + row_step * i);
                lv_obj_set_height(row, row_height);
                const auto* font = lv_obj_get_style_text_font(row, LV_PART_MAIN);
                lv_obj_set_style_pad_top(row, std::max(0, (row_height - static_cast<int>(font->line_height)) / 2), 0);
                const bool selected = first + i == selection_;
                lv_obj_remove_flag(row, LV_OBJ_FLAG_HIDDEN);
                lv_label_set_text(row, rows[first + i].c_str());
                lv_obj_set_style_border_color(row, accent, 0);
                lv_obj_set_style_border_opa(row, selected ? LV_OPA_COVER : LV_OPA_TRANSP, 0);
                lv_obj_set_style_bg_opa(row, selected ? LV_OPA_COVER : LV_OPA_TRANSP, 0);
                lv_obj_set_style_bg_color(row, lv_color_hex(selected ? 0x356654 : 0x243940), 0);
                lv_obj_set_style_text_color(row, selected ? lv_color_hex(0xf0fff7) : foreground, 0);
            }
            if (rows.empty() && screen_ == Screen::MemoryTopics) content = "暂无主题文件";
            if (rows.empty() && screen_ == Screen::CatalogList)
                content = catalog_kind_ == 0 ? "尚未配置 MCP" : "没有可用技能";
            hint = screen_ == Screen::Menu && MainMenu[selection_].action == MenuAction::Quit
                ? "F/X 选择     Enter 退出     Esc 返回" : "F/X 选择     Enter 打开     Esc 返回";
        } else if (screen_ == Screen::Edit) {
            title = "编辑";
            const auto& field = edit_field_;
            const bool secret = field.value("secret", false);
            content = field.value("label", SettingLabels.value(field.value("key", ""), "")) + "\n";
            if (secret) content += field.value("value", "").empty() ? "输入新值" : "已保存值 · 输入以替换";
            else content += field.value("value", "");
            if (field.value("key", "") == "session.max_context_memory_mib")
                content += "\n单位 MiB，0 表示未设置";
            if (!error_.empty()) content += "\n" + error_;
            composer = "› " + (secret ? std::string(std::min<std::size_t>(edited_.size(), 30), '*') : tail(edited_));
            hint = "Enter 保存   Tab 清空   Esc 返回";
        } else if (screen_ == Screen::Help) {
            title = "帮助 / " + HelpSections[help_kind_];
            content = HelpTexts[help_kind_];
            composer.clear();
            hint = "F/X 滚动  Esc 返回";
        } else if (screen_ == Screen::Task) {
            title = "任务进度";
            content = task_text_.empty() ? "当前没有记录的任务" : task_text_;
            composer.clear();
            hint = "F/X 滚动  Enter 刷新  Esc 返回";
        } else if (screen_ == Screen::CatalogItem) {
            const auto& item = catalog_items_[selection_];
            title = "查看  /  " + CatalogKinds[catalog_kind_];
            content = item.value("name", "") + "\n\n" + item.value("description", "");
            composer.clear();
            hint = "F/X 滚动  Z/Esc 返回";
        } else if (screen_ == Screen::MemoryView) {
            title = "记忆  /  " + memory_file_;
            content = memory_text_.empty() ? "文件为空" : memory_text_;
            composer = error_;
            hint = "F/X 滚动  C/Enter 刷新  Z/Esc 返回";
        }
        lv_label_set_text(title_, title.c_str());
        const bool show_mode = screen_ == Screen::Chat && !starting_ && !recording_ && !transcribing_;
        lv_obj_set_flag(status_, LV_OBJ_FLAG_HIDDEN, !show_mode);
        if (show_mode) lv_label_set_text(status_, chinese_ ? "拼音" : "ABC");
        if (transcribing_ && screen_ == Screen::Chat) composer = "正在转写";
        const bool reserve_composer = screen_ == Screen::Chat || screen_ == Screen::Edit || !composer.empty();
        lv_obj_set_height(panel_, reserve_composer ? 96 : 124);
        if (show_chat) {
            lv_obj_add_flag(body_, LV_OBJ_FLAG_HIDDEN);
            lv_obj_remove_flag(chat_thread_, LV_OBJ_FLAG_HIDDEN);
            lv_obj_set_style_text_color(chat_hint_, foreground, 0);
            if (model_hint_.empty()) lv_obj_add_flag(chat_hint_, LV_OBJ_FLAG_HIDDEN);
            else {
                lv_obj_remove_flag(chat_hint_, LV_OBJ_FLAG_HIDDEN);
                lv_label_set_text(chat_hint_, model_hint_.c_str());
            }
        } else {
            if (show_menu_rows) lv_obj_add_flag(body_, LV_OBJ_FLAG_HIDDEN);
            else lv_obj_remove_flag(body_, LV_OBJ_FLAG_HIDDEN);
            lv_obj_add_flag(chat_thread_, LV_OBJ_FLAG_HIDDEN);
        }
        const bool changed = content != lv_label_get_text(body_);
        if (changed) lv_label_set_text(body_, content.c_str());
        if (screen_ == Screen::Chat && bottom) {
            lv_obj_update_layout(panel_);
            lv_obj_scroll_by_bounded(panel_, 0, -lv_obj_get_scroll_bottom(panel_), LV_ANIM_OFF);
        } else if (screen_ != Screen::Chat && (changed || show_menu_rows)) {
            lv_obj_scroll_to_y(panel_, 0, LV_ANIM_OFF);
        }
        if (composer.empty()) lv_obj_add_flag(composer_, LV_OBJ_FLAG_HIDDEN);
        else lv_obj_remove_flag(composer_, LV_OBJ_FLAG_HIDDEN);
        const bool voice_visible = recording_ && screen_ == Screen::Chat;
        for (size_t i = 0; i < voice_bars_.size(); ++i) {
            auto* bar = voice_bars_[i];
            lv_obj_set_flag(bar, LV_OBJ_FLAG_HIDDEN, !(voice_visible || (has_activity_dots(activity()) && i < 3)));
            lv_obj_set_style_bg_color(bar, accent, 0);
        }
        lv_obj_set_flag(voice_time_, LV_OBJ_FLAG_HIDDEN, !voice_visible);
        if (voice_visible) {
            lv_label_set_text(voice_time_, recording_caption().c_str());
        }
        lv_label_set_text(composer_, composer.c_str());
        lv_obj_update_layout(composer_);
        update_composer_animation();
        lv_obj_set_style_image_recolor(bird_, accent, 0);
        lv_obj_set_style_image_recolor_opa(bird_, LV_OPA_COVER, 0);
        const bool keycaps = spelling_.empty();
        lv_obj_set_flag(candidates_, LV_OBJ_FLAG_HIDDEN, starting_ || keycaps);
        lv_obj_set_flag(footer_, LV_OBJ_FLAG_HIDDEN, starting_ || !keycaps);
        if (keycaps) {
            std::istringstream words(hint);
            for (size_t i = 0; i < footer_parts_.size(); ++i) {
                auto* part = footer_parts_[i];
                std::string word;
                words >> word;
                lv_obj_set_flag(part, LV_OBJ_FLAG_HIDDEN, word.empty());
                lv_label_set_text(part, word.c_str());
                lv_obj_set_style_text_color(part, i % 2 ? foreground : accent, 0);
                if (i % 2 == 0)
                    lv_obj_set_style_bg_color(part, lv_color_hex(0x243e36), 0);
            }
        } else lv_label_set_text(candidates_, hint.c_str());
    }
};

#if COLIBRI_DESKTOP
bool save_screenshot(const char* path) {
    auto* snapshot = lv_snapshot_take(lv_screen_active(), LV_COLOR_FORMAT_XRGB8888);
    if (!snapshot) return false;
    auto* surface = SDL_CreateRGBSurfaceWithFormatFrom(snapshot->data, snapshot->header.w,
        snapshot->header.h, 32, snapshot->header.stride, SDL_PIXELFORMAT_RGB888);
    const bool written = surface && SDL_SaveBMP(surface, path) == 0;
    if (surface) SDL_FreeSurface(surface);
    lv_draw_buf_destroy(snapshot);
    return written;
}
#endif

uint32_t ticks() {
    using namespace std::chrono;
    return static_cast<uint32_t>(duration_cast<milliseconds>(steady_clock::now().time_since_epoch()).count());
}
}

int main(int argc, char** argv) {
    if (argc == 2 && std::string_view(argv[1]) == "--weixin-qr-view") return run_qr_view();
    if (argc > 1) {
        const char* worker = std::getenv("COLIBRI_WORKER");
        if (!worker) worker = COLIBRI_WORKER_PATH;
        argv[0] = const_cast<char*>(worker);
        execv(worker, argv);
        std::perror(worker);
        return 1;
    }
    try {
        WorkerLink worker;
        lv_init();
        lv_tick_set_cb(ticks);
#if COLIBRI_DESKTOP
        auto* display = lv_sdl_window_create(Width, Height);
        if (!display) return 1;
        lv_sdl_window_set_zoom(display, 2);
#else
        auto* display = lv_linux_fbdev_create();
        const char* framebuffer = std::getenv("COLIBRI_FRAMEBUFFER");
        if (!display || lv_linux_fbdev_set_file(display, framebuffer ? framebuffer : "/dev/fb0") != LV_RESULT_OK)
            return 1;
#endif
        DisplayIdle idle(ticks(), !COLIBRI_DESKTOP);
        View view(worker);
#if COLIBRI_DESKTOP
        if (argc == 3 && std::string(argv[1]) == "--screenshot")
            return save_screenshot(argv[2]) ? 0 : 1;
#endif
        const auto first_frame_deadline = std::chrono::steady_clock::now() + std::chrono::milliseconds(1000);
        while (view.running) {
            const bool input_open = poll_input([&](Key key) { view.key(key); },
                                          [&](const std::string& text) { view.text(text); },
                                          [&](unsigned code, int value) {
                                              const bool was_sleeping = idle.sleeping();
                                              const bool consumed = idle.input(code, value, ticks());
                                              if (was_sleeping && !idle.sleeping()) lv_obj_invalidate(lv_screen_active());
                                              return consumed;
                                          });
            if (!input_open) view.request_exit();
            if (!view.running) break;
            for (const auto& event : worker.receive()) view.receive(event);
            if (!view.running) break;
            idle.update(ticks());
            view.update_device_status();
            if (!idle.sleeping()) view.update_composer_animation();
            if (!idle.sleeping() && (!view.restoring_history() || std::chrono::steady_clock::now() >= first_frame_deadline))
                lv_timer_handler();
            std::this_thread::sleep_for(std::chrono::milliseconds(12));
        }
    } catch (const std::exception& error) {
        std::fprintf(stderr, "Colibri: %s\n", error.what());
        return 1;
    }
    return 0;
}
