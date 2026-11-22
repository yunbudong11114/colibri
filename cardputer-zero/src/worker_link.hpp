#pragma once
#include <nlohmann/json.hpp>
#include <string>
#include <sys/types.h>
#include <vector>

class WorkerLink {
public:
    WorkerLink();
    ~WorkerLink();
    void send(const nlohmann::json& request);
    std::vector<nlohmann::json> receive();

private:
    pid_t child_ = -1;
    int input_ = -1;
    int output_ = -1;
    std::string pending_;
};
