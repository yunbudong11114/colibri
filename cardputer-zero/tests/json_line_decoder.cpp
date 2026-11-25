#include "json_line_decoder.hpp"
#include <cassert>
int main() {
    std::vector<nlohmann::json> rows;
    JsonLineDecoder decoder;
    const std::string wire = "{\"text\":\"中文\"}\n{\"value\":2}\n";
    for (char byte : wire) decoder.feed(std::string_view(&byte,1),rows);
    assert(rows.size()==2 && rows[0]["text"]=="中文" && rows[1]["value"]==2);
    assert(decoder.buffered_bytes()==0);
    JsonLineDecoder bounded(32); rows.clear();
    bounded.feed(std::string(200000,'x'),rows);
    assert(rows.size()==1 && rows[0]["type"]=="error");
    assert(bounded.buffered_bytes()<=32);
    bounded.feed("more junk\n{\"okay\":true}\n",rows);
    assert(rows.size()==2 && rows[1]["okay"]==true);
    bounded.feed("not-json\n{\"next\":true}\n",rows);
    assert(rows.size()==4 && rows[2]["type"]=="error" && rows[3]["next"]==true);
}
