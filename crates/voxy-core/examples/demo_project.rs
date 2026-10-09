//! Writes a small demo project: `cargo run -p voxy-core --example demo_project -- out.voxy`

use voxy_core::command::{AddSolidLayer, Command, SetProperty};
use voxy_core::glam::DVec2;
use voxy_core::{CompSettings, Document, LayerKind, LinearRgba, Value, persist};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "demo.voxy".into());
    let mut doc = Document::new(
        "Demo",
        CompSettings {
            background: LinearRgba::from_srgb8(18, 18, 28, 255),
            ..CompSettings::default()
        },
    );
    let comp = doc.root_comp;
    let shapes = [
        ((124, 92, 255), (800.0, 450.0), (760.0, 540.0), -12.0, 1.0),
        ((255, 196, 64), (520.0, 520.0), (1180.0, 520.0), 20.0, 0.75),
        ((40, 210, 180), (300.0, 300.0), (1000.0, 760.0), 45.0, 0.6),
    ];
    for ((r, g, b), size, pos, rot, opacity) in shapes {
        AddSolidLayer {
            comp,
            name: None,
            color: LinearRgba::from_srgb8(r, g, b, 255),
            index: 0,
        }
        .apply(&mut doc)?;
        let l = doc.root().layers[0].clone();
        let LayerKind::Solid {
            size: size_prop, ..
        } = &l.kind
        else {
            unreachable!()
        };
        let size = DVec2::from(size);
        for (prop, value) in [
            (size_prop.id, Value::Vec2(size)),
            (l.transform.anchor.id, Value::Vec2(size / 2.0)),
            (l.transform.position.id, Value::Vec2(DVec2::from(pos))),
            (l.transform.rotation.id, Value::Scalar(rot)),
            (l.transform.opacity.id, Value::Scalar(opacity)),
        ] {
            SetProperty { prop, value }.apply(&mut doc)?;
        }
    }
    persist::save(&doc, std::path::Path::new(&path))?;
    println!("wrote {path}");
    Ok(())
}
