#pragma once

#include <cstring>

namespace nativesift {

inline bool is_ignored_name(const char* name) {
    return std::strcmp(name, ".git") == 0 || std::strcmp(name, ".hg") == 0 ||
           std::strcmp(name, ".svn") == 0 || std::strcmp(name, "node_modules") == 0;
}

}  // namespace nativesift
