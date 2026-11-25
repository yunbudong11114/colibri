#pragma once
#include <stdexcept>
#if !COLIBRI_DESKTOP && defined(__linux__)
#include <cerrno>
#include <csignal>
#include <fstream>
#include <fcntl.h>
#include <linux/fb.h>
#include <sstream>
#include <string>
#include <sys/ioctl.h>
#include <sys/stat.h>
#include <sys/syscall.h>
#include <sys/wait.h>
#include <unistd.h>
#include <vector>

// A detached guardian restores the CLI even when the view is killed with SIGKILL.
// It only pauses the exact, same-user Launcher found in our own ancestry.
class QrDisplayLease {
    int control_ = -1;
    pid_t guardian_ = -1;
    static bool signal(int fd, int value) {
        return syscall(SYS_pidfd_send_signal, fd, value, nullptr, 0) == 0;
    }
    static pid_t launcher_ancestor() {
        pid_t pid = getppid();
        for (int depth = 0; pid > 1 && depth < 32; ++depth) {
            const std::string root = "/proc/" + std::to_string(pid);
            char target[1024];
            const auto n = readlink((root + "/exe").c_str(), target, sizeof(target));
            struct stat info{};
            if (n > 0 && std::string(target, n) == "/usr/share/APPLaunch/bin/M5CardputerZero-APPLaunch" &&
                stat(root.c_str(), &info) == 0 && info.st_uid == geteuid()) return pid;
            std::ifstream status(root + "/status");
            std::string line; pid_t parent = 0;
            while (std::getline(status, line))
                if (line.rfind("PPid:", 0) == 0) { std::istringstream(line.substr(5)) >> parent; break; }
            if (parent == pid) break;
            pid = parent;
        }
        return -1;
    }
public:
    QrDisplayLease() {
        const pid_t launcher = launcher_ancestor();
        if (launcher < 0) throw std::runtime_error("QR view requires the native Launcher CLI");
        const int pidfd = syscall(SYS_pidfd_open, launcher, 0);
        const int fb = open("/dev/fb0", O_RDWR | O_CLOEXEC);
        fb_var_screeninfo variable{}; fb_fix_screeninfo fixed{};
        if (pidfd < 0 || fb < 0 || ioctl(fb, FBIOGET_VSCREENINFO, &variable) < 0 ||
            ioctl(fb, FBIOGET_FSCREENINFO, &fixed) < 0 || variable.xres != 320 ||
            variable.yres != 170 || variable.bits_per_pixel != 16 || fixed.line_length != 640 ||
            variable.xoffset || variable.yoffset) {
            if (pidfd >= 0) close(pidfd);
            if (fb >= 0) close(fb);
            throw std::runtime_error("QR view cannot acquire the 320x170 display");
        }
        std::vector<char> saved(320 * 170 * 2);
        if (pread(fb, saved.data(), saved.size(), 0) != static_cast<ssize_t>(saved.size())) {
            close(fb); close(pidfd); throw std::runtime_error("QR display snapshot failed");
        }
        int brightness = -1;
        std::ifstream("/sys/class/backlight/backlight/brightness") >> brightness;
        int control[2], ready[2];
        if (pipe2(control, O_CLOEXEC) < 0) { close(fb); close(pidfd); throw std::runtime_error("QR control pipe failed"); }
        if (pipe2(ready, O_CLOEXEC) < 0) {
            close(control[0]); close(control[1]); close(fb); close(pidfd);
            throw std::runtime_error("QR ready pipe failed");
        }
        guardian_ = fork();
        if (guardian_ == 0) {
            setsid();
            std::signal(SIGINT, SIG_IGN); std::signal(SIGTERM, SIG_IGN);
            close(control[1]); close(ready[0]);
            // Never retain the authentication pipe or terminal output descriptors.
            close(STDIN_FILENO); close(STDOUT_FILENO); close(STDERR_FILENO);
            const bool paused = signal(pidfd, SIGSTOP);
            const char ok = paused ? 1 : 0;
            (void)write(ready[1], &ok, 1); close(ready[1]);
            char byte;
            if (paused) {
                while (true) {
                    const auto count = read(control[0], &byte, 1);
                    if (count == 0 || (count < 0 && errno != EINTR)) break;
                }
                (void)pwrite(fb, saved.data(), saved.size(), 0);
                if (brightness >= 0) std::ofstream("/sys/class/backlight/backlight/brightness") << brightness;
                signal(pidfd, SIGCONT);
            }
            _exit(paused ? 0 : 1);
        }
        close(control[0]); close(ready[1]); close(fb); close(pidfd);
        if (guardian_ < 0) {
            close(control[1]); close(ready[0]); throw std::runtime_error("QR guardian failed");
        }
        control_ = control[1]; char ok = 0;
        ssize_t count;
        do { count = read(ready[0], &ok, 1); } while (count < 0 && errno == EINTR);
        close(ready[0]);
        if (count != 1 || !ok) {
            close(control_); control_ = -1;
            while (waitpid(guardian_, nullptr, 0) < 0 && errno == EINTR) {}
            guardian_ = -1;
            throw std::runtime_error("Launcher display lease unavailable");
        }
    }
    ~QrDisplayLease() {
        if (control_ >= 0) close(control_);
        if (guardian_ > 0) while (waitpid(guardian_, nullptr, 0) < 0 && errno == EINTR) {}
    }
    QrDisplayLease(const QrDisplayLease&) = delete;
    QrDisplayLease& operator=(const QrDisplayLease&) = delete;
};
#else
class QrDisplayLease {};
#endif
