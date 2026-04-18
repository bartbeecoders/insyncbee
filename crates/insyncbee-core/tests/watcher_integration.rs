//! Integration tests for `watcher::FileWatcher` and the helpers it exposes.

use insyncbee_core::watcher::{self, FileWatcher, FsEvent};
use std::time::Duration;
use tempfile::TempDir;

#[test]
fn scan_directory_skips_dotfiles_and_returns_relative_paths() {
    let tmp = TempDir::new().unwrap();
    std::fs::write(tmp.path().join("a.txt"), b"a").unwrap();
    std::fs::write(tmp.path().join(".hidden"), b"x").unwrap();
    std::fs::create_dir(tmp.path().join("sub")).unwrap();
    std::fs::write(tmp.path().join("sub/b.txt"), b"b").unwrap();

    let mut entries = watcher::scan_directory(tmp.path()).unwrap();
    entries.sort_by(|x, y| x.relative_path.cmp(&y.relative_path));
    let names: Vec<_> = entries.iter().map(|e| e.relative_path.clone()).collect();

    assert!(!names.iter().any(|n| n == ".hidden"));
    assert!(names.iter().any(|n| n == "a.txt"));
    assert!(names.iter().any(|n| n == "sub"));
    // Subdir contents should appear with their relative path including the parent.
    assert!(names.iter().any(|n| n.ends_with("b.txt") && n.contains("sub")));
}

#[test]
fn hash_file_is_deterministic_and_distinguishes_content() {
    let tmp = TempDir::new().unwrap();
    let a = tmp.path().join("a.bin");
    std::fs::write(&a, [1u8; 1024]).unwrap();
    let h1 = watcher::hash_file(&a).unwrap();
    let h2 = watcher::hash_file(&a).unwrap();
    assert_eq!(h1, h2);
    assert_eq!(h1.len(), 64, "blake3 hex string is 32 bytes -> 64 hex chars");

    let b = tmp.path().join("b.bin");
    let mut bytes = vec![1u8; 1024];
    bytes[42] ^= 1;
    std::fs::write(&b, &bytes).unwrap();
    let hb = watcher::hash_file(&b).unwrap();
    assert_ne!(h1, hb, "single-bit change must produce a different hash");
}

#[tokio::test]
async fn watcher_emits_events_for_writes() {
    let tmp = TempDir::new().unwrap();
    // Short debounce to keep the test snappy.
    let (_w, mut rx) = FileWatcher::start(tmp.path(), 100).expect("start watcher");

    // Give the OS-level watcher a moment to register before we start writing.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let p = tmp.path().join("hello.txt");
    std::fs::write(&p, b"hi").unwrap();

    // Wait for the debounced event with a reasonable cap.
    let got = tokio::time::timeout(Duration::from_secs(2), rx.recv()).await;
    let events = got
        .expect("watcher did not deliver any events within 2s")
        .expect("watcher channel closed unexpectedly");
    assert!(!events.is_empty(), "expected at least one event");
    let saw_target = events.iter().any(|e| {
        matches!(
            e,
            FsEvent::Created(path) | FsEvent::Modified(path) if path.ends_with("hello.txt")
        )
    });
    assert!(saw_target, "expected a Created/Modified event for hello.txt, got {events:?}");
}
