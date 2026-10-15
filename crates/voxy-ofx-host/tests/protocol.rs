//! The sandbox wire protocol: frames round-trip, and bad input is an error,
//! never a panic or a huge allocation.

#![allow(clippy::unwrap_used)]

use std::io::Cursor;
use std::sync::Arc;

use voxy_ofx_host::effect::{Effect, Message, MessageKind, Project};
use voxy_ofx_host::image::{Components, Depth, Premult};
use voxy_ofx_host::params::{ParamKind, ParamValue};
use voxy_ofx_host::protocol::{
    FrameError, ImageSlot, MAX_FRAME, ParamSnapshot, PluginDesc, Prop, Reply, Request, decode,
    read_frame, write_frame,
};
use voxy_ofx_host::suites::driver::Description;

fn requests() -> Vec<Request> {
    let slot = ImageSlot {
        offset: 4096,
        bounds: [0, 0, 64, 32],
        row_bytes: 1024,
        depth: Depth::Float,
        components: Components::Rgba,
        premult: Premult::Premultiplied,
    };
    vec![
        Request::Hello { version: 1 },
        Request::Describe,
        Request::CreateInstance {
            instance: 7,
            plugin: "org.example.Blur".into(),
            context: "OfxImageEffectContextFilter".into(),
            project: Project::default(),
            params: vec![ParamSnapshot {
                name: "radius".into(),
                value: ParamValue::Doubles(vec![2.5]),
                keys: vec![0.0, 10.0],
            }],
        },
        Request::Render {
            instance: 7,
            time: 3.0,
            window: [0, 0, 64, 32],
            scale: [1.0, 1.0],
            inputs: vec![("Source".into(), slot)],
            output: ImageSlot {
                offset: 1 << 20,
                ..slot
            },
        },
        Request::Unload,
    ]
}

#[test]
fn frames_round_trip() {
    let mut buf = Vec::new();
    for r in requests() {
        write_frame(&mut buf, &r).unwrap();
    }
    let reply = Reply::Done {
        messages: vec![Message {
            kind: MessageKind::Warning,
            id: Some("w".into()),
            text: "slow".into(),
        }],
        edits: vec![],
    };
    write_frame(&mut buf, &reply).unwrap();
    let mut r = Cursor::new(buf);
    for want in requests() {
        assert_eq!(read_frame::<Request>(&mut r).unwrap(), want);
    }
    assert_eq!(read_frame::<Reply>(&mut r).unwrap(), reply);
    assert!(
        matches!(read_frame::<Request>(&mut r), Err(FrameError::Io(_))),
        "end of stream"
    );
}

#[test]
fn oversized_truncated_and_trailing_frames_are_refused() {
    let mut big = (MAX_FRAME + 1).to_le_bytes().to_vec();
    big.extend_from_slice(&[0; 8]);
    assert!(matches!(
        read_frame::<Request>(&mut Cursor::new(big)),
        Err(FrameError::TooLarge(_))
    ));

    let mut buf = Vec::new();
    write_frame(&mut buf, &requests()[2]).unwrap();
    buf.truncate(buf.len() - 3);
    assert!(matches!(
        read_frame::<Request>(&mut Cursor::new(buf)),
        Err(FrameError::Io(_))
    ));

    let mut payload = postcard::to_stdvec(&Request::Describe).unwrap();
    payload.push(0);
    assert!(matches!(
        decode::<Request>(&payload),
        Err(FrameError::Decode(_))
    ));
}

#[test]
fn garbage_never_panics() {
    // A deterministic byte soup: every prefix of it must decode or fail.
    let mut x: u32 = 0x9e37_79b9;
    let soup: Vec<u8> = (0..4096)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x & 0xff) as u8
        })
        .collect();
    for n in 0..soup.len() {
        let _ = decode::<Request>(&soup[..n]);
        let _ = decode::<Reply>(&soup[n..]);
    }
}

#[test]
fn a_description_crosses_the_wire_without_pointers() {
    let general = Arc::new(Effect::descriptor());
    general
        .props
        .set(
            "OfxPropLabel",
            voxy_ofx_host::props::Values::strings(&["Blur"]),
        )
        .unwrap();
    let ctx = Arc::new(Effect::context_descriptor(
        &general,
        b"OfxImageEffectContextFilter\0",
    ));
    ctx.define_clip("Source").unwrap();
    ctx.define_clip("Output").unwrap();
    ctx.params.define(ParamKind::Double, "radius").unwrap();
    let d = Description {
        general,
        contexts: vec![("OfxImageEffectContextFilter".into(), ctx)],
    };
    let desc = PluginDesc::from_description("org.example.Blur", (1, 0), &d);
    let mut buf = Vec::new();
    write_frame(&mut buf, &desc).unwrap();
    let back: PluginDesc = read_frame(&mut Cursor::new(buf)).unwrap();
    assert_eq!(back, desc);
    assert_eq!(back.props.string("OfxPropLabel"), Some("Blur"));
    let filter = &back.contexts[0];
    assert_eq!(
        filter
            .clips
            .iter()
            .map(|c| c.name.as_str())
            .collect::<Vec<_>>(),
        ["Source", "Output"]
    );
    assert_eq!(filter.params[0].kind, ParamKind::Double);
    assert_eq!(
        filter.params[0].props.get("OfxParamPropDefault"),
        Some(&Prop::Double(vec![0.0]))
    );
    assert!(
        filter.params[0].props.get("OfxParamPropDataPtr").is_none(),
        "pointers stay home"
    );
}
