mod bridge;
mod event;
mod watcher;

pub use event::ChangeEvent;
pub use watcher::{FsWatcher, WatchError};
