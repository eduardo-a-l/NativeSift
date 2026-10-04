use std::path::PathBuf;

use crate::bridge::ffi::{FsEvent, FsEventKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Deleted(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
    Overflow(PathBuf),
}

impl ChangeEvent {
    pub(crate) fn from_ffi(event: FsEvent) -> Option<Self> {
        let path = PathBuf::from(event.path);
        match event.kind {
            FsEventKind::Created => Some(Self::Created(path)),
            FsEventKind::Modified => Some(Self::Modified(path)),
            FsEventKind::Deleted => Some(Self::Deleted(path)),
            FsEventKind::Renamed => Some(Self::Renamed {
                from: PathBuf::from(event.old_path),
                to: path,
            }),
            FsEventKind::Overflow => Some(Self::Overflow(path)),
            _ => None,
        }
    }

    pub fn path(&self) -> &PathBuf {
        match self {
            Self::Created(path)
            | Self::Modified(path)
            | Self::Deleted(path)
            | Self::Overflow(path) => path,
            Self::Renamed { to, .. } => to,
        }
    }
}
