//! Filesystem watcher using notify + debouncer.

use notify_debouncer_full::{
    new_debouncer, DebounceEventResult, Debouncer, RecommendedCache,
};
use notify::RecursiveMode;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::sync::mpsc;

/// Events emitted by the file watcher.
#[derive(Debug, Clone)]
pub enum FsEvent {
    Created(PathBuf),
    Modified(PathBuf),
    Removed(PathBuf),
    Renamed { from: PathBuf, to: PathBuf },
}

/// Watches a directory for file changes, debounces events, and sends them
/// through a tokio channel.
pub struct FileWatcher {
    _debouncer: Debouncer<notify::RecommendedWatcher, RecommendedCache>,
}

impl FileWatcher {
    /// Start watching `root` and send debounced events to the returned receiver.
    /// `debounce_ms` controls how long to wait before emitting batched events.
    pub fn start(
        root: &Path,
        debounce_ms: u64,
    ) -> anyhow::Result<(Self, mpsc::UnboundedReceiver<Vec<FsEvent>>)> {
        let (tx, rx) = mpsc::unbounded_channel();

        let event_tx = tx.clone();
        let mut debouncer = new_debouncer(
            Duration::from_millis(debounce_ms),
            None,
            move |result: DebounceEventResult| {
                match result {
                    Ok(events) => {
                        let fs_events: Vec<FsEvent> = events
                            .into_iter()
                            .flat_map(|event| classify_event(event))
                            .collect();
                        if !fs_events.is_empty() {
                            let _ = event_tx.send(fs_events);
                        }
                    }
                    Err(errors) => {
                        for e in errors {
                            tracing::error!("File watcher error: {e}");
                        }
                    }
                }
            },
        )?;

        debouncer.watch(root, RecursiveMode::Recursive)?;
        tracing::info!("Watching {}", root.display());

        Ok((Self { _debouncer: debouncer }, rx))
    }
}

fn classify_event(event: notify_debouncer_full::DebouncedEvent) -> Vec<FsEvent> {
    use notify::EventKind;

    let paths = &event.paths;
    let mut out = Vec::new();

    match event.kind {
        EventKind::Create(_) => {
            for p in paths {
                out.push(FsEvent::Created(p.clone()));
            }
        }
        EventKind::Modify(notify::event::ModifyKind::Name(
            notify::event::RenameMode::Both,
        )) => {
            if paths.len() >= 2 {
                out.push(FsEvent::Renamed {
                    from: paths[0].clone(),
                    to: paths[1].clone(),
                });
            }
        }
        EventKind::Modify(_) => {
            for p in paths {
                out.push(FsEvent::Modified(p.clone()));
            }
        }
        EventKind::Remove(_) => {
            for p in paths {
                out.push(FsEvent::Removed(p.clone()));
            }
        }
        _ => {}
    }

    out
}

/// Compute the blake3 hash of a file.
pub fn hash_file(path: &Path) -> anyhow::Result<String> {
    let data = std::fs::read(path)?;
    let hash = blake3::hash(&data);
    Ok(hash.to_hex().to_string())
}

/// Scan a directory recursively and return relative paths with their metadata.
pub fn scan_directory(root: &Path) -> anyhow::Result<Vec<LocalFileInfo>> {
    let mut results = Vec::new();
    scan_recursive(root, root, &mut results)?;
    Ok(results)
}

#[derive(Debug, Clone)]
pub struct LocalFileInfo {
    pub relative_path: String,
    pub absolute_path: PathBuf,
    pub is_directory: bool,
    pub size: u64,
    pub modified: Option<String>,
}

fn scan_recursive(root: &Path, current: &Path, results: &mut Vec<LocalFileInfo>) -> anyhow::Result<()> {
    let entries = std::fs::read_dir(current)?;
    for entry in entries {
        let entry = entry?;
        let path = entry.path();
        let metadata = entry.metadata()?;

        // Skip hidden files and our own metadata
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        if name_str.starts_with('.') || name_str == ".insyncbee" {
            continue;
        }

        let relative = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();

        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| {
                let dt: chrono::DateTime<chrono::Utc> = t.into();
                Some(dt.to_rfc3339())
            });

        results.push(LocalFileInfo {
            relative_path: relative,
            absolute_path: path.clone(),
            is_directory: metadata.is_dir(),
            size: metadata.len(),
            modified,
        });

        if metadata.is_dir() {
            scan_recursive(root, &path, results)?;
        }
    }
    Ok(())
}
