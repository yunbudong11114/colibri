#include "worker_link.hpp"
#include <cerrno>
#include <chrono>
#include <csignal>
#include <cstdlib>
#include <fcntl.h>
#include <stdexcept>
#include <sys/wait.h>
#include <thread>
#include <unistd.h>

using nlohmann::json;

WorkerLink::WorkerLink() {
    int request[2];
    int response[2];
    if (pipe(request) < 0) throw std::runtime_error("worker request pipe");
    if (pipe(response) < 0) {
        close(request[0]);
        close(request[1]);
        throw std::runtime_error("worker response pipe");
    }
    child_ = fork();
    if (child_ == 0) {
        dup2(request[0], STDIN_FILENO);
        dup2(response[1], STDOUT_FILENO);
        for (int fd : {request[0], request[1], response[0], response[1]}) close(fd);
        const char* command = std::getenv("COLIBRI_WORKER");
        if (!command) command = COLIBRI_WORKER_PATH;
        execl(command, command, static_cast<char*>(nullptr));
        _exit(127);
    }
    close(request[0]);
    close(response[1]);
    if (child_ < 0) {
        close(request[1]);
        close(response[0]);
        throw std::runtime_error("could not start worker");
    }
    input_ = request[1];
    output_ = response[0];
    fcntl(output_, F_SETFL, O_NONBLOCK);
    std::signal(SIGPIPE, SIG_IGN);
}

WorkerLink::~WorkerLink() {
    close(input_);
    if (output_ >= 0) close(output_);
    if (child_ > 0) {
        for (int attempt = 0; attempt < 50; ++attempt) {
            const auto result = waitpid(child_, nullptr, WNOHANG);
            if (result == child_ || (result < 0 && errno == ECHILD)) return;
            if (attempt == 5) kill(child_, SIGTERM);
            std::this_thread::sleep_for(std::chrono::milliseconds(20));
        }
        kill(child_, SIGKILL);
        while (waitpid(child_, nullptr, 0) < 0 && errno == EINTR) {}
    }
}

void WorkerLink::send(const json& request) {
    const std::string wire = request.dump() + "\n";
    for (std::size_t offset = 0; offset < wire.size();) {
        const auto count = write(input_, wire.data() + offset, wire.size() - offset);
        if (count < 0 && errno == EINTR) continue;
        if (count <= 0) throw std::runtime_error("worker unavailable");
        offset += static_cast<std::size_t>(count);
    }
}

std::vector<json> WorkerLink::receive() {
    std::vector<json> messages;
    if (output_ < 0) return messages;
    char buffer[8192];
    ssize_t count;
    while ((count = read(output_, buffer, sizeof(buffer))) > 0) {
        pending_.append(buffer, count);
    }
    std::size_t newline;
    while ((newline = pending_.find('\n')) != std::string::npos) {
        auto message = json::parse(pending_.substr(0, newline), nullptr, false);
        pending_.erase(0, newline + 1);
        if (message.is_object()) messages.push_back(std::move(message));
    }
    if (!count) {
        close(output_);
        output_ = -1;
        messages.push_back({{"type", "error"}, {"text", "运行时已退出"}});
    }
    return messages;
}
