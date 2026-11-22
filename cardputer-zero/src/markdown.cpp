#include "markdown.hpp"
#include "md4c.h"

#include <algorithm>
#include <array>
#include <charconv>
#include <cstdint>
#include <utility>

namespace markdown {
namespace {
constexpr std::size_t MaxBlocks = 96;
constexpr std::size_t MaxRuns = 384;
constexpr std::size_t MaxRenderedBytes = 24000;

struct List {
    bool ordered;
    unsigned next;
};

struct State {
    std::vector<Block> blocks;
    std::vector<List> lists;
    std::vector<unsigned> styles;
    std::vector<std::string> headers;
    std::size_t run_count = 0;
    std::size_t rendered_bytes = 0;
    std::size_t cell = 0;
    unsigned style = 0;
    unsigned quotes = 0;
    bool header = false;
    bool table_has_rows = false;
    bool after_bullet = false;
    bool block_open = false;
    bool overflow = false;

    BlockKind paragraph() const { return quotes ? BlockKind::Quote : BlockKind::Text; }

    void start(BlockKind kind) {
        if (blocks.size() >= MaxBlocks) {
            overflow = true;
            return;
        }
        blocks.push_back({kind, {}});
        block_open = true;
    }

    void add(std::string_view text, unsigned format) {
        if (text.empty() || overflow) return;
        if (header && !headers.empty()) {
            headers.back().append(text);
            return;
        }
        if (text.size() > MaxRenderedBytes - rendered_bytes) {
            overflow = true;
            return;
        }
        rendered_bytes += text.size();
        if (!block_open) start(paragraph());
        if (overflow) return;
        auto& runs = blocks.back().runs;
        if (!runs.empty() && runs.back().style == format) {
            runs.back().text.append(text);
        } else if (run_count < MaxRuns) {
            runs.push_back({std::string(text), format});
            ++run_count;
        } else {
            overflow = true;
        }
    }
};

int enter_block(MD_BLOCKTYPE type, void* detail, void* user) {
    auto& s = *static_cast<State*>(user);
    switch (type) {
    case MD_BLOCK_QUOTE:
        ++s.quotes;
        break;
    case MD_BLOCK_UL:
        s.lists.push_back({false, 1});
        break;
    case MD_BLOCK_OL:
        s.lists.push_back({true, static_cast<MD_BLOCK_OL_DETAIL*>(detail)->start});
        break;
    case MD_BLOCK_LI: {
        s.start(s.quotes ? BlockKind::Quote : BlockKind::ListItem);
        if (s.lists.empty()) break;
        auto& list = s.lists.back();
        const auto* item = static_cast<MD_BLOCK_LI_DETAIL*>(detail);
        std::string bullet(std::min<std::size_t>(s.lists.size() - 1, 4) * 2, ' ');
        if (item->is_task) bullet += item->task_mark == ' ' ? "[ ] " : "✓ ";
        else if (list.ordered) bullet += std::to_string(list.next++) + ". ";
        else bullet += "• ";
        s.add(bullet, Bold);
        s.after_bullet = true;
        break;
    }
    case MD_BLOCK_P:
        if (!s.after_bullet) s.start(s.paragraph());
        s.after_bullet = false;
        break;
    case MD_BLOCK_H:
        s.start(BlockKind::Heading);
        break;
    case MD_BLOCK_CODE:
        s.start(BlockKind::Code);
        break;
    case MD_BLOCK_HR:
        s.start(BlockKind::Rule);
        break;
    case MD_BLOCK_TABLE:
        s.headers.clear();
        s.table_has_rows = false;
        break;
    case MD_BLOCK_THEAD:
        s.header = true;
        break;
    case MD_BLOCK_TR:
        if (!s.header) {
            s.start(BlockKind::Table);
            s.table_has_rows = true;
            s.cell = 0;
        }
        break;
    case MD_BLOCK_TH:
        s.headers.emplace_back();
        break;
    case MD_BLOCK_TD:
        if (s.cell) s.add("\n", 0);
        if (s.cell < s.headers.size() && !s.headers[s.cell].empty()) {
            s.add(s.headers[s.cell], Bold);
            s.add("：", Bold);
        }
        ++s.cell;
        break;
    default:
        break;
    }
    return s.overflow ? -1 : 0;
}

int leave_block(MD_BLOCKTYPE type, void*, void* user) {
    auto& s = *static_cast<State*>(user);
    switch (type) {
    case MD_BLOCK_QUOTE:
        --s.quotes;
        break;
    case MD_BLOCK_UL:
    case MD_BLOCK_OL:
        s.lists.pop_back();
        break;
    case MD_BLOCK_LI:
        s.after_bullet = false;
        s.block_open = false;
        break;
    case MD_BLOCK_P:
    case MD_BLOCK_H:
    case MD_BLOCK_CODE:
    case MD_BLOCK_HR:
    case MD_BLOCK_TR:
        s.block_open = false;
        break;
    case MD_BLOCK_THEAD:
        s.header = false;
        break;
    case MD_BLOCK_TABLE:
        if (!s.table_has_rows && !s.headers.empty()) {
            s.start(BlockKind::Table);
            for (std::size_t i = 0; i < s.headers.size(); ++i) {
                if (i) s.add(" · ", 0);
                s.add(s.headers[i], Bold);
            }
            s.block_open = false;
        }
        break;
    default:
        break;
    }
    return s.overflow ? -1 : 0;
}

int enter_span(MD_SPANTYPE type, void*, void* user) {
    auto& s = *static_cast<State*>(user);
    s.styles.push_back(s.style);
    switch (type) {
    case MD_SPAN_STRONG: s.style |= Bold; break;
    case MD_SPAN_EM: s.style |= Italic; break;
    case MD_SPAN_CODE: s.style |= InlineCode; break;
    case MD_SPAN_A: s.style |= Link; break;
    case MD_SPAN_DEL: s.style |= Strike; break;
    default: break;
    }
    return 0;
}

int leave_span(MD_SPANTYPE, void*, void* user) {
    auto& s = *static_cast<State*>(user);
    s.style = s.styles.back();
    s.styles.pop_back();
    return 0;
}

std::string entity(std::string_view input) {
    constexpr std::array names{std::pair{"&amp;", "&"}, std::pair{"&lt;", "<"}, std::pair{"&gt;", ">"},
        std::pair{"&quot;", "\""}, std::pair{"&apos;", "'"}, std::pair{"&nbsp;", "\xc2\xa0"}};
    for (const auto& [name, value] : names) if (input == name) return value;
    if (input.size() < 4 || input.substr(0, 2) != "&#") return std::string(input);
    input.remove_prefix(2);
    input.remove_suffix(1);
    const bool hex = !input.empty() && (input.front() == 'x' || input.front() == 'X');
    if (hex) input.remove_prefix(1);
    std::uint32_t code = 0;
    const auto end = input.data() + input.size();
    const auto converted = std::from_chars(input.data(), end, code, hex ? 16 : 10);
    if (converted.ec != std::errc{} || converted.ptr != end || code == 0 ||
        code > 0x10ffff || (code >= 0xd800 && code <= 0xdfff)) return "�";
    std::string result;
    if (code < 0x80) result += static_cast<char>(code);
    else {
        const unsigned tails = code < 0x800 ? 1 : code < 0x10000 ? 2 : 3;
        result += static_cast<char>((tails == 1 ? 0xc0 : tails == 2 ? 0xe0 : 0xf0) | (code >> (tails * 6)));
        for (unsigned i = tails; i; --i)
            result += static_cast<char>(0x80 | ((code >> ((i - 1) * 6)) & 0x3f));
    }
    return result;
}

int add_text(MD_TEXTTYPE type, const MD_CHAR* text, MD_SIZE size, void* user) {
    auto& s = *static_cast<State*>(user);
    s.after_bullet = false;
    switch (type) {
    case MD_TEXT_NULLCHAR: s.add("�", s.style); break;
    case MD_TEXT_BR: s.add("\n", s.style); break;
    case MD_TEXT_SOFTBR: s.add("\n", s.style); break;
    case MD_TEXT_ENTITY: s.add(entity({text, size}), s.style); break;
    default: s.add({text, size}, s.style); break;
    }
    return s.overflow ? -1 : 0;
}
}

std::vector<Block> parse(std::string_view source) {
    State state;
    MD_PARSER parser{};
    parser.flags = MD_FLAG_TABLES | MD_FLAG_TASKLISTS | MD_FLAG_STRIKETHROUGH | MD_FLAG_NOHTML;
    parser.enter_block = enter_block;
    parser.leave_block = leave_block;
    parser.enter_span = enter_span;
    parser.leave_span = leave_span;
    parser.text = add_text;
    if (md_parse(source.data(), static_cast<MD_SIZE>(source.size()), &parser, &state) != 0) return {};
    return std::move(state.blocks);
}
}
