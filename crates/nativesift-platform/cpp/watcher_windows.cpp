#define WIN32_LEAN_AND_MEAN
#define NOMINMAX
#include <windows.h>
#include <winioctl.h>

#include <atomic>
#include <cstdint>
#include <cwchar>
#include <stdexcept>
#include <string>
#include <thread>
#include <unordered_map>
#include <utility>
#include <vector>

#include "nativesift-platform/src/bridge.rs.h"
#include "watcher.h"

namespace nativesift {
namespace detail {

namespace {

constexpr DWORD kBufferSize = 64 * 1024;
constexpr DWORDLONG kWaitSeconds = 1;
constexpr DWORD kReasonMask = USN_REASON_FILE_CREATE | USN_REASON_FILE_DELETE |
                              USN_REASON_RENAME_OLD_NAME | USN_REASON_RENAME_NEW_NAME |
                              USN_REASON_DATA_OVERWRITE | USN_REASON_DATA_EXTEND |
                              USN_REASON_DATA_TRUNCATION;
constexpr DWORD kDataChange =
    USN_REASON_DATA_OVERWRITE | USN_REASON_DATA_EXTEND | USN_REASON_DATA_TRUNCATION;

std::wstring widen(rust::Str text) {
    if (text.size() == 0) {
        return std::wstring();
    }
    const int size = MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()),
                                         nullptr, 0);
    std::wstring result(static_cast<size_t>(size), L'\0');
    MultiByteToWideChar(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), &result[0], size);
    return result;
}

rust::String narrow(const std::wstring& text) {
    if (text.empty()) {
        return rust::String();
    }
    const int size = WideCharToMultiByte(CP_UTF8, 0, text.data(), static_cast<int>(text.size()),
                                         nullptr, 0, nullptr, nullptr);
    std::string result(static_cast<size_t>(size), '\0');
    WideCharToMultiByte(CP_UTF8, 0, text.data(), static_cast<int>(text.size()), &result[0], size,
                        nullptr, nullptr);
    return rust::String::lossy(result);
}

std::string error_message(const std::string& what) {
    return what + " (win32 error " + std::to_string(GetLastError()) + ")";
}

std::wstring strip_verbatim_prefix(const std::wstring& path) {
    if (path.size() > 6 && path.compare(0, 4, L"\\\\?\\") == 0 && path[5] == L':') {
        return path.substr(4);
    }
    return path;
}

}  // namespace

class WatcherImpl {
public:
    WatcherImpl(const std::wstring& root, rust::Box<EventSink> sink) : sink_(std::move(sink)) {
        root_ = strip_verbatim_prefix(root);
        while (root_.size() > 2 && (root_.back() == L'\\' || root_.back() == L'/')) {
            root_.pop_back();
        }
        if (root_.size() < 2 || root_[1] != L':') {
            throw std::runtime_error("only local drive-letter paths are supported on Windows");
        }

        const std::wstring volume_path = L"\\\\.\\" + root_.substr(0, 2);
        volume_ = CreateFileW(volume_path.c_str(), GENERIC_READ,
                              FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE, nullptr,
                              OPEN_EXISTING, 0, nullptr);
        if (volume_ == INVALID_HANDLE_VALUE) {
            throw std::runtime_error(error_message(
                "cannot open the volume, the NTFS change journal requires administrator rights"));
        }
        if (!query_journal()) {
            const std::string message = error_message("cannot query the NTFS change journal");
            CloseHandle(volume_);
            volume_ = INVALID_HANDLE_VALUE;
            throw std::runtime_error(message);
        }
        thread_ = std::thread([this] { run(); });
    }

    ~WatcherImpl() { stop(); }

    void stop() {
        if (stopped_.exchange(true)) {
            return;
        }
        if (thread_.joinable()) {
            thread_.join();
        }
        if (volume_ != INVALID_HANDLE_VALUE) {
            CloseHandle(volume_);
            volume_ = INVALID_HANDLE_VALUE;
        }
    }

private:
    bool query_journal() {
        USN_JOURNAL_DATA journal{};
        DWORD returned = 0;
        if (!DeviceIoControl(volume_, FSCTL_QUERY_USN_JOURNAL, nullptr, 0, &journal,
                             sizeof journal, &returned, nullptr)) {
            return false;
        }
        journal_id_ = journal.UsnJournalID;
        next_usn_ = journal.NextUsn;
        return true;
    }

    bool is_under_root(const std::wstring& path) const {
        if (path.size() < root_.size() ||
            _wcsnicmp(path.c_str(), root_.c_str(), root_.size()) != 0) {
            return false;
        }
        return path.size() == root_.size() || path[root_.size()] == L'\\' ||
               root_.back() == L'\\';
    }

