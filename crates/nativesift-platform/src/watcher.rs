use std::path::Path;

use crossbeam_channel::Receiver;
use cxx::UniquePtr;
use thiserror::Error;

use crate::bridge::{ffi, EventSink};
use crate::event::ChangeEvent;

#[derive(Debug, Error)]
pub enum WatchError {
    #[error("watch root is not valid UTF-8: {0}")]
    NonUtf8Path(String),
    #[error("failed to start native watcher: {0}")]
    Start(String),
}

pub struct FsWatcher {
    inner: UniquePtr<ffi::Watcher>,
}

impl FsWatcher {
    pub fn start(root: &Path) -> Result<(Self, Receiver<ChangeEvent>), WatchError> {
        let root_str = root
            .to_str()
            .ok_or_else(|| WatchError::NonUtf8Path(root.to_string_lossy().into_owned()))?;
        let (sender, receiver) = crossbeam_channel::unbounded();
        let sink = Box::new(EventSink::new(sender));
        let inner = ffi::new_watcher(root_str, sink)
            .map_err(|error| WatchError::Start(error.what().to_owned()))?;
        Ok((Self { inner }, receiver))
    }

    pub fn stop(&mut self) {
        if let Some(watcher) = self.inner.as_mut() {
            watcher.stop();
        }
    }
}

impl Drop for FsWatcher {
    fn drop(&mut self) {
        self.stop();
    }
}
