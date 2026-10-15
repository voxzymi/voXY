//! The editor's sandbox client: a clean run, a crash, a hang, a bundle that
//! does not load, and the blocklist. The test process stands in for the
//! editor and must come through all of them.

#![allow(clippy::unwrap_used)]

mod common;

use std::path::Path;
use std::time::{Duration, Instant};

use voxy_ofx_host::client::{Blocklist, Sandbox, SandboxError, Timeouts};
use voxy_ofx_host::effect::Project;
use voxy_ofx_host::image::{Components, Depth, Premult};
use voxy_ofx_host::params::ParamValue;
use voxy_ofx_host::protocol::{ImageSlot, ParamSnapshot, Reply, Request};
use voxy_ofx_host::shm::SessionDir;

const EXE: &str = env!("CARGO_BIN_EXE_voxy-ofx-sandbox");

fn launch(dir: &SessionDir, bundle: &Path, timeouts: Timeouts) -> Result<Sandbox, SandboxError> {
    static N: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let id = format!("c{}", N.fetch_add(1, std::sync::atomic::Ordering::SeqCst));
    Sandbox::launch(Path::new(EXE), bundle, dir, &id, 1 << 20, timeouts)
}

fn create(sb: &mut Sandbox, plugin: &str) {
    let r = sb
        .request(Request::CreateInstance {
            instance: 1,
            plugin: plugin.into(),
            context: "OfxImageEffectContextFilter".into(),
            project: Project::default(),
            params: vec![ParamSnapshot {
                name: "gain".into(),
                value: ParamValue::Doubles(vec![2.0]),
                keys: vec![],
            }],
        })
        .unwrap();
    assert!(matches!(r, Reply::Done { .. }), "{r:?}");
}

fn render() -> Request {
    let slot = |offset| ImageSlot {
        offset,
        bounds: [0, 0, 2, 2],
        row_bytes: 32,
        depth: Depth::Float,
        components: Components::Rgba,
        premult: Premult::Premultiplied,
    };
    Request::Render {
        instance: 1,
        time: 0.0,
        window: [0, 0, 2, 2],
        scale: [1.0, 1.0],
        inputs: vec![("Source".into(), slot(4096))],
        output: slot(4096 + 64),
    }
}

#[test]
fn a_clean_run_and_a_clean_end() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = common::bundle(tmp.path());
    let dir = SessionDir::create(tmp.path()).unwrap();
    let mut sb = launch(&dir, &bundle, Timeouts::default()).unwrap();
    assert!(matches!(
        sb.request(Request::Describe).unwrap(),
        Reply::Described(_)
    ));
    create(&mut sb, "org.voxy.test.Gain");
    assert!(matches!(sb.request(render()).unwrap(), Reply::Done { .. }));
    assert_eq!(sb.dead(), None);
    drop(sb);
}

#[test]
fn a_crash_is_reported_and_the_sandbox_stays_dead() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = common::bundle(tmp.path());
    let dir = SessionDir::create(tmp.path()).unwrap();
    let mut sb = launch(&dir, &bundle, Timeouts::default()).unwrap();
    sb.request(Request::Describe).unwrap();
    create(&mut sb, "org.voxy.test.Crash");
    let err = sb.request(render()).unwrap_err();
    let SandboxError::Crashed { action, .. } = &err else {
        panic!("{err:?}")
    };
    assert_eq!(action, "Render");
    assert!(matches!(
        sb.request(Request::Describe),
        Err(SandboxError::Dead(_))
    ));

    let mut blocks = Blocklist::new();
    let b = blocks.record("org.voxy.test.Crash", &err);
    assert!(!b.persistent);
    assert!(b.reason.contains("crashed during Render"), "{}", b.reason);
}

#[test]
fn a_hang_times_out_and_is_killed() {
    let tmp = tempfile::tempdir().unwrap();
    let bundle = common::bundle(tmp.path());
    let dir = SessionDir::create(tmp.path()).unwrap();
    let timeouts = Timeouts {
        render: Duration::from_millis(500),
        ..Timeouts::default()
    };
    let mut sb = launch(&dir, &bundle, timeouts).unwrap();
    sb.request(Request::Describe).unwrap();
    create(&mut sb, "org.voxy.test.Hang");
    let start = Instant::now();
    let err = sb.request(render()).unwrap_err();
    assert_eq!(
        err,
        SandboxError::Timeout {
            action: "Render".into(),
            after: Duration::from_millis(500)
        }
    );
    assert!(start.elapsed() < Duration::from_secs(5));
    assert!(sb.dead().is_some());
    // Dropping a dead sandbox does not wait on it.
    let t = Instant::now();
    drop(sb);
    assert!(t.elapsed() < Duration::from_secs(2));
}

#[test]
fn a_bundle_that_does_not_load_fails_the_launch() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = SessionDir::create(tmp.path()).unwrap();
    let missing = tmp.path().join("Nothing.ofx.bundle");
    std::fs::create_dir_all(&missing).unwrap();
    let err = launch(&dir, &missing, Timeouts::default()).unwrap_err();
    let SandboxError::Launch(why) = &err else {
        panic!("{err:?}")
    };
    assert!(why.contains("no binary"), "{why}");
}

#[test]
fn three_failures_make_a_block_persistent_and_retry_clears_it() {
    let err = SandboxError::Timeout {
        action: "Render".into(),
        after: Duration::from_secs(30),
    };
    let mut b = Blocklist::new();
    assert!(!b.record("x", &err).persistent);
    assert!(!b.record("x", &err).persistent);
    assert!(b.record("x", &err).persistent);
    assert_eq!(b.persistent().len(), 1);
    let restored = Blocklist::with_persistent(b.persistent());
    assert!(restored.blocked("x").unwrap().persistent);
    b.retry("x");
    assert_eq!(b.blocked("x"), None);
}
