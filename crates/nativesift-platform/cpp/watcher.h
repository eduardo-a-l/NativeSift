#pragma once

#include <memory>

#include "rust/cxx.h"

namespace nativesift {

struct EventSink;

namespace detail {
class WatcherImpl;
}

class Watcher {
public:
    Watcher(rust::Str root, rust::Box<EventSink> sink);
    ~Watcher();
    Watcher(const Watcher&) = delete;
    Watcher& operator=(const Watcher&) = delete;

    void stop();

private:
    std::unique_ptr<detail::WatcherImpl> impl_;
};

std::unique_ptr<Watcher> new_watcher(rust::Str root, rust::Box<EventSink> sink);

}  // namespace nativesift
