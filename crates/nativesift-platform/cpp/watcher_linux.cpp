#include <dirent.h>
#include <poll.h>
#include <sys/eventfd.h>
#include <sys/inotify.h>
#include <sys/stat.h>
#include <unistd.h>

#include <atomic>
#include <cerrno>
#include <cstdint>
#include <cstring>
#include <stdexcept>
#include <string>
#include <thread>
#include <unordered_map>
#include <utility>

#include "ignore.h"
#include "nativesift-platform/src/bridge.rs.h"
#include "watcher.h"

namespace nativesift {
namespace detail {

namespace {

constexpr uint32_t kWatchMask =
    IN_CREATE | IN_DELETE | IN_CLOSE_WRITE | IN_MOVED_FROM | IN_MOVED_TO | IN_ONLYDIR;
constexpr int kMoveFlushTimeoutMs = 100;
constexpr size_t kReadBufferSize = 64 * 1024;

std::string errno_message(const std::string& what) {
    return what + ": " + std::strerror(errno);
}

bool is_directory_entry(const dirent* entry, const std::string& full_path) {
    if (entry->d_type == DT_DIR) {
        return true;
    }
    if (entry->d_type != DT_UNKNOWN) {
        return false;
    }
    struct stat info;
    return lstat(full_path.c_str(), &info) == 0 && S_ISDIR(info.st_mode);
}

struct PendingMove {
    std::string path;
    bool is_dir;
};

}  // namespace

class WatcherImpl {
public:
    WatcherImpl(std::string root, rust::Box<EventSink> sink)
        : root_(std::move(root)), sink_(std::move(sink)) {
        inotify_fd_ = inotify_init1(IN_NONBLOCK | IN_CLOEXEC);
        if (inotify_fd_ < 0) {
            throw std::runtime_error(errno_message("inotify_init1 failed"));
        }
        stop_fd_ = eventfd(0, EFD_CLOEXEC | EFD_NONBLOCK);
        if (stop_fd_ < 0) {
            std::string message = errno_message("eventfd failed");
            close_fds();
            throw std::runtime_error(message);
        }
        try {
            add_tree(root_, true);
        } catch (...) {
            close_fds();
            throw;
        }
        thread_ = std::thread([this] { run(); });
    }

    ~WatcherImpl() { stop(); }

    void stop() {
        if (stopped_.exchange(true)) {
            return;
        }
        uint64_t one = 1;
        if (write(stop_fd_, &one, sizeof one) < 0) {
            stopped_.store(true);
        }
        if (thread_.joinable()) {
            thread_.join();
        }
        close_fds();
    }

private:
    void close_fds() {
        if (inotify_fd_ >= 0) {
            close(inotify_fd_);
            inotify_fd_ = -1;
        }
        if (stop_fd_ >= 0) {
            close(stop_fd_);
            stop_fd_ = -1;
        }
    }

    bool add_watch(const std::string& dir) {
        int wd = inotify_add_watch(inotify_fd_, dir.c_str(), kWatchMask);
        if (wd < 0) {
            return false;
        }
        watches_[wd] = dir;
        return true;
    }

    void add_tree(const std::string& dir, bool strict) {
        if (!add_watch(dir)) {
            if (strict) {
                throw std::runtime_error(errno_message("cannot watch " + dir));
            }
            return;
        }
        DIR* handle = opendir(dir.c_str());
        if (handle == nullptr) {
            return;
        }
        while (dirent* entry = readdir(handle)) {
            const char* name = entry->d_name;
            if (std::strcmp(name, ".") == 0 || std::strcmp(name, "..") == 0 ||
                is_ignored_name(name)) {
                continue;
            }
            std::string child = dir + "/" + name;
            if (is_directory_entry(entry, child)) {
                add_tree(child, false);
            }
        }
        closedir(handle);
    }

    void remove_watches_under(const std::string& path) {
        const std::string nested = path + "/";
        for (auto it = watches_.begin(); it != watches_.end();) {
            const std::string& watched = it->second;
            if (watched == path || watched.compare(0, nested.size(), nested) == 0) {
                inotify_rm_watch(inotify_fd_, it->first);
                it = watches_.erase(it);
            } else {
                ++it;
            }
        }
    }

