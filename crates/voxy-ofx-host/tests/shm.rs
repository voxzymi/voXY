//! Shared memory (design §6.7, D4): two mappings of one file see each
//! other's writes, bounds are checked, and stale session folders go.

#![allow(clippy::unwrap_used)]

use voxy_ofx_host::shm::{ALIGN, HEADER, SessionDir, SharedMemory, Slots, clean_stale};

#[test]
fn two_mappings_share_bytes_flags_and_heartbeat() {
    let base = tempfile::tempdir().unwrap();
    let session = SessionDir::create(base.path()).unwrap();
    let path = session.path().join("sandbox-1");
    let mut host = SharedMemory::create(&path, 1 << 16).unwrap();
    let mut sandbox = SharedMemory::open(&path).unwrap();
    assert_eq!(host.len(), sandbox.len());

    let mut slots = Slots::new(host.len());
    let a = slots.take(100).unwrap();
    let b = slots.take(10).unwrap();
    assert_eq!(a, HEADER as u64);
    assert_eq!(b % ALIGN as u64, 0);
    assert!(b >= a + 100);

    host.bytes_mut(a, 100).unwrap().fill(7);
    assert_eq!(sandbox.bytes(a, 100).unwrap(), &[7; 100][..]);
    sandbox
        .bytes_mut(b, 10)
        .unwrap()
        .copy_from_slice(b"0123456789");
    assert_eq!(host.bytes(b, 10).unwrap(), b"0123456789");

    assert!(!sandbox.aborted());
    host.set_abort(true);
    assert!(sandbox.aborted());
    assert_eq!(sandbox.beat(), 1);
    assert_eq!(sandbox.beat(), 2);
    assert_eq!(host.heartbeat(), 2);
}

#[test]
fn ranges_outside_the_slots_are_refused() {
    let base = tempfile::tempdir().unwrap();
    let session = SessionDir::create(base.path()).unwrap();
    let mut m = SharedMemory::create(&session.path().join("m"), HEADER + 256).unwrap();
    assert!(m.bytes(0, 8).is_none(), "the header is not a slot");
    assert!(m.bytes(HEADER as u64 + 1, 8).is_none(), "unaligned");
    assert!(m.bytes(HEADER as u64, 257).is_none(), "past the end");
    assert!(m.bytes(u64::MAX, 1).is_none());
    assert!(m.ptr(HEADER as u64, 256).is_some());
    let mut slots = Slots::new(m.len());
    assert!(slots.take(256).is_some());
    assert!(slots.take(1).is_none(), "full");
    slots.reset();
    assert_eq!(slots.used(), HEADER);
}

#[test]
fn foreign_files_are_not_mappings() {
    let base = tempfile::tempdir().unwrap();
    let junk = base.path().join("junk");
    std::fs::write(&junk, vec![1u8; 8192]).unwrap();
    assert!(SharedMemory::open(&junk).is_err());
    let short = base.path().join("short");
    std::fs::write(&short, b"VOXYOFX\0").unwrap();
    assert!(SharedMemory::open(&short).is_err());
    assert!(SharedMemory::open(&base.path().join("missing")).is_err());
}

#[test]
fn stale_session_folders_are_removed_and_live_ones_kept() {
    let base = tempfile::tempdir().unwrap();
    let live = SessionDir::create(base.path()).unwrap();
    // A crashed session: its folder and lock file exist, nobody holds it.
    let dead = base.path().join("voxy-ofx-999999-0");
    std::fs::create_dir(&dead).unwrap();
    std::fs::write(dead.join("lock"), b"").unwrap();
    std::fs::write(dead.join("sandbox-1"), b"left over").unwrap();
    let other = base.path().join("not-ours");
    std::fs::create_dir(&other).unwrap();

    assert_eq!(clean_stale(base.path()), 1);
    assert!(!dead.exists());
    assert!(live.path().exists(), "a held session stays");
    assert!(other.exists(), "other folders are left alone");

    let path = live.path().to_path_buf();
    drop(live);
    assert!(!path.exists(), "removed when the session ends");
}
