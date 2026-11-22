#include "input.hpp"
#include "markdown.hpp"
#include "worker_link.hpp"
#include <lvgl.h>
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
#include <optional>
#include <thread>
#include <string_view>
#include <unistd.h>

using nlohmann::json;

namespace {
constexpr int Width = 320;
constexpr int Height = 170;
constexpr int PageSize = 6;
constexpr int ChatWidth = 288;
constexpr int CodePadding = 6;
constexpr int CodeBorder = 1;
constexpr std::size_t MaxSpelling = 64;
const json SettingLabels{
    {"model.provider", "模型 · 提供方"}, {"model.base_url", "模型 · API 地址"},
    {"model.model", "模型 · 名称"}, {"model.api_key", "模型 · API 密钥"},
    {"vision.model", "视觉模型名称"}, {"vision.base_url", "视觉 API 地址"},
    {"vision.api_key", "视觉 API 密钥"}, {"vision.timeout_seconds", "超时（秒）"},
    {"vision.max_image_bytes", "图片上限（字节）"},
    {"speech.backend", "语音 · 转写方式"}, {"speech.base_url", "语音 · API 地址"}, {"speech.model", "语音 · 模型"},
    {"speech.api_key", "语音 · API 密钥"}, {"web_search.engine", "搜索 · 引擎"},
    {"web_search.endpoint", "搜索 · API 地址"}, {"web_search.api_key", "搜索 · API 密钥"},
    {"session.transcript", "历史 · 保存记录"}, {"session.restore_transcript", "历史 · 启动恢复"},
    {"session.transcript_retention_days", "历史 · 保留天数"},
    {"session.transcript_max_total_bytes", "历史 · 容量（字节）"},
    {"memory.enabled", "启用长期记忆"}, {"memory.root", "记忆目录"},
    {"mcp.servers", "MCP · 服务器 JSON"},
};
const std::vector<std::string> Menu{"MCP 与技能", "设置", "退出 Colibri"};

enum class Screen { Chat, Menu, Settings, Fields, ModelMode, Providers, ModelForm, Edit, MemoryTopics, MemoryView, MemoryEdit, MemoryActions, CatalogMenu, CatalogList, CatalogItem };

const std::vector<std::string> CategoryNames{"模型", "图片理解", "语音", "搜索", "历史", "记忆", "MCP 配置", "显示"};
const std::vector<std::string> CategoryPrefixes{"model.", "vision.", "speech.", "web_search.", "session.", "memory.", "mcp.", "display."};
constexpr std::size_t MemoryCategory = 5;
constexpr std::size_t DisplayCategory = 7;
const std::vector<std::string> CatalogKinds{"MCP 服务", "技能"};
constexpr std::size_t MemoryEditMaxBytes = 64 * 1024;
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
    return screen == Screen::Menu || screen == Screen::Settings || screen == Screen::Fields ||
           screen == Screen::ModelMode || screen == Screen::Providers || screen == Screen::ModelForm ||
           screen == Screen::MemoryTopics || screen == Screen::MemoryActions ||
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
                           std::uint16_t size = 14) {
    const char* custom = std::getenv("COLIBRI_FONT");
    const std::vector<std::string> paths{
        custom ? custom : "",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
    };
    for (const auto& path : paths) {
        if (path.empty() || !std::filesystem::exists(path)) continue;
        if (auto* font = lv_freetype_font_create(path.c_str(), LV_FREETYPE_FONT_RENDER_MODE_BITMAP,
                                                 size, style)) {
            static const auto* emoji = lv_freetype_font_create(
                COLIBRI_EMOJI_FONT_PATH, LV_FREETYPE_FONT_RENDER_MODE_BITMAP, 14, LV_FREETYPE_FONT_STYLE_NORMAL);
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
                                                 14, LV_FREETYPE_FONT_STYLE_NORMAL)) {
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
        const auto* normal = load_font();
        lv_obj_set_style_text_font(root, normal, 0);
        bold_ = load_font(LV_FREETYPE_FONT_STYLE_BOLD);
        italic_ = load_font(LV_FREETYPE_FONT_STYLE_ITALIC);
        heading_ = load_font(LV_FREETYPE_FONT_STYLE_BOLD, 16);
        code_ = code_font(normal);

        title_ = label(root, 10, 4, 212);
        status_ = label(root, 225, 4, 85);
        lv_obj_set_style_text_align(status_, LV_TEXT_ALIGN_RIGHT, 0);
        panel_ = lv_obj_create(root);
        lv_obj_remove_style_all(panel_);
        lv_obj_set_pos(panel_, 7, 27);
        lv_obj_set_size(panel_, 306, 91);
        lv_obj_set_style_radius(panel_, 7, 0);
        lv_obj_set_scroll_dir(panel_, LV_DIR_VER);
        lv_obj_remove_flag(panel_, LV_OBJ_FLAG_SCROLL_ELASTIC);
        lv_obj_set_scrollbar_mode(panel_, LV_SCROLLBAR_MODE_AUTO);
        body_ = label(panel_, 8, 4, 288, LV_LABEL_LONG_WRAP);
        lv_obj_set_style_text_line_space(body_, 3, 0);
        chat_thread_ = column(panel_, 8, 4, 8);
        chat_hint_ = label(chat_thread_, 0, 0, 288, LV_LABEL_LONG_WRAP);
        lv_obj_add_flag(chat_hint_, LV_OBJ_FLAG_HIDDEN);
        for (std::size_t i = 0; i < menu_rows_.size(); ++i) {
            auto* row = label(panel_, 8, 3 + 22 * i, ChatWidth, LV_LABEL_LONG_DOT);
            lv_obj_set_height(row, 21);
            lv_obj_set_style_pad_hor(row, 7, 0);
            lv_obj_set_style_pad_top(row, 2, 0);
            lv_obj_set_style_radius(row, 4, 0);
            menu_rows_[i] = row;
        }
        composer_ = label(root, 7, 122, 306);
        lv_obj_set_height(composer_, 24);
        lv_obj_set_style_pad_left(composer_, 8, 0);
        lv_obj_set_style_pad_top(composer_, 3, 0);
        lv_obj_set_style_radius(composer_, 6, 0);
        candidates_ = label(root, 10, 149, 300);
        memory_editor_ = lv_textarea_create(root);
        lv_obj_set_pos(memory_editor_, 7, 27);
        lv_obj_set_size(memory_editor_, 306, 91);
        lv_textarea_set_one_line(memory_editor_, false);
        lv_obj_remove_flag(memory_editor_, LV_OBJ_FLAG_SCROLL_ELASTIC);
        lv_obj_add_state(memory_editor_, LV_STATE_FOCUSED);
        send({{"type", "model_status"}}, "检查配置");
        starting_ = busy_;
        refresh();
    }

    bool running = true;

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
        if (type == "status") {
            append("系统", text);
            refresh(true);
            return;
        }
        busy_ = false;
        status_text_ = "";
        recording_ = type == "recording";
        if (recording_) offline_voice_ = event.value("backend", "") == "offline";
        if (type == "model_status") {
            starting_ = false;
            model_hint_ = text;
            for (const auto& message : event.value("history", json::array())) {
                append(message.value("role", "") == "user" ? "我" : "Colibri", message.value("text", ""));
            }
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
        } else if (type == "memory_file" || type == "memory_saved") {
            memory_file_ = event.at("file").get<std::string>();
            memory_text_ = text;
            memory_revision_ = event.at("revision");
            screen_ = Screen::MemoryView;
            error_ = type == "memory_saved" ? "已保存" : "";
        } else if (type == "model_saved") {
            fields_ = event.at("fields");
            model_hint_.clear();
            model_url_ = saved_model("model.base_url");
            model_id_ = saved_model("model.model");
            model_key_ = nullptr;
            error_ = "已保存";
        } else if (type == "draft") {
            if (!insert_draft(text)) append("系统", "草稿已满，语音未插入");
        } else if (type == "chat_cleared") {
            for (const auto& message : chat_) lv_obj_delete(message.block);
            chat_.clear();
            chat_bytes_ = 0;
            refresh();
            lv_obj_update_layout(panel_);
            lv_obj_scroll_to_y(panel_, 0, LV_ANIM_OFF);
            return;
        } else if (type == "reply") {
            append("Colibri", text);
        } else if (type == "error") {
            starting_ = false;
            if (screen_ == Screen::Edit || screen_ == Screen::ModelForm || screen_ == Screen::MemoryActions ||
                screen_ == Screen::MemoryTopics || screen_ == Screen::CatalogList || (screen_ == Screen::Fields && category_ == MemoryCategory))
                error_ = text;
            else append("系统", text);
        }
        refresh(true);
    }

    void key(Key key) {
        if (key == Key::ChatTop) {
            if (screen_ == Screen::Chat) lv_obj_scroll_to_y(panel_, 0, LV_ANIM_OFF);
            return;
        }
        if (key == Key::ChatBottom) {
            if (screen_ == Screen::Chat) refresh(true);
            return;
        }
        if (busy_) {
            if (key == Key::Up || key == Key::Down) scroll(key);
            return;
        }
        if (recording_) {
            if (key == Key::Voice || key == Key::Enter || key == Key::Escape) {
                recording_ = false;
                send({{"type", key == Key::Escape ? "voice_cancel" : "voice_stop"}},
                     key == Key::Escape ? "取消录音" : offline_voice_ ? "离线转写" : "在线转写");
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
        if (screen_ == Screen::CatalogItem) {
            if (key == Key::Left || key == Key::Escape) screen_ = Screen::CatalogList;
            else if (key == Key::Up || key == Key::Down) scroll(key);
            refresh();
            return;
        }
        if (screen_ == Screen::MemoryView || screen_ == Screen::MemoryEdit) {
            memory_key(key);
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
            } else if (draft_ == ClearCommand) {
                draft_.clear();
                draft_cursor_ = draft_view_start_ = 0;
                send({{"type", "clear_chat"}}, "清理屏幕");
                refresh();
                return;
            } else if (draft_.find_first_not_of(" \t\n") != std::string::npos) {
                if (!model_hint_.empty()) {
                    refresh(true);
                    return;
                }
                append("我", draft_);
                send({{"type", "submit"}, {"text", draft_}}, "思考中");
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
        if (is_list(screen_) || screen_ == Screen::MemoryView || screen_ == Screen::CatalogItem) {
            if (text == "f" || text == "F") key(Key::Up);
            else if (text == "x" || text == "X") key(Key::Down);
            else if (text == "z" || text == "Z") key(Key::Left);
            else if (text == "c" || text == "C") key(Key::Right);
            return;
        }
        if (recording_ || busy_) return;
        if (screen_ == Screen::Edit) {
            if (edited_.size() + text.size() <= 4096) edited_ += text;
        } else if (screen_ == Screen::Chat && spelling_.empty() && text.size() == 1 &&
                   draft_cursor_ == draft_.size() && draft_.size() < ClearCommand.size() &&
                   draft_ == ClearCommand.substr(0, draft_.size()) && text[0] == ClearCommand[draft_.size()]) {
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
            if (screen_ == Screen::MemoryEdit) insert_memory_text(text);
            else insert_draft(text);
        }
        refresh();
    }

private:
    static constexpr std::string_view ClearCommand = "/clear";
    WorkerLink& worker_;
    Screen screen_ = Screen::Chat, edit_parent_ = Screen::Fields, model_parent_ = Screen::ModelMode;
    lv_obj_t *title_, *status_, *panel_, *body_, *composer_, *candidates_, *memory_editor_;
    lv_obj_t *chat_thread_, *chat_hint_;
    std::array<lv_obj_t*, 4> menu_rows_{};
    const lv_font_t *bold_, *italic_, *heading_, *code_;
    enum class Speaker { User, Agent, System };
    struct ChatMessage {
        lv_obj_t* block;
        lv_obj_t* tag;
        lv_obj_t* body;
        Speaker speaker;
        std::size_t bytes;
    };
    std::deque<ChatMessage> chat_;
    std::size_t chat_bytes_ = 0;
    bool chat_colors_dark_ = true;
    Screen memory_parent_ = Screen::Fields;
    std::size_t memory_selection_ = 0;
    std::string memory_file_, memory_text_;
    json memory_revision_;
    std::vector<std::string> memory_topics_;
    json catalog_items_ = json::array();
    std::size_t catalog_kind_ = 0;
    bool busy_ = false, recording_ = false, offline_voice_ = false, chinese_ = false, dark_ = true;
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
        lv_obj_set_size(item, 288, LV_SIZE_CONTENT);
        lv_obj_set_flex_flow(item, LV_FLEX_FLOW_COLUMN);
        lv_obj_set_style_pad_row(item, gap, 0);
        return item;
    }

    void color_chat_message(const ChatMessage& message) const {
        unsigned text, background;
        switch (message.speaker) {
        case Speaker::User:
            text = dark_ ? 0xbce0ff : 0x204e79;
            background = dark_ ? 0x284358 : 0xd6e7fb;
            break;
        case Speaker::Agent:
            text = dark_ ? 0xa3efbe : 0x18583e;
            background = dark_ ? 0x23493f : 0xd4eddf;
            break;
        case Speaker::System:
            text = dark_ ? 0xd2dade : 0x425459;
            background = dark_ ? 0x34434b : 0xdfebec;
            break;
        }
        lv_obj_set_style_text_color(message.tag, lv_color_hex(text), 0);
        lv_obj_set_style_bg_color(message.tag, lv_color_hex(background), 0);
        lv_obj_set_style_text_color(message.body, lv_color_hex(dark_ ? 0xe7f4ed : 0x192c28), 0);
        for (uint32_t i = 0; i < lv_obj_get_child_count(message.body); ++i) {
            auto* item = lv_obj_get_child(message.body, i);
            if (lv_obj_has_flag(item, LV_OBJ_FLAG_USER_3)) {
                lv_obj_set_style_bg_color(item, lv_color_hex(dark_ ? 0x354c61 : 0xc3d3e6), 0);
                lv_obj_set_style_border_color(item, lv_color_hex(dark_ ? 0x89adc6 : 0x526d88), 0);
            } else if (lv_obj_has_flag(item, LV_OBJ_FLAG_USER_1)) {
                lv_obj_set_style_bg_color(item, lv_color_hex(dark_ ? 0x213333 : 0xe6efec), 0);
                lv_obj_set_style_border_color(item, lv_color_hex(dark_ ? 0x72aa96 : 0x4b7567), 0);
            } else if (lv_obj_has_flag(item, LV_OBJ_FLAG_USER_2)) {
                lv_obj_set_style_bg_color(item, lv_color_hex(dark_ ? 0x558476 : 0x87ada0), 0);
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
            auto* line = label(body, 0, 0, 288, LV_LABEL_LONG_WRAP);
            lv_obj_set_style_text_line_space(line, 3, 0);
            lv_label_set_text(line, content.c_str());
            return;
        }
        for (const auto& block : blocks) {
            using markdown::BlockKind;
            if (block.kind == BlockKind::Rule) {
                auto* rule = lv_obj_create(body);
                lv_obj_remove_style_all(rule);
                lv_obj_remove_flag(rule, LV_OBJ_FLAG_SCROLLABLE);
                lv_obj_add_flag(rule, LV_OBJ_FLAG_USER_2);
                lv_obj_set_size(rule, 288, 1);
                lv_obj_set_style_bg_opa(rule, LV_OPA_COVER, 0);
                continue;
            }
            if (block.runs.empty()) continue;
            auto* group = lv_spangroup_create(body);
            lv_obj_set_size(group, ChatWidth, LV_SIZE_CONTENT);
            lv_obj_remove_flag(group, LV_OBJ_FLAG_SCROLLABLE);
            lv_obj_set_style_text_line_space(group, 3, 0);
            if (block.kind == BlockKind::Heading) {
                lv_obj_set_style_text_font(group, heading_, 0);
                lv_obj_set_style_pad_top(group, 2, 0);
            } else if (block.kind == BlockKind::Code || block.kind == BlockKind::Quote || block.kind == BlockKind::Table) {
                lv_obj_add_flag(group, block.kind == BlockKind::Code ? LV_OBJ_FLAG_USER_3 : LV_OBJ_FLAG_USER_1);
                lv_obj_set_style_bg_opa(group, LV_OPA_COVER, 0);
                lv_obj_set_style_pad_hor(group, CodePadding, 0);
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
        auto* block = column(chat_thread_, 0, 0, 2);
        auto* tag = label(block, 0, 0, LV_SIZE_CONTENT);
        lv_label_set_text(tag, speaker.c_str());
        lv_obj_set_style_bg_opa(tag, LV_OPA_COVER, 0);
        lv_obj_set_style_radius(tag, 4, 0);
        lv_obj_set_style_pad_left(tag, 5, 0);
        lv_obj_set_style_pad_right(tag, 5, 0);
        auto* body = column(block, 0, 0, 3);
        render_markdown(body, content, speaker != "系统");
        const auto role = speaker == "我" ? Speaker::User :
                          speaker == "Colibri" ? Speaker::Agent : Speaker::System;
        const auto bytes = content.size() + speaker.size() + 3;
        chat_.push_back({block, tag, body, role, bytes});
        color_chat_message(chat_.back());
        chat_bytes_ += bytes;
        while (chat_bytes_ > 16000 && chat_.size() > 1) {
            const bool removed_question = chat_.front().speaker == Speaker::User;
            if (removed_question && std::none_of(chat_.begin() + 1, chat_.end(),
                    [](const ChatMessage& item) { return item.speaker == Speaker::User; })) {
                if (chat_.size() == 2) break;
                const auto oldest_detail = chat_.begin() + 1;
                chat_bytes_ -= oldest_detail->bytes;
                lv_obj_delete(oldest_detail->block);
                chat_.erase(oldest_detail);
                continue;
            }
            do {
                chat_bytes_ -= chat_.front().bytes;
                lv_obj_delete(chat_.front().block);
                chat_.pop_front();
            } while (removed_question && chat_.front().speaker != Speaker::User);
        }
        lv_obj_move_to_index(chat_hint_, -1);
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
        if (screen_ == Screen::MemoryEdit) {
            if (std::string_view(lv_textarea_get_text(memory_editor_)).size() + candidate.text.size() > MemoryEditMaxBytes)
                return;
            insert_memory_text(candidate.text);
        } else if (!insert_draft(candidate.text)) return;
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

    void scroll(Key key) {
        lv_obj_scroll_by_bounded(panel_, 0, key == Key::Up ? 25 : -25, LV_ANIM_OFF);
    }

    void insert_memory_text(const std::string& text) {
        std::string_view current = lv_textarea_get_text(memory_editor_);
        if (current.size() + text.size() <= MemoryEditMaxBytes)
            lv_textarea_add_text(memory_editor_, text.c_str());
    }

    void open_memory(const std::string& name) {
        memory_parent_ = screen_;
        memory_selection_ = selection_;
        error_.clear();
        send({{"type", "memory_read"}, {"file", name}}, "读取文件");
    }

    void memory_key(Key key) {
        if (screen_ == Screen::MemoryView) {
            if (key == Key::Escape || key == Key::Left) {
                screen_ = memory_parent_;
                selection_ = memory_selection_;
                error_.clear();
            } else if (key == Key::Enter || key == Key::Right) {
                lv_textarea_set_text(memory_editor_, memory_text_.c_str());
                lv_textarea_set_cursor_pos(memory_editor_, 0);
                clear_pinyin();
                error_.clear();
                screen_ = Screen::MemoryEdit;
            } else if (key == Key::Up || key == Key::Down) scroll(key);
            return;
        }
        if (key == Key::Escape) {
            if (!spelling_.empty()) clear_pinyin();
            else if (memory_text_ == lv_textarea_get_text(memory_editor_)) screen_ = Screen::MemoryView;
            else {
                screen_ = Screen::MemoryActions;
                selection_ = 0;
            }
        } else if (key == Key::Tab) {
            chinese_ = !chinese_;
            clear_pinyin();
        } else if (!spelling_.empty()) {
            if (key == Key::Enter) choose(0);
            else if (key == Key::Backspace) {
                spelling_.pop_back();
                query_pinyin();
            } else if (key == Key::Left && page_ > 0) --page_;
            else if (key == Key::Right && page_ + 1 < candidate_pages_.size()) ++page_;
        } else {
            switch (key) {
            case Key::Enter: insert_memory_text("\n"); break;
            case Key::Backspace: lv_textarea_delete_char(memory_editor_); break;
            case Key::Left: lv_textarea_cursor_left(memory_editor_); break;
            case Key::Right: lv_textarea_cursor_right(memory_editor_); break;
            case Key::Up: lv_textarea_cursor_up(memory_editor_); break;
            case Key::Down: lv_textarea_cursor_down(memory_editor_); break;
            default: break;
            }
        }
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
        case Screen::Menu: return Menu;
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
            if (category_ == DisplayCategory) return {dark_ ? "明暗主题 · 深色" : "明暗主题 · 浅色"};
            std::vector<std::string> rows;
            for (const auto& field : group_fields_) {
                const auto key = field.value("key", "");
                auto label = SettingLabels.value(key, "");
                if (key == "memory.enabled") label += field.value("value", "") == "true" ? " · 开" : " · 关";
                rows.push_back(std::move(label));
            }
            if (category_ == MemoryCategory) {
                rows.insert(rows.end(), MemoryFiles.begin(), MemoryFiles.end());
                rows.emplace_back("主题文件");
            }
            return rows;
        }
        case Screen::MemoryTopics: {
            std::vector<std::string> rows;
            for (const auto& file : memory_topics_) rows.push_back(file.substr(7));
            return rows;
        }
        case Screen::MemoryActions: return {"保存修改", "继续编辑", "放弃修改"};
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
        case Screen::Menu: screen_ = Screen::Chat; break;
        case Screen::Settings: screen_ = Screen::Menu; selection_ = 1; break;
        case Screen::CatalogMenu: screen_ = Screen::Menu; selection_ = 0; break;
        case Screen::CatalogList: screen_ = Screen::CatalogMenu; selection_ = catalog_kind_; break;
        case Screen::Fields:
        case Screen::ModelMode: screen_ = Screen::Settings; selection_ = category_; break;
        case Screen::Providers: screen_ = Screen::ModelMode; selection_ = 0; break;
        case Screen::MemoryTopics: screen_ = Screen::Fields; selection_ = group_fields_.size() + MemoryFiles.size(); break;
        case Screen::MemoryActions: screen_ = Screen::MemoryEdit; break;
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
            switch (selection_) {
            case 0:
                screen_ = Screen::CatalogMenu;
                selection_ = 0;
                break;
            case 1:
                screen_ = Screen::Settings;
                selection_ = 0;
                send({{"type", "settings"}}, "读取设置");
                break;
            case 2: running = false; break;
            }
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
            if (category_ == DisplayCategory) dark_ = !dark_;
            else if (selection_ < group_fields_.size()) {
                const auto& field = group_fields_[selection_];
                if (field.value("key", "") == "memory.enabled") {
                    edit_parent_ = Screen::Fields;
                    send({{"type", "save"}, {"key", "memory.enabled"},
                          {"value", field.value("value", "") == "true" ? "false" : "true"}}, "保存");
                } else open_editor(field);
            } else {
                const auto index = selection_ - group_fields_.size();
                if (index < MemoryFiles.size()) open_memory(MemoryFiles[index]);
                else {
                    memory_topics_.clear();
                    screen_ = Screen::MemoryTopics;
                    selection_ = 0;
                    error_.clear();
                    send({{"type", "memory_topics"}}, "读取主题");
                }
            }
            break;
        case Screen::MemoryTopics: open_memory(memory_topics_[selection_]); break;
        case Screen::MemoryActions:
            if (selection_ == 0) {
                error_.clear();
                send({{"type", "memory_save"}, {"file", memory_file_},
                      {"text", lv_textarea_get_text(memory_editor_)}, {"revision", memory_revision_}}, "保存");
            } else if (selection_ == 1) {
                screen_ = Screen::MemoryEdit;
                error_.clear();
            } else {
                screen_ = Screen::MemoryView;
                error_.clear();
            }
            break;
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
            return size.x <= 288;
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
        const auto background = lv_color_hex(dark_ ? 0x101b21 : 0xe7efe9);
        const auto surface = lv_color_hex(dark_ ? 0x1b2c33 : 0xf8fbf7);
        const auto foreground = lv_color_hex(dark_ ? 0xe7f4ed : 0x192c28);
        const auto accent = lv_color_hex(dark_ ? 0x82dfab : 0x176240);
        lv_obj_set_style_bg_color(lv_screen_active(), background, 0);
        lv_obj_set_style_bg_opa(lv_screen_active(), LV_OPA_COVER, 0);
        lv_obj_set_style_bg_color(panel_, surface, 0);
        lv_obj_set_style_bg_opa(panel_, LV_OPA_COVER, 0);
        lv_obj_set_style_text_color(body_, foreground, 0);
        lv_obj_set_style_text_color(composer_, foreground, 0);
        lv_obj_set_style_bg_color(composer_, dark_ ? lv_color_hex(0x233a3e) : lv_color_hex(0xd6e8dd), 0);
        lv_obj_set_style_bg_opa(composer_, LV_OPA_COVER, 0);
        for (auto* item : {title_, status_, candidates_}) lv_obj_set_style_text_color(item, accent, 0);

        std::string title = "Colibri  /  对话";
        const bool show_chat = screen_ == Screen::Chat && !chat_.empty();
        bool show_menu_rows = false;
        for (auto* row : menu_rows_) lv_obj_add_flag(row, LV_OBJ_FLAG_HIDDEN);
        std::string content;
        if (!show_chat && !starting_) {
            content = model_hint_.empty() ?
                "输入问题开始对话。\n用 Tab 切换拼音，或用 Esc 打开设置。" : model_hint_;
        }
        std::string composer = screen_ == Screen::Chat ? draft_preview() : "";
        std::string hint = "Esc 菜单  Tab 中/英  Fn+空格 语音";
        if (!spelling_.empty()) {
            hint = candidate_pages_.empty() ? "继续输入拼音" : candidate_pages_[page_].text;
            if (candidate_pages_.size() > 1) hint += " ‹ ›";
        }
        if (recording_) {
            composer = "正在录音 · 最多 60 秒";
            hint = "Fn+空格 结束   Esc 取消";
        }
        if (is_list(screen_)) {
            const auto rows = list_rows();
            switch (screen_) {
            case Screen::Menu: title = "Colibri  /  菜单"; break;
            case Screen::Settings: title = "Colibri  /  设置"; break;
            case Screen::Fields: title = "设置  /  " + CategoryNames[category_]; break;
            case Screen::ModelMode: title = "设置  /  模型"; break;
            case Screen::Providers: title = "模型  /  服务商预设"; break;
            case Screen::ModelForm: title = "模型  /  " + model_title_; break;
            case Screen::MemoryTopics: title = "记忆  /  主题文件"; break;
            case Screen::MemoryActions: title = "编辑  /  " + memory_file_; break;
            case Screen::CatalogMenu: title = "Colibri  /  MCP 与技能"; break;
            case Screen::CatalogList: title = "查看  /  " + CatalogKinds[catalog_kind_]; break;
            default: break;
            }
            content.clear();
            show_menu_rows = !rows.empty();
            const auto visible = menu_rows_.size();
            const auto max_first = rows.size() > visible ? rows.size() - visible : 0;
            const auto first = std::min(selection_ > 2 ? selection_ - 2 : 0, max_first);
            for (std::size_t i = 0; i < visible && first + i < rows.size(); ++i) {
                auto* row = menu_rows_[i];
                const bool selected = first + i == selection_;
                lv_obj_remove_flag(row, LV_OBJ_FLAG_HIDDEN);
                lv_label_set_text(row, rows[first + i].c_str());
                lv_obj_set_style_bg_opa(row, selected ? LV_OPA_COVER : LV_OPA_TRANSP, 0);
                lv_obj_set_style_bg_color(row, lv_color_hex(dark_ ? 0x356654 : 0xb5ddc7), 0);
                lv_obj_set_style_text_color(row, selected ? lv_color_hex(dark_ ? 0xf0fff7 : 0x13382b) : foreground, 0);
            }
            if (rows.empty() && screen_ == Screen::MemoryTopics) content = "暂无主题文件";
            if (rows.empty() && screen_ == Screen::CatalogList)
                content = catalog_kind_ == 0 ? "尚未配置 MCP 服务" : "没有可用技能";
            composer = screen_ == Screen::ModelForm && error_.empty() ? "OpenAI 兼容接口" : error_;
            hint = "F/X 选择  C/Enter 打开  Z/Esc 返回";
        } else if (screen_ == Screen::Edit) {
            title = "Colibri  /  编辑";
            const auto& field = edit_field_;
            const bool secret = field.value("secret", false);
            content = field.value("label", SettingLabels.value(field.value("key", ""), "")) + "\n";
            if (secret) content += field.value("value", "").empty() ? "输入新值" : "已保存值 · 输入以替换";
            else content += field.value("value", "");
            if (!error_.empty()) content += "\n" + error_;
            composer = "› " + (secret ? std::string(std::min<std::size_t>(edited_.size(), 30), '*') : tail(edited_));
            hint = "Enter 保存   Tab 清空   Esc 返回";
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
            hint = "F/X 滚动  C/Enter 编辑  Z/Esc 返回";
        } else if (screen_ == Screen::MemoryEdit) {
            title = "编辑  /  " + memory_file_;
            content.clear();
            composer = tail(spelling_);
            if (spelling_.empty()) hint = "Tab 中/英  Enter 换行  Esc 保存菜单";
        }
        if (screen_ == Screen::MemoryEdit) {
            lv_obj_add_flag(panel_, LV_OBJ_FLAG_HIDDEN);
            lv_obj_remove_flag(memory_editor_, LV_OBJ_FLAG_HIDDEN);
            lv_obj_set_style_bg_color(memory_editor_, surface, 0);
            lv_obj_set_style_text_color(memory_editor_, foreground, 0);
            lv_obj_set_style_border_color(memory_editor_, accent, 0);
        } else {
            lv_obj_remove_flag(panel_, LV_OBJ_FLAG_HIDDEN);
            lv_obj_add_flag(memory_editor_, LV_OBJ_FLAG_HIDDEN);
        }
        lv_label_set_text(title_, title.c_str());
        lv_label_set_text(status_, busy_ ? status_text_.c_str() : recording_ ? "录音中" :
                          screen_ == Screen::Chat && !model_hint_.empty() ? "未配置" : chinese_ ? "拼音" : "ABC");
        if (chat_colors_dark_ != dark_) {
            for (const auto& message : chat_) color_chat_message(message);
            chat_colors_dark_ = dark_;
        }
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
        lv_label_set_text(composer_, composer.c_str());
        lv_label_set_text(candidates_, hint.c_str());
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
        View view(worker);
#if COLIBRI_DESKTOP
        if (argc == 3 && std::string(argv[1]) == "--screenshot")
            return save_screenshot(argv[2]) ? 0 : 1;
#endif
        while (view.running && poll_input([&](Key key) { view.key(key); },
                                          [&](const std::string& text) { view.text(text); })) {
            if (!view.running) break;
            for (const auto& event : worker.receive()) view.receive(event);
            lv_timer_handler();
            std::this_thread::sleep_for(std::chrono::milliseconds(12));
        }
    } catch (const std::exception& error) {
        std::fprintf(stderr, "Colibri: %s\n", error.what());
        return 1;
    }
    return 0;
}
