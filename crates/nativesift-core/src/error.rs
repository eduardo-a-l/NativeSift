use nativesift_platform::WatchError;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("index error: {0}")]
    Index(#[from] tantivy::TantivyError),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Error)]
pub enum ServiceError {
    #[error(transparent)]
    Watch(#[from] WatchError),
    #[error("failed to spawn the pipeline thread: {0}")]
    Spawn(#[source] std::io::Error),
}
