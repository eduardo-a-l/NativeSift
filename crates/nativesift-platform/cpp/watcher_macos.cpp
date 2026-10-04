#include <CoreServices/CoreServices.h>
#include <dispatch/dispatch.h>
#include <sys/stat.h>

#include <stdexcept>
#include <string>
#include <utility>

#include "nativesift-platform/src/bridge.rs.h"
#include "watcher.h"

namespace nativesift {
namespace detail {

namespace {

constexpr CFTimeInterval kLatencySeconds = 0.05;

void drain_noop(void*) {}

}  // namespace

class WatcherImpl {
public:
    WatcherImpl(std::string root, rust::Box<EventSink> sink)
        : root_(std::move(root)), sink_(std::move(sink)) {
        CFStringRef path = CFStringCreateWithBytes(
            kCFAllocatorDefault, reinterpret_cast<const UInt8*>(root_.data()),
            static_cast<CFIndex>(root_.size()), kCFStringEncodingUTF8, false);
        if (path == nullptr) {
            throw std::runtime_error("watch root is not valid UTF-8");
        }
        CFArrayRef paths = CFArrayCreate(kCFAllocatorDefault,
                                         reinterpret_cast<const void**>(&path), 1,
                                         &kCFTypeArrayCallBacks);
        CFRelease(path);
        if (paths == nullptr) {
            throw std::runtime_error("failed to allocate FSEvents path list");
        }

        FSEventStreamContext context = {0, this, nullptr, nullptr, nullptr};
        stream_ = FSEventStreamCreate(
            kCFAllocatorDefault, &WatcherImpl::on_events, &context, paths,
            kFSEventStreamEventIdSinceNow, kLatencySeconds,
            kFSEventStreamCreateFlagFileEvents | kFSEventStreamCreateFlagNoDefer);
        CFRelease(paths);
        if (stream_ == nullptr) {
            throw std::runtime_error("FSEventStreamCreate failed");
        }

        queue_ = dispatch_queue_create("nativesift.fsevents", DISPATCH_QUEUE_SERIAL);
        FSEventStreamSetDispatchQueue(stream_, queue_);
        if (!FSEventStreamStart(stream_)) {
            teardown();
            throw std::runtime_error("FSEventStreamStart failed");
        }
    }

    ~WatcherImpl() { stop(); }

    void stop() { teardown(); }

private:
    static void on_events(ConstFSEventStreamRef, void* info, size_t count, void* paths,
                          const FSEventStreamEventFlags flags[], const FSEventStreamEventId[]) {
        static_cast<WatcherImpl*>(info)->handle(count, static_cast<char**>(paths), flags);
    }

    void teardown() {
        if (stream_ != nullptr) {
            FSEventStreamStop(stream_);
            FSEventStreamInvalidate(stream_);
            FSEventStreamRelease(stream_);
            stream_ = nullptr;
        }
        if (queue_ != nullptr) {
            dispatch_sync_f(queue_, nullptr, drain_noop);
            dispatch_release(queue_);
            queue_ = nullptr;
        }
    }

    void emit(FsEventKind kind, const std::string& path) {
        FsEvent event{kind, rust::String::lossy(path), rust::String()};
        sink_->push_event(std::move(event));
    }

    void handle(size_t count, char** paths, const FSEventStreamEventFlags* flags) {
        constexpr FSEventStreamEventFlags kDropped = kFSEventStreamEventFlagMustScanSubDirs |
                                                     kFSEventStreamEventFlagUserDropped |
                                                     kFSEventStreamEventFlagKernelDropped;
        constexpr FSEventStreamEventFlags kNew =
            kFSEventStreamEventFlagItemCreated | kFSEventStreamEventFlagItemRenamed;

        for (size_t i = 0; i < count; ++i) {
            const std::string path = paths[i];
            if (flags[i] & kDropped) {
                emit(FsEventKind::Overflow, root_);
                continue;
            }
            struct stat info;
            if (lstat(path.c_str(), &info) != 0) {
                emit(FsEventKind::Deleted, path);
            } else if (flags[i] & kNew) {
                emit(FsEventKind::Created, path);
            } else {
                emit(FsEventKind::Modified, path);
            }
        }
    }

    std::string root_;
    rust::Box<EventSink> sink_;
    FSEventStreamRef stream_ = nullptr;
    dispatch_queue_t queue_ = nullptr;
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
