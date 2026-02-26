use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::Path;
use std::sync::mpsc::{channel, Receiver, TryRecvError};

use stasis_runner::swap::contracts::{FileChangeEvent, FileChangeKind, TextSource};

pub struct WatchService {
    _watcher: RecommendedWatcher,
    rx: Receiver<notify::Result<Event>>,
    next_revision: u64,
}

impl WatchService {
    pub fn start(root: &Path) -> notify::Result<Self> {
        let (tx, rx) = channel::<notify::Result<Event>>();
        let mut watcher = RecommendedWatcher::new(
            move |res| {
                let _ = tx.send(res);
            },
            Config::default(),
        )?;
        watcher.watch(root, RecursiveMode::Recursive)?;

        Ok(Self {
            _watcher: watcher,
            rx,
            next_revision: 1,
        })
    }

    pub fn drain_stasis_changes(&mut self) -> Vec<FileChangeEvent> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(Ok(event)) => {
                    out.extend(map_notify_event(event, &mut self.next_revision));
                }
                Ok(Err(_)) => {}
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        out
    }
}

fn map_notify_event(event: Event, next_revision: &mut u64) -> Vec<FileChangeEvent> {
    let Some(change_kind) = map_event_kind(&event.kind) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    for path in event.paths {
        if is_stasis_path(&path) {
            let revision = *next_revision;
            *next_revision = revision.saturating_add(1);
            out.push(FileChangeEvent::new(
                path,
                revision,
                TextSource::FileWatcher,
                change_kind.clone(),
            ));
        }
    }
    out
}

fn map_event_kind(kind: &EventKind) -> Option<FileChangeKind> {
    match kind {
        EventKind::Create(_) => Some(FileChangeKind::Created),
        EventKind::Modify(_) => Some(FileChangeKind::Modified),
        EventKind::Remove(_) => Some(FileChangeKind::Deleted),
        _ => None,
    }
}

fn is_stasis_path(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("stasis"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use notify::event::{CreateKind, ModifyKind, RemoveKind};
    use std::path::PathBuf;

    #[test]
    fn maps_create_modify_remove_and_filters_non_stasis() {
        let mut revision = 10;

        let create = Event {
            kind: EventKind::Create(CreateKind::File),
            paths: vec![PathBuf::from("a.stasis"), PathBuf::from("note.txt")],
            attrs: Default::default(),
        };
        let modify = Event {
            kind: EventKind::Modify(ModifyKind::Any),
            paths: vec![PathBuf::from("b.STASIS")],
            attrs: Default::default(),
        };
        let remove = Event {
            kind: EventKind::Remove(RemoveKind::File),
            paths: vec![PathBuf::from("c.stasis")],
            attrs: Default::default(),
        };

        let create_events = map_notify_event(create, &mut revision);
        let modify_events = map_notify_event(modify, &mut revision);
        let remove_events = map_notify_event(remove, &mut revision);

        assert_eq!(create_events.len(), 1);
        assert_eq!(create_events[0].change_kind, FileChangeKind::Created);
        assert_eq!(create_events[0].revision, 10);

        assert_eq!(modify_events.len(), 1);
        assert_eq!(modify_events[0].change_kind, FileChangeKind::Modified);
        assert_eq!(modify_events[0].revision, 11);

        assert_eq!(remove_events.len(), 1);
        assert_eq!(remove_events[0].change_kind, FileChangeKind::Deleted);
        assert_eq!(remove_events[0].revision, 12);
    }

    #[test]
    fn ignores_non_file_change_events() {
        let mut revision = 1;
        let access_event = Event {
            kind: EventKind::Any,
            paths: vec![PathBuf::from("x.stasis")],
            attrs: Default::default(),
        };

        let mapped = map_notify_event(access_event, &mut revision);
        assert!(mapped.is_empty());
        assert_eq!(revision, 1);
    }
}
