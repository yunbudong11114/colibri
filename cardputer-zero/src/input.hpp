#pragma once
#include <functional>
#include <string>

enum class Key { Up, Down, Left, Right, Enter, Backspace, Escape, Tab, Voice, ChatTop, ChatBottom, DraftStart, DraftEnd };
using KeyHandler = std::function<void(Key)>;
using TextHandler = std::function<void(const std::string&)>;
// Return true to consume a physical key before shortcut/text mapping.
using InputFilter = std::function<bool(unsigned, int)>;
bool poll_input(const KeyHandler& key, const TextHandler& text, const InputFilter& filter = {});

// Temporary exclusive ownership for a CLI overlay; released on process exit too.
bool set_input_exclusive(bool exclusive);
