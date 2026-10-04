use crossbeam_channel::Sender;

use crate::event::ChangeEvent;

pub struct EventSink {
    sender: Sender<ChangeEvent>,
}

impl EventSink {
    pub(crate) fn new(sender: Sender<ChangeEvent>) -> Self {
        Self { sender }
    }

    fn push_event(&self, event: ffi::FsEvent) {
        if let Some(change) = ChangeEvent::from_ffi(event) {
            let _ = self.sender.send(change);
        }
    }
}

#[cxx::bridge(namespace = "nativesift")]
pub mod ffi {
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum FsEventKind {
        Created,
        Modified,
        Deleted,
        Renamed,
        Overflow,
    }

    #[derive(Debug, Clone)]
    struct FsEvent {
        kind: FsEventKind,
        path: String,
        old_path: String,
    }

    extern "Rust" {
        type EventSink;

        fn push_event(self: &EventSink, event: FsEvent);
    }

    unsafe extern "C++" {
        include!("watcher.h");

        type Watcher;

        fn new_watcher(root: &str, sink: Box<EventSink>) -> Result<UniquePtr<Watcher>>;
        fn stop(self: Pin<&mut Watcher>);
    }
}

unsafe impl Send for ffi::Watcher {}
