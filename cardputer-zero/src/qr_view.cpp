#include "qr_view.hpp"
#include "qr_display_lease.hpp"
#include "input.hpp"
#include <lvgl.h>
#include <nlohmann/json.hpp>
#if COLIBRI_DESKTOP
#include "src/drivers/sdl/lv_sdl_window.h"
#else
#include "src/drivers/display/fb/lv_linux_fbdev.h"
#endif
#include <chrono>
#include <csignal>
#include <cstdio>
#include <iostream>
#include <poll.h>
#include <string>
#include <thread>
#include <vector>
#include <unistd.h>

namespace {
class InputLease {
public:
    InputLease() { if (!set_input_exclusive(true)) throw std::runtime_error("QR keyboard unavailable"); }
    ~InputLease() { set_input_exclusive(false); }
};
volatile std::sig_atomic_t interrupted = 0;
void interrupt(int) { interrupted = 1; }
uint32_t qr_ticks() {
    using namespace std::chrono;
    return static_cast<uint32_t>(duration_cast<milliseconds>(steady_clock::now().time_since_epoch()).count());
}
lv_obj_t* caption(lv_obj_t* root, const char* text, int y) {
    auto* label = lv_label_create(root);
    lv_label_set_text(label, text);
    lv_obj_set_pos(label, 168, y); lv_obj_set_width(label, 146);
    return label;
}
}

int run_qr_view() {
    try {
        if (isatty(STDIN_FILENO)) return 1;
        std::string line;
        // Read only one bounded setup message; the pipe then acts as a lifetime lease.
        for (int c; line.size() <= 8192 && (c = std::cin.get()) != EOF && c != '\n';) line += char(c);
        if (line.size() > 8192) return 1;
        const auto data = nlohmann::json::parse(line);
        const auto matrix = data.at("matrix").get<std::vector<std::string>>();
        const int n = static_cast<int>(matrix.size());
        if (n < 21 || n > 65) return 1;
        for (const auto& row : matrix)
            if (row.size() != matrix.size() || row.find_first_not_of("01") != std::string::npos) return 1;
        bool cancelled = false;
        const pid_t parent = getppid();
        {
            QrDisplayLease lease;
            InputLease input;
            std::signal(SIGINT, interrupt); std::signal(SIGTERM, interrupt);
            lv_init(); lv_tick_set_cb(qr_ticks);
#if COLIBRI_DESKTOP
            auto* display = lv_sdl_window_create(320, 170);
#else
            auto* display = lv_linux_fbdev_create();
            if (!display || lv_linux_fbdev_set_file(display, "/dev/fb0") != LV_RESULT_OK)
                throw std::runtime_error("QR framebuffer unavailable");
#endif
            auto* root = lv_screen_active();
            lv_obj_set_style_bg_color(root, lv_color_hex(0x101c22), 0);
            lv_obj_set_style_text_color(root, lv_color_hex(0xdbe8e3), 0);
            const auto* font = lv_freetype_font_create(
                "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
                LV_FREETYPE_FONT_RENDER_MODE_BITMAP, 12, LV_FREETYPE_FONT_STYLE_NORMAL);
            lv_obj_set_style_text_font(root, font ? font : LV_FONT_DEFAULT, 0);
            lv_obj_remove_flag(root, LV_OBJ_FLAG_SCROLLABLE);
            caption(root, "Colibri", 15);
            caption(root, "微信登录", 38);
            caption(root, "用微信扫描二维码\n并在手机上确认登录。", 64);
            caption(root, "等待扫码确认", 104);
            if (data.value("demo", false)) caption(root, "演示码 · 无法登录", 122);
            caption(root, "Esc  取消", 147);
            const int modules = n + 8;
            const int scale = 146 / modules;
            const int side = modules * scale;
            std::vector<uint16_t> pixels(side * side, 0xffff);
            for (int y = 0; y < n; ++y) for (int x = 0; x < n; ++x)
                if (matrix[y][x] == '1') for (int dy = 0; dy < scale; ++dy)
                    for (int dx = 0; dx < scale; ++dx)
                        pixels[((y + 4) * scale + dy) * side + (x + 4) * scale + dx] = 0;
            auto* canvas = lv_canvas_create(root);
            lv_canvas_set_buffer(canvas, pixels.data(), side, side, LV_COLOR_FORMAT_RGB565);
            lv_obj_set_pos(canvas, 8 + (146 - side) / 2, (170 - side) / 2);
            lv_refr_now(display);
            std::cout << "ready\n" << std::flush;
            bool running = true;
            while (running && !interrupted) {
                pollfd pipe{STDIN_FILENO, POLLIN | POLLHUP, 0};
                if (poll(&pipe, 1, 0) > 0 && (pipe.revents & (POLLHUP | POLLERR | POLLNVAL))) break;
                if (!poll_input([&](Key key) { if (key == Key::Escape) cancelled = true; },
                                [](const std::string&) {})) cancelled = true;
                if (cancelled) break;
                lv_timer_handler();
                std::this_thread::sleep_for(std::chrono::milliseconds(12));
            }
            lv_deinit();
        }
        // Equivalent to Ctrl+C in the originating CLI; no partial auth is saved.
        if (cancelled && parent > 1 && getppid() == parent) kill(parent, SIGINT);
        return cancelled ? 2 : 0;
    } catch (const std::exception& error) {
        std::fprintf(stderr, "Colibri QR view: %s\n", error.what());
        return 1;
    }
}