    std::wstring path_for_id(DWORDLONG id) {
        FILE_ID_DESCRIPTOR descriptor{};
        descriptor.dwSize = sizeof descriptor;
        descriptor.Type = FileIdType;
        descriptor.FileId.QuadPart = static_cast<LONGLONG>(id);
        HANDLE handle = OpenFileById(volume_, &descriptor, FILE_READ_ATTRIBUTES,
                                     FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
                                     nullptr, FILE_FLAG_BACKUP_SEMANTICS);
        if (handle == INVALID_HANDLE_VALUE) {
            return std::wstring();
        }
        std::wstring buffer(1024, L'\0');
        DWORD length = GetFinalPathNameByHandleW(handle, &buffer[0], static_cast<DWORD>(buffer.size()),
                                                 FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
        if (length > buffer.size()) {
            buffer.resize(length);
            length = GetFinalPathNameByHandleW(handle, &buffer[0], static_cast<DWORD>(buffer.size()),
                                               FILE_NAME_NORMALIZED | VOLUME_NAME_DOS);
        }
        CloseHandle(handle);
        if (length == 0 || length > buffer.size()) {
            return std::wstring();
        }
        buffer.resize(length);
        return strip_verbatim_prefix(buffer);
    }

    void emit(FsEventKind kind, const std::wstring& path,
              const std::wstring& old_path = std::wstring()) {
        FsEvent event{kind, narrow(path), narrow(old_path)};
        sink_->push_event(std::move(event));
    }

    void process(const USN_RECORD& record) {
        const WCHAR* name_data = reinterpret_cast<const WCHAR*>(
            reinterpret_cast<const char*>(&record) + record.FileNameOffset);
        const std::wstring name(name_data, record.FileNameLength / sizeof(WCHAR));
        std::wstring parent = path_for_id(record.ParentFileReferenceNumber);
        if (parent.empty()) {
            return;
        }
        if (parent.back() != L'\\') {
            parent += L'\\';
        }
        const std::wstring full = parent + name;
        const DWORD reason = record.Reason;

        if (reason & USN_REASON_RENAME_OLD_NAME) {
            old_names_[record.FileReferenceNumber] = full;
            return;
        }
        if (reason & USN_REASON_RENAME_NEW_NAME) {
            std::wstring old_path;
            auto found = old_names_.find(record.FileReferenceNumber);
            if (found != old_names_.end()) {
                old_path = std::move(found->second);
                old_names_.erase(found);
            }
            const bool old_inside = !old_path.empty() && is_under_root(old_path);
            const bool new_inside = is_under_root(full);
            if (old_inside && new_inside) {
                emit(FsEventKind::Renamed, full, old_path);
            } else if (old_inside) {
                emit(FsEventKind::Deleted, old_path);
            } else if (new_inside) {
                emit(FsEventKind::Created, full);
            }
            return;
        }
        if (!is_under_root(full)) {
            return;
        }
        if (reason & USN_REASON_FILE_DELETE) {
            emit(FsEventKind::Deleted, full);
        } else if (reason & USN_REASON_FILE_CREATE) {
            emit(FsEventKind::Created, full);
        } else if (reason & kDataChange) {
            emit(FsEventKind::Modified, full);
        }
    }

    void run() {
        std::vector<char> buffer(kBufferSize);
        while (!stopped_.load()) {
            READ_USN_JOURNAL_DATA request{};
            request.StartUsn = next_usn_;
            request.ReasonMask = kReasonMask;
            request.ReturnOnlyOnClose = FALSE;
            request.Timeout = kWaitSeconds;
            request.BytesToWaitFor = 1;
            request.UsnJournalID = journal_id_;

            DWORD returned = 0;
            if (!DeviceIoControl(volume_, FSCTL_READ_USN_JOURNAL, &request, sizeof request,
                                 buffer.data(), static_cast<DWORD>(buffer.size()), &returned,
                                 nullptr)) {
                const DWORD error = GetLastError();
                if (error == ERROR_JOURNAL_ENTRY_DELETED && query_journal()) {
                    emit(FsEventKind::Overflow, root_);
                    continue;
                }
                break;
            }
            if (returned < sizeof(USN)) {
                continue;
            }
            next_usn_ = *reinterpret_cast<const USN*>(buffer.data());
            DWORD offset = sizeof(USN);
            while (offset + sizeof(USN_RECORD) <= returned) {
                const auto* record = reinterpret_cast<const USN_RECORD*>(buffer.data() + offset);
                if (record->RecordLength == 0) {
                    break;
                }
                if (record->MajorVersion == 2) {
                    process(*record);
                }
                offset += record->RecordLength;
            }
        }
    }

    rust::Box<EventSink> sink_;
    std::wstring root_;
    HANDLE volume_ = INVALID_HANDLE_VALUE;
    DWORDLONG journal_id_ = 0;
    USN next_usn_ = 0;
    std::unordered_map<DWORDLONG, std::wstring> old_names_;
    std::thread thread_;
    std::atomic<bool> stopped_{false};
};

}  // namespace detail

Watcher::Watcher(rust::Str root, rust::Box<EventSink> sink)
    : impl_(std::make_unique<detail::WatcherImpl>(detail::widen(root), std::move(sink))) {}

Watcher::~Watcher() = default;

void Watcher::stop() {
    impl_->stop();
}

std::unique_ptr<Watcher> new_watcher(rust::Str root, rust::Box<EventSink> sink) {
    return std::make_unique<Watcher>(root, std::move(sink));
}

}  // namespace nativesift
