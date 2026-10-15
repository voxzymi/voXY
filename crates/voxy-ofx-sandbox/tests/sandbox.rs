//! The sandbox process, end to end: a real `.ofx.bundle` built from our
//! test plugins, loaded in `voxy-ofx-sandbox`, driven over the local socket
//! with images in shared memory. A crashing plugin takes down only the
//! sandbox.

#![allow(clippy::unwrap_used)]

mod common;

use common::bundle;

use std::process::{Child, Command};

use voxy_ofx_host::channel;
use voxy_ofx_host::effect::{MessageKind, Project};
use voxy_ofx_host::image::{Components, Depth, Image, Premult, RectI};
use voxy_ofx_host::params::ParamValue;
use voxy_ofx_host::protocol::{
    ImageSlot, ParamSnapshot, Reply, Request, VERSION, read_frame, write_frame,
};
use voxy_ofx_host::shm::{SessionDir, SharedMemory, Slots};

struct Session {
    _tmp: tempfile::TempDir,
    _dir: SessionDir,
    child: Child,
    stream: channel::Stream,
    shm: SharedMemory,
}

impl Session {
    fn start() -> Session {
        let tmp = tempfile::tempdir().unwrap();
        let bundle = bundle(tmp.path());
        let dir = SessionDir::create(tmp.path()).unwrap();
        let spec = channel::spec(dir.path(), &format!("t{}", rand_id()));
        let listener = channel::listen(&spec).unwrap();
        let child = Command::new(env!("CARGO_BIN_EXE_voxy-ofx-sandbox"))
            .args([
                "--bundle".as_ref(),
                bundle.as_os_str(),
                "--channel".as_ref(),
                spec.as_ref(),
            ])
            .spawn()
            .unwrap();
        let stream = channel::accept(&listener).unwrap();
        let shm = SharedMemory::create(&dir.path().join("sandbox-1"), 1 << 20).unwrap();
        let mut s = Session {
            _tmp: tmp,
            _dir: dir,
            child,
            stream,
            shm,
        };
        assert!(matches!(
            s.ask(&Request::Hello { version: VERSION }),
            Reply::Hello {
                version: VERSION,
                ..
            }
        ));
        let path = s.shm.path().display().to_string();
        assert!(matches!(s.ask(&Request::Map { path }), Reply::Done { .. }));
        // The sandbox beats in the shared mapping.
        let start = std::time::Instant::now();
        while s.shm.heartbeat() == 0 {
            assert!(start.elapsed().as_secs() < 5, "no heartbeat");
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        s
    }

    fn ask(&mut self, r: &Request) -> Reply {
        write_frame(&mut self.stream, r).unwrap();
        read_frame(&mut self.stream).unwrap()
    }
}

fn rand_id() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let t = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos() as u64;
    t ^ u64::from(std::process::id()) << 32
}

const SOURCE: [f32; 16] = [
    0.1, 0.2, 0.3, 1.0, //
    0.2, 0.1, 0.0, 0.5, //
    0.0, 0.0, 0.0, 0.0, //
    0.25, 0.25, 0.25, 0.25,
];

fn slot(offset: u64) -> ImageSlot {
    ImageSlot {
        offset,
        bounds: [0, 0, 2, 2],
        row_bytes: 32,
        depth: Depth::Float,
        components: Components::Rgba,
        premult: Premult::Premultiplied,
    }
}

fn create(s: &mut Session, plugin: &str, gain: f64) -> Reply {
    s.ask(&Request::CreateInstance {
        instance: 1,
        plugin: plugin.into(),
        context: "OfxImageEffectContextFilter".into(),
        project: Project {
            size: [2.0, 2.0],
            extent: [2.0, 2.0],
            ..Project::default()
        },
        params: vec![
            ParamSnapshot {
                name: "gain".into(),
                value: ParamValue::Doubles(vec![gain]),
                keys: vec![],
            },
            ParamSnapshot {
                name: "straight".into(),
                value: ParamValue::Ints(vec![0]),
                keys: vec![],
            },
        ],
    })
}

/// Puts the source in a slot and asks for a render into another.
fn render(s: &mut Session) -> Reply {
    let mut slots = Slots::new(s.shm.len());
    let (src, out) = (slots.take(128).unwrap(), slots.take(128).unwrap());
    let img = Image::from_rgba(
        RectI::new(0, 0, 2, 2),
        &SOURCE,
        Depth::Float,
        Components::Rgba,
        Premult::Premultiplied,
    )
    .unwrap();
    assert!(s.shm.write_from(src, &img.raw()));
    s.ask(&Request::Render {
        instance: 1,
        time: 0.0,
        window: [0, 0, 2, 2],
        scale: [1.0, 1.0],
        inputs: vec![("Source".into(), slot(src))],
        output: slot(out),
    })
}

