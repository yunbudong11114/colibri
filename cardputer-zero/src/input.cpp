#include "input.hpp"
#include <chrono>
#include <optional>

namespace {
using EscapeClock = std::chrono::steady_clock;
constexpr auto EscapeHold = std::chrono::milliseconds(1500);
std::optional<EscapeClock::time_point> escape_pressed;

bool escape_expired() {
    return escape_pressed && EscapeClock::now() - *escape_pressed >= EscapeHold;
}

bool handle_escape(int value, const KeyHandler& key) {
    if (value == 1 && !escape_pressed) escape_pressed = EscapeClock::now();
    const bool expired = escape_expired();
    if (value == 0 && escape_pressed) {
        escape_pressed.reset();
        if (!expired) key(Key::Escape);
    }
    return !expired;
}
}

#if COLIBRI_DESKTOP
#include <SDL.h>
#include <deque>
#include <mutex>

namespace {
std::deque<SDL_Event> events;
std::mutex events_mutex;
int enqueue(void*, SDL_Event* event) {
    const bool closed = event->type == SDL_WINDOWEVENT && event->window.event == SDL_WINDOWEVENT_CLOSE;
    const bool escape_released = event->type == SDL_KEYUP && event->key.keysym.sym == SDLK_ESCAPE;
    if (event->type == SDL_TEXTINPUT || event->type == SDL_KEYDOWN ||
        escape_released || event->type == SDL_QUIT || closed) {
        std::lock_guard lock(events_mutex);
        events.push_back(*event);
    }
    return 0;
}
}

bool set_input_exclusive(bool) { return true; }

bool poll_input(const KeyHandler& key, const TextHandler& text, const InputFilter& filter) {
    static const bool ready = [] {
        SDL_AddEventWatch(enqueue, nullptr);
        SDL_StartTextInput();
        return true;
    }();
    (void)ready;
    std::deque<SDL_Event> pending;
    {
        std::lock_guard lock(events_mutex);
        pending.swap(events);
    }
    static bool suppress_text = false;
    for (const auto& event : pending) {
        if (event.type == SDL_QUIT || event.type == SDL_WINDOWEVENT) return false;
        if (event.type == SDL_KEYDOWN || event.type == SDL_KEYUP) {
            const bool consumed = filter && filter(event.key.keysym.scancode,
                event.type == SDL_KEYUP ? 0 : event.key.repeat ? 2 : 1);
            if (event.type == SDL_KEYDOWN) suppress_text = consumed;
            if (consumed) continue;
        }
        if (event.type == SDL_TEXTINPUT && !suppress_text) text(event.text.text);
        if ((event.type == SDL_KEYDOWN || event.type == SDL_KEYUP) && event.key.keysym.sym == SDLK_ESCAPE) {
            if (!handle_escape(event.type == SDL_KEYUP ? 0 : event.key.repeat ? 2 : 1, key)) return false;
            continue;
        }
        if (event.type != SDL_KEYDOWN) continue;
        switch (event.key.keysym.sym) {
        case SDLK_UP: key(Key::Up); break;
        case SDLK_DOWN: key(Key::Down); break;
        case SDLK_LEFT: key(Key::Left); break;
        case SDLK_RIGHT: key(Key::Right); break;
        case SDLK_RETURN: if (!event.key.repeat) key(Key::Enter); break;
        case SDLK_SPACE:
            if (!event.key.repeat && (event.key.keysym.mod & KMOD_ALT)) key(Key::Voice);
            break;
        case SDLK_PAGEUP: if (!event.key.repeat) key(Key::ChatTop); break;
        case SDLK_PAGEDOWN: if (!event.key.repeat) key(Key::ChatBottom); break;
        case SDLK_HOME: if (!event.key.repeat) key(Key::DraftStart); break;
        case SDLK_END: if (!event.key.repeat) key(Key::DraftEnd); break;
        case SDLK_BACKSPACE: key(Key::Backspace); break;
        case SDLK_TAB: key(Key::Tab); break;
        }
    }
    return !escape_expired();
}

#else
#include <cstdlib>
#include <dirent.h>
#include <fcntl.h>
#include <linux/input.h>
#include <string_view>
#include <sys/ioctl.h>
#include <unistd.h>

