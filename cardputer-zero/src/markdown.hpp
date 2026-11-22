#pragma once
#include <string>
#include <string_view>
#include <vector>

namespace markdown {
enum class BlockKind { Text, Heading, Quote, Code, Table, Rule };
enum Style : unsigned { Bold = 1, Italic = 2, InlineCode = 4, Link = 8, Strike = 16 };

struct Run {
    std::string text;
    unsigned style;
};

struct Block {
    BlockKind kind;
    std::vector<Run> runs;
};

std::vector<Block> parse(std::string_view source);
}