    void rewrite_watch_prefix(const std::string& from, const std::string& to) {
        for (auto& entry : watches_) {
            std::string& watched = entry.second;
            if (watched == from) {
                watched = to;
            } else if (watched.size() > from.size() && watched.compare(0, from.size(), from) == 0 &&
                       watched[from.size()] == '/') {
                watched = to + watched.substr(from.size());
            }
        }
    }

    void emit(FsEventKind kind, const std::string& path,
              const std::string& old_path = std::string()) {
        FsEvent event{kind, rust::String::lossy(path), rust::String::lossy(old_path)};
        sink_->push_event(std::move(event));
    }

    void flush_pending_moves() {
        for (const auto& entry : pending_moves_) {
            emit(FsEventKind::Deleted, entry.second.path);
            if (entry.second.is_dir) {
                remove_watches_under(entry.second.path);
            }
        }
        pending_moves_.clear();
    }

    void handle(const inotify_event& event) {
        if (event.mask & IN_Q_OVERFLOW) {
            emit(FsEventKind::Overflow, root_);
            return;
        }
        if (event.mask & IN_IGNORED) {
            watches_.erase(event.wd);
            return;
        }
        auto parent = watches_.find(event.wd);
        if (parent == watches_.end()) {
            return;
        }
        std::string path = parent->second;
        if (event.len > 0 && event.name[0] != '\0') {
            path += '/';
            path += event.name;
        }
        const bool is_dir = (event.mask & IN_ISDIR) != 0;

        if (event.mask & IN_CREATE) {
            if (is_dir && !is_ignored_name(event.name)) {
                add_tree(path, false);
            }
            emit(FsEventKind::Created, path);
        } else if (event.mask & IN_DELETE) {
            emit(FsEventKind::Deleted, path);
        } else if (event.mask & IN_CLOSE_WRITE) {
            emit(FsEventKind::Modified, path);
        } else if (event.mask & IN_MOVED_FROM) {
            pending_moves_[event.cookie] = PendingMove{path, is_dir};
        } else if (event.mask & IN_MOVED_TO) {
            auto pending = pending_moves_.find(event.cookie);
            if (pending != pending_moves_.end()) {
                if (is_dir) {
                    rewrite_watch_prefix(pending->second.path, path);
                }
                emit(FsEventKind::Renamed, path, pending->second.path);
                pending_moves_.erase(pending);
            } else {
                if (is_dir && !is_ignored_name(event.name)) {
                    add_tree(path, false);
                }
                emit(FsEventKind::Created, path);
            }
        }
    }

    void drain() {
        alignas(inotify_event) char buffer[kReadBufferSize];
        ssize_t length;
        while ((length = read(inotify_fd_, buffer, sizeof buffer)) > 0) {
            char* cursor = buffer;
            while (cursor < buffer + length) {
                const auto* event = reinterpret_cast<const inotify_event*>(cursor);
                handle(*event);
                cursor += sizeof(inotify_event) + event->len;
            }
        }
    }

    void run() {
        pollfd fds[2] = {{inotify_fd_, POLLIN, 0}, {stop_fd_, POLLIN, 0}};
        while (true) {
            int timeout = pending_moves_.empty() ? -1 : kMoveFlushTimeoutMs;
            int ready = poll(fds, 2, timeout);
            if (ready < 0) {
                if (errno == EINTR) {
                    continue;
                }
                break;
            }
            if (ready == 0) {
                flush_pending_moves();
                continue;
            }
            if (fds[1].revents & POLLIN) {
                break;
            }
            if (fds[0].revents & POLLIN) {
                drain();
            }
        }
    }

    int inotify_fd_ = -1;
    int stop_fd_ = -1;
    std::string root_;
    rust::Box<EventSink> sink_;
    std::unordered_map<int, std::string> watches_;
    std::unordered_map<uint32_t, PendingMove> pending_moves_;
    std::thread thread_;
    std::atomic<bool> stopped_{false};
};

}  // namespace detail

Watcher::Watcher(rust::Str root, rust::Box<EventSink> sink)
    : impl_(std::make_unique<detail::WatcherImpl>(std::string(root), std::move(sink))) {}

Watcher::~Watcher() = default;

void Watcher::stop() {
    impl_->stop();
}

std::unique_ptr<Watcher> new_watcher(rust::Str root, rust::Box<EventSink> sink) {
    return std::make_unique<Watcher>(root, std::move(sink));
}

}  // namespace nativesift
