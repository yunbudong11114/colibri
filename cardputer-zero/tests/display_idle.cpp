#include "display_idle.hpp"
#include <cassert>
#include <filesystem>
#include <fstream>
#include <unistd.h>
using namespace colibri_ui;
namespace fs = std::filesystem;
void write(const fs::path& path, const char* value) { std::ofstream(path) << value; }
int read(const fs::path& path) { int value=-1; std::ifstream(path)>>value; return value; }
int main() {
    IdlePolicy keys(0);
    assert(!keys.due(29999,30) && keys.due(30000,30));
    assert(!keys.due(999999,0));
    assert(!keys.input(42,1,30000));
    assert(!keys.due(70000,30)); // Holding a modifier prevents sleep.
    keys.input(42,0,70000);
    assert(!keys.due(99999,30) && keys.due(100000,30));
    keys.sleep();
    assert(keys.input(464,1,100001)); // Fn wake chord is fully swallowed.
    assert(keys.input(57,1,100002));
    assert(keys.input(57,2,100003));
    assert(keys.input(57,0,100004));
    assert(keys.input(464,0,100005));
    assert(!keys.input(28,1,100006));
    IdlePolicy wrapped(0xfffffff0u);
    assert(wrapped.due(1000,1));
    const auto root=fs::temp_directory_path()/ ("colibri-display-idle-"+std::to_string(getpid()));
    fs::create_directories(root/"backlight");
    const auto config=root/"config.json", brightness=root/"backlight/brightness";
    write(root/"backlight/max_brightness","100"); write(brightness,"42");
    {
        DisplayIdle display(0,true,config,root/"backlight");
        display.update(29999); assert(!display.sleeping() && read(brightness)==42);
        display.update(30000); assert(display.sleeping() && read(brightness)==0);
        assert(display.input(1,1,30001)); assert(!display.sleeping() && read(brightness)==42);
        assert(display.input(1,0,30002)); assert(!display.input(28,1,30003));
        display.input(28,0,30004);
        display.update(60004); assert(display.sleeping());
    }
    assert(read(brightness)==42); // Exit restores the original brightness.
    write(config,"{\"dark_time\":10}");
    {
        DisplayIdle display(0,true,config,root/"backlight");
        display.update(10000); assert(display.sleeping());
        write(config,"{\"dark_time\":0}");
        display.update(15000); assert(!display.sleeping() && read(brightness)==42);
        display.update(999999); assert(!display.sleeping());
    }
    write(config,"{\"dark_time\":\"10\",\"brightness\":60}");write(brightness,"0");
    {
        DisplayIdle display(0,true,config,root/"backlight");
        assert(read(brightness)==60);
        display.update(10000); assert(display.sleeping());
    }
    assert(read(brightness)==60);
    write(config,"{\"dark_time\":-1}");
    {
        DisplayIdle display(0,true,config,root/"backlight");
        display.update(10000); assert(!display.sleeping());
        display.update(30000); assert(display.sleeping());
    }
    {
        DisplayIdle disabled(0,false,config,root/"backlight");
        disabled.update(999999);assert(!disabled.sleeping());
        DisplayIdle unavailable(0,true,config,root/"missing");
        unavailable.update(30000);assert(!unavailable.sleeping());
    }
    fs::remove_all(root);
}
