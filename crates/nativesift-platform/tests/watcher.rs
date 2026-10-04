#![cfg(any(target_os = "linux", target_os = "macos"))]

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use nativesift_platform::{ChangeEvent, FsWatcher};

const TIMEOUT: Duration = Duration::from_secs(10);

fn wait_for(receiver: &Receiver<ChangeEvent>, predicate: impl Fn(&ChangeEvent) -> bool) -> bool {
    let deadline = Instant::now() + TIMEOUT;
    while let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
        match receiver.recv_timeout(remaining) {
            Ok(event) if predicate(&event) => return true,
            Ok(_) => continue,
            Err(_) => return false,
        }
    }
    false
}

fn has_name(path: &Path, name: &str) -> bool {
    path.file_name().is_some_and(|file| file == name)
}

fn watched_root() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("create temp dir");
    let root = fs::canonicalize(dir.path()).expect("canonicalize temp dir");
    (dir, root)
}

#[test]
fn reports_created_and_modified_files() {
    let (_guard, root) = watched_root();
    let (_watcher, events) = FsWatcher::start(&root).expect("start watcher");

    fs::write(root.join("hello.txt"), "hello").expect("write file");

    assert!(wait_for(&events, |event| {
        matches!(event, ChangeEvent::Created(path) if has_name(path, "hello.txt"))
    }));
    assert!(wait_for(&events, |event| {
        matches!(event, ChangeEvent::Modified(path) if has_name(path, "hello.txt"))
    }));
}

#[test]
fn reports_deleted_files() {
    let (_guard, root) = watched_root();
    let target = root.join("doomed.txt");
    fs::write(&target, "bye").expect("write file");
    let (_watcher, events) = FsWatcher::start(&root).expect("start watcher");

    fs::remove_file(&target).expect("remove file");

    assert!(wait_for(&events, |event| {
        matches!(event, ChangeEvent::Deleted(path) if has_name(path, "doomed.txt"))
    }));
}

#[test]
fn reports_renamed_files() {
    let (_guard, root) = watched_root();
    let original = root.join("before.txt");
    fs::write(&original, "data").expect("write file");
    let (_watcher, events) = FsWatcher::start(&root).expect("start watcher");

    fs::rename(&original, root.join("after.txt")).expect("rename file");

    assert!(wait_for(&events, |event| match event {
        ChangeEvent::Renamed { to, .. } | ChangeEvent::Created(to) => has_name(to, "after.txt"),
        _ => false,
    }));
}

#[test]
fn follows_new_subdirectories() {
    let (_guard, root) = watched_root();
    let (_watcher, events) = FsWatcher::start(&root).expect("start watcher");

    let nested = root.join("nested");
    fs::create_dir(&nested).expect("create dir");
    assert!(wait_for(&events, |event| {
        matches!(event, ChangeEvent::Created(path) if has_name(path, "nested"))
    }));

    fs::write(nested.join("inner.txt"), "inner").expect("write file");
    assert!(wait_for(&events, |event| {
        matches!(event, ChangeEvent::Created(path) if has_name(path, "inner.txt"))
    }));
}

#[test]
fn follows_directory_renames() {
    let (_guard, root) = watched_root();
    let before = root.join("old_dir");
    fs::create_dir(&before).expect("create dir");
    let (_watcher, events) = FsWatcher::start(&root).expect("start watcher");

    let after = root.join("new_dir");
    fs::rename(&before, &after).expect("rename dir");
    assert!(wait_for(&events, |event| match event {
        ChangeEvent::Renamed { to, .. } | ChangeEvent::Created(to) => has_name(to, "new_dir"),
        _ => false,
    }));

    fs::write(after.join("file.txt"), "x").expect("write file");
    assert!(wait_for(&events, |event| {
        matches!(event, ChangeEvent::Created(path) if has_name(path, "file.txt"))
    }));
}

#[cfg(target_os = "linux")]
#[test]
fn rejects_missing_root() {
    assert!(FsWatcher::start(Path::new("/definitely/not/a/real/path")).is_err());
}

#[test]
fn stops_cleanly_on_drop() {
    let (_guard, root) = watched_root();
    let (watcher, events) = FsWatcher::start(&root).expect("start watcher");
    drop(watcher);
    assert!(events.recv_timeout(Duration::from_millis(200)).is_err());
}
