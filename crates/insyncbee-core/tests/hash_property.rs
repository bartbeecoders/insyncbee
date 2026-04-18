//! Property tests for `watcher::hash_file`. blake3 is the integrity guarantee
//! that the sync engine relies on to detect local changes — a regression that
//! breaks determinism would silently corrupt change detection.

use insyncbee_core::watcher;
use proptest::prelude::*;
use tempfile::TempDir;

proptest! {
    #[test]
    fn hash_is_deterministic_over_arbitrary_bytes(bytes in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("prop.bin");
        std::fs::write(&p, &bytes).unwrap();
        let h1 = watcher::hash_file(&p).unwrap();
        let h2 = watcher::hash_file(&p).unwrap();
        prop_assert_eq!(h1, h2);
    }

    #[test]
    fn distinct_bytes_yield_distinct_hashes(
        a in proptest::collection::vec(any::<u8>(), 1..1024),
        b in proptest::collection::vec(any::<u8>(), 1..1024),
    ) {
        // Skip the trivial case where the two byte vectors happened to be equal.
        prop_assume!(a != b);
        let tmp = TempDir::new().unwrap();
        let pa = tmp.path().join("a.bin");
        let pb = tmp.path().join("b.bin");
        std::fs::write(&pa, &a).unwrap();
        std::fs::write(&pb, &b).unwrap();
        let ha = watcher::hash_file(&pa).unwrap();
        let hb = watcher::hash_file(&pb).unwrap();
        prop_assert_ne!(ha, hb);
    }

    #[test]
    fn hash_is_64_hex_chars(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
        let tmp = TempDir::new().unwrap();
        let p = tmp.path().join("len.bin");
        std::fs::write(&p, &bytes).unwrap();
        let h = watcher::hash_file(&p).unwrap();
        prop_assert_eq!(h.len(), 64);
        prop_assert!(h.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