namespace {
struct Keyboard {
    int fd;
    bool cardputer;
};

Keyboard keyboard() {
    constexpr char device[] = "/dev/input/by-path/platform-3f804000.i2c-event";
    const int flags = O_RDONLY | O_NONBLOCK | O_CLOEXEC;
    if (const char* path = std::getenv("COLIBRI_KEYBOARD"))
        return {open(path, flags), std::getenv("COLIBRI_STANDARD_KEYMAP") == nullptr};
    if (const int fd = open(device, flags); fd >= 0) return {fd, true};
    DIR* directory = opendir("/dev/input");
    if (!directory) return {-1, false};
    int fd = -1;
    while (const auto* item = readdir(directory)) {
        if (std::string_view(item->d_name).find("event") != 0) continue;
        const auto path = std::string("/dev/input/") + item->d_name;
        fd = open(path.c_str(), flags);
        if (fd < 0) continue;
        unsigned char bits[(KEY_MAX + 8) / 8] = {};
        if (ioctl(fd, EVIOCGBIT(EV_KEY, sizeof(bits)), bits) >= 0 &&
            (bits[KEY_Q / 8] & (1 << (KEY_Q % 8))) &&
            (bits[KEY_ENTER / 8] & (1 << (KEY_ENTER % 8)))) break;
        close(fd);
        fd = -1;
    }
    closedir(directory);
    return {fd, false};
}

const Keyboard& input_device() {
    static const auto device = keyboard();
    return device;
}

char character(unsigned code, bool shift, bool caps, bool cardputer) {
    // OEM symbol events documented by CardputerZero/launcher (M5Stack, MIT).
    static constexpr unsigned symbol_codes[] = {
        26, 27, 39, 40, 41, 43, 51, 52, 53, 94, 55, 69, 70, 71, 72, 73,
        74, 75, 76, 77, 79, 80, 81, 82, 83, 85, 86, 89, 90, 91, 92, 93,
    };
    static constexpr char symbols[] = "!@#$%^&*()~`_-+=[]{};:'\"<>\\|,./?";
    static_assert(sizeof(symbol_codes) / sizeof(*symbol_codes) == sizeof(symbols) - 1);
    if (cardputer) {
        for (std::size_t i = 0; i < sizeof(symbols) - 1; ++i)
            if (symbol_codes[i] == code) return symbols[i];
    }
    static const char letters[] = "qwertyuiop\0\0\0\0asdfghjkl\0\0\0\0\0zxcvbnm";
    if (code >= KEY_Q && code <= KEY_M) {
        char letter = letters[code - KEY_Q];
        if (letter) return (shift != caps) ? letter - 'a' + 'A' : letter;
    }
    if (code >= KEY_1 && code <= KEY_0)
        return (shift ? "!@#$%^&*()" : "1234567890")[code - KEY_1];
    switch (code) {
    case KEY_SPACE: return ' ';
    case KEY_MINUS: return shift ? '_' : '-';
    case KEY_EQUAL: return shift ? '+' : '=';
    case KEY_LEFTBRACE: return shift ? '{' : '[';
    case KEY_RIGHTBRACE: return shift ? '}' : ']';
    case KEY_BACKSLASH: return shift ? '|' : '\\';
    case KEY_SEMICOLON: return shift ? ':' : ';';
    case KEY_APOSTROPHE: return shift ? '"' : '\'';
    case KEY_GRAVE: return shift ? '~' : '`';
    case KEY_COMMA: return shift ? '<' : ',';
    case KEY_DOT: return shift ? '>' : '.';
    case KEY_SLASH: return shift ? '?' : '/';
    default: return 0;
    }
}
}

bool set_input_exclusive(bool exclusive) {
    return input_device().fd >= 0 && ioctl(input_device().fd, EVIOCGRAB, exclusive ? 1 : 0) == 0;
}

bool poll_input(const KeyHandler& key, const TextHandler& text, const InputFilter& filter) {
    const auto& device = input_device();
    const int fd = device.fd;
    static bool left_shift = false, right_shift = false, caps = false, fn = false;
    input_event event{};
    while (fd >= 0 && read(fd, &event, sizeof(event)) == sizeof(event)) {
        if (event.type != EV_KEY) continue;
        if (filter && filter(event.code, event.value)) continue;
        if (event.code == KEY_ESC) {
            if (!handle_escape(event.value, key)) return false;
            continue;
        }
        // The Zero's keypad reports its physical Fn as KEY_DVD on the current firmware.
        if (event.code == KEY_FN || (device.cardputer && event.code == KEY_DVD)) {
            fn = event.value != 0;
            continue;
        }
        if (event.code == KEY_LEFTSHIFT) left_shift = event.value;
        if (event.code == KEY_RIGHTSHIFT) right_shift = event.value;
        if (event.code == KEY_CAPSLOCK && event.value == 1) caps = !caps;
        if (!event.value) continue;
        if (fn) {
            std::optional<Key> shortcut;
            switch (event.code) {
            case KEY_L: shortcut = Key::ChatTop; break;
            case KEY_M: shortcut = Key::ChatBottom; break;
            case KEY_K: shortcut = Key::DraftStart; break;
            case KEY_N: shortcut = Key::DraftEnd; break;
            }
            if (shortcut) {
                if (event.value == 1) key(*shortcut);
                continue;
            }
        }
        switch (event.code) {
        case KEY_UP: key(Key::Up); break;
        case KEY_DOWN: key(Key::Down); break;
        case KEY_LEFT: key(Key::Left); break;
        case KEY_RIGHT: key(Key::Right); break;
        case KEY_ENTER: if (event.value == 1) key(Key::Enter); break;
        case KEY_SPACE:
            if (fn) {
                if (event.value == 1) key(Key::Voice);
            } else text(" ");
            break;
        case KEY_PAGEUP: if (event.value == 1) key(Key::ChatTop); break;
        case KEY_PAGEDOWN: if (event.value == 1) key(Key::ChatBottom); break;
        case KEY_HOME: if (event.value == 1) key(Key::DraftStart); break;
        case KEY_END: if (event.value == 1) key(Key::DraftEnd); break;
        case KEY_BACKSPACE: key(Key::Backspace); break;
        case KEY_TAB: if (event.value == 1) key(Key::Tab); break;
        default:
            if (char c = character(event.code, left_shift || right_shift, caps, device.cardputer)) text(std::string(1, c));
        }
    }
    return !escape_expired();
}
#endif