#[test]
fn a_bundle_describes_and_renders_out_of_process() {
    let mut s = Session::start();
    let Reply::Described(d) = s.ask(&Request::Describe) else {
        panic!("describe failed")
    };
    let ids: Vec<&str> = d.plugins.iter().map(|p| p.identifier.as_str()).collect();
    assert_eq!(
        ids,
        [
            "org.voxy.test.Gain",
            "org.voxy.test.Crash",
            "org.voxy.test.Hang",
            "org.voxy.test.Solid",
            "org.voxy.test.Blur"
        ]
    );
    assert_eq!(d.plugins[0].props.string("OfxPropLabel"), Some("Test Gain"));
    assert_eq!(d.failed, Vec::<(String, String)>::new());

    assert!(matches!(
        create(&mut s, "org.voxy.test.Gain", 2.0),
        Reply::Done { .. }
    ));
    let Reply::ClipPreferences {
        clips,
        output_premult,
        ..
    } = s.ask(&Request::ClipPreferences { instance: 1 })
    else {
        panic!()
    };
    assert_eq!(clips[1], ("Source".into(), Depth::Float, Components::Rgba));
    assert_eq!(output_premult, Premult::Premultiplied);
    assert_eq!(
        s.ask(&Request::RegionOfDefinition {
            instance: 1,
            time: 0.0,
            scale: [1.0, 1.0],
            inputs: vec![("Source".into(), [0.0, 0.0, 2.0, 2.0])],
        }),
        Reply::Region([0.0, 0.0, 2.0, 2.0])
    );
    assert_eq!(
        s.ask(&Request::IsIdentity {
            instance: 1,
            time: 0.0,
            window: [0, 0, 2, 2],
            scale: [1.0, 1.0]
        }),
        Reply::Identity(None)
    );

    assert!(matches!(render(&mut s), Reply::Done { .. }));
    let mut bytes = vec![0u8; 64];
    assert!(s.shm.read_into(4096 + 128, &mut bytes));
    let out = Image::from_raw(
        RectI::new(0, 0, 2, 2),
        Depth::Float,
        Components::Rgba,
        Premult::Premultiplied,
        &bytes,
    )
    .unwrap();
    let want: Vec<f32> = SOURCE
        .iter()
        .enumerate()
        .map(|(i, v)| if i % 4 == 3 { *v } else { v * 2.0 })
        .collect();
    assert!(
        out.to_rgba()
            .iter()
            .zip(&want)
            .all(|(a, b)| (a - b).abs() < 1e-6)
    );

    let Reply::Done { messages, .. } = s.ask(&Request::InstanceChanged {
        instance: 1,
        param: "gain".into(),
        time: 0.0,
    }) else {
        panic!()
    };
    assert_eq!(
        (messages[0].kind, messages[0].text.as_str()),
        (MessageKind::Log, "gain changed")
    );

    assert!(matches!(
        s.ask(&Request::DestroyInstance { instance: 1 }),
        Reply::Done { .. }
    ));
    assert!(matches!(
        s.ask(&Request::DestroyInstance { instance: 1 }),
        Reply::Error(_)
    ));
    assert!(matches!(s.ask(&Request::Unload), Reply::Done { .. }));
    assert!(s.child.wait().unwrap().success());
}

#[test]
fn a_crashing_plugin_only_takes_down_the_sandbox() {
    let mut s = Session::start();
    assert!(matches!(s.ask(&Request::Describe), Reply::Described(_)));
    assert!(matches!(
        create(&mut s, "org.voxy.test.Crash", 1.5),
        Reply::Done { .. }
    ));
    write_frame(&mut s.stream, &render_request()).unwrap();
    assert!(
        read_frame::<Reply>(&mut s.stream).is_err(),
        "the channel closes"
    );
    let status = s.child.wait().unwrap();
    assert!(!status.success(), "{status:?}");
    // This process carries on.
    assert_eq!(1 + 1, 2);
}

fn render_request() -> Request {
    Request::Render {
        instance: 1,
        time: 0.0,
        window: [0, 0, 2, 2],
        scale: [1.0, 1.0],
        inputs: vec![("Source".into(), slot(4096))],
        output: slot(4096 + 128),
    }
}
