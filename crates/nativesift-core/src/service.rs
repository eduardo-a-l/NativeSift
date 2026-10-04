use std::path::Path;
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{Receiver, RecvTimeoutError};
use nativesift_platform::{ChangeEvent, FsWatcher};

use crate::engine::Engine;
use crate::error::ServiceError;

const QUIET_PERIOD: Duration = Duration::from_millis(150);
const MAX_BATCH_LATENCY: Duration = Duration::from_secs(2);
const MAX_BATCH_EVENTS: usize = 50_000;

pub struct WatchService {
    watcher: Option<FsWatcher>,
    worker: Option<JoinHandle<()>>,
}

impl WatchService {
    pub fn start(engine: Arc<Engine>, root: &Path) -> Result<Self, ServiceError> {
        let (watcher, receiver) = FsWatcher::start(root)?;
        let worker = thread::Builder::new()
            .name("nativesift-pipeline".into())
            .spawn(move || run_pipeline(&engine, &receiver))
            .map_err(ServiceError::Spawn)?;
        Ok(Self {
            watcher: Some(watcher),
            worker: Some(worker),
        })
    }
}

impl Drop for WatchService {
    fn drop(&mut self) {
        self.watcher.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

fn run_pipeline(engine: &Engine, receiver: &Receiver<ChangeEvent>) {
    while let Ok(first) = receiver.recv() {
        let started = Instant::now();
        let mut batch = vec![first];
        while batch.len() < MAX_BATCH_EVENTS && started.elapsed() < MAX_BATCH_LATENCY {
            match receiver.recv_timeout(QUIET_PERIOD) {
                Ok(event) => batch.push(event),
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => break,
            }
        }
        let count = batch.len();
        if let Err(error) = engine.apply_events(batch) {
            log::error!("failed to apply {count} file system events: {error}");
        }
    }
}

#[cfg(all(test, any(target_os = "linux", target_os = "macos")))]
mod tests {
    use std::fs;
    use std::time::Duration;

    use super::*;
    use crate::fsutil::normalize_root;

    fn eventually(mut condition: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + Duration::from_secs(15);
        while Instant::now() < deadline {
            if condition() {
                return true;
            }
            thread::sleep(Duration::from_millis(50));
        }
        false
    }

    #[test]
    fn live_changes_flow_into_the_indexes() {
        let dir = tempfile::tempdir().unwrap();
        let root = normalize_root(dir.path()).unwrap();
        let engine = Arc::new(Engine::new().unwrap());
        let service = WatchService::start(Arc::clone(&engine), &root).unwrap();
        engine.index_tree(&root).unwrap();

        fs::write(root.join("live_note.md"), "streaming update arrives").unwrap();
        assert!(eventually(|| !engine
            .search_paths("live_note", 5)
            .is_empty()));
        assert!(eventually(|| !engine
            .search_content("streaming", 5)
            .unwrap()
            .is_empty()));

        fs::remove_file(root.join("live_note.md")).unwrap();
        assert!(eventually(|| engine
            .search_paths("live_note", 5)
            .is_empty()));
        assert!(eventually(|| engine
            .search_content("streaming", 5)
            .unwrap()
            .is_empty()));

        drop(service);
    }

    #[test]
    fn dropping_the_service_stops_the_pipeline() {
        let dir = tempfile::tempdir().unwrap();
        let root = normalize_root(dir.path()).unwrap();
        let engine = Arc::new(Engine::new().unwrap());
        let service = WatchService::start(engine, &root).unwrap();
        drop(service);
    }
}
