//! External (OpenFX) effects in the render graph (design §6.9): the layer
//! is read back, a CPU provider runs, the result is uploaded. Without a
//! provider, or when it fails, the image passes through and the error is
//! reported.

#![allow(clippy::unwrap_used)]

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use common::gpu;
use voxy_core::command::{AddEffect, AddSolidLayer, Command, SetProperty};
use voxy_core::external::{CpuFrame, ExternalEffects};
use voxy_core::glam::DVec2;
use voxy_core::registry::{Category, EffectDef, EffectOrigin, EffectSpace};
use voxy_core::{
    CompSettings, Document, EffectTypeId, EvaluatedEffect, LinearRgba, Time, Value, WorkingSpace,
};
use voxy_render::{Compositor, FrameRequest, NoVideo, RenderPlan, RenderReport, RgbaImage};

const TYPE: &str = "ofx:org.voxy.test.CpuInvert";

fn def() -> EffectDef {
    EffectDef {
        type_id: EffectTypeId::new(TYPE),
        version: 1,
        name: "CPU Invert".into(),
        category: Category::Plugin("tests".into()),
        description: "".into(),
        params: Arc::new([]),
        groups: Arc::new([]),
        space: EffectSpace::Working,
        seeded: false,
        origin: EffectOrigin::Ofx {
            binary: "test.ofx".into(),
            index: 0,
        },
    }
}

/// A 4×4 composition with an orange solid and the external effect.
fn scene() -> Document {
    let mut doc = Document::with_comp(
        "External",
        CompSettings {
            width: 4,
            height: 4,
            background: LinearRgba::from_srgb8(0, 0, 0, 0),
            ..CompSettings::default()
        },
    );
    doc.meta.working_space = WorkingSpace::SrgbCompat;
    let comp = doc.main_comp.unwrap();
    AddSolidLayer {
        comp,
        name: None,
        color: LinearRgba::from_srgb8(230, 120, 40, 255),
        index: 0,
    }
    .apply(&mut doc)
    .unwrap();
    let l = doc.main().unwrap().layers[0].clone();
    if let voxy_core::LayerKind::Solid { size, .. } = &l.kind {
        for (prop, v) in [
            (size.id, Value::Vec2(DVec2::new(4.0, 4.0))),
            (l.transform.anchor.id, Value::Vec2(DVec2::ZERO)),
            (l.transform.position.id, Value::Vec2(DVec2::ZERO)),
        ] {
            SetProperty {
                prop,
                time: Time::ZERO,
                value: v,
            }
            .apply(&mut doc)
            .unwrap();
        }
    }
    AddEffect {
        layer: l.id,
        index: None,
        def: Arc::new(def()),
    }
    .apply(&mut doc)
    .unwrap();
    doc
}

#[derive(Debug, Default)]
struct CpuInvert {
    calls: AtomicUsize,
    fail: bool,
}

impl ExternalEffects for CpuInvert {
    fn supports(&self, type_id: &EffectTypeId) -> bool {
        type_id.as_str() == TYPE
    }

    fn render(&self, _fx: &EvaluatedEffect, frame: &mut CpuFrame) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err("the plugin crashed during Render".into());
        }
        assert_eq!((frame.width, frame.height), (4, 4));
        for p in frame.pixels.as_chunks_mut::<4>().0 {
            for c in 0..3 {
                p[c] = p[3] - p[c];
            }
        }
        Ok(())
    }
}

fn render(doc: &Document, ext: Option<Arc<CpuInvert>>) -> (RgbaImage, RenderReport) {
    let gpu = gpu().unwrap();
    let mut c = Compositor::new(&gpu.device);
    c.set_external(ext.map(|e| e as Arc<dyn ExternalEffects>));
    let req = FrameRequest::export(doc.main_comp.unwrap(), Time::ZERO);
    let eval = voxy_core::evaluate(doc, req.comp, req.time).unwrap();
    let plan = RenderPlan::build(&eval, &req.settings);
    c.render_to_image(
        &gpu.device,
        &gpu.queue,
        &doc.assets,
        &NoVideo,
        &plan,
        req.alpha,
    )
    .unwrap()
}

fn px(img: &RgbaImage) -> [u8; 4] {
    img.pixels[..4].try_into().unwrap()
}

fn close(a: [u8; 4], b: [u8; 4]) -> bool {
    a.iter().zip(&b).all(|(x, y)| x.abs_diff(*y) <= 1)
}

#[test]
fn a_cpu_effect_runs_between_gpu_passes() {
    if gpu().is_none() {
        return;
    }
    let doc = scene();
    let ext = Arc::new(CpuInvert::default());
    let (img, report) = render(&doc, Some(ext.clone()));
    assert_eq!(ext.calls.load(Ordering::SeqCst), 1);
    assert_eq!(report.external_errors, vec![]);
    assert!(close(px(&img), [25, 135, 215, 255]), "{:?}", px(&img));
}

#[test]
fn without_a_provider_or_on_failure_the_image_passes_through() {
    if gpu().is_none() {
        return;
    }
    let doc = scene();
    let (img, report) = render(&doc, None);
    assert!(close(px(&img), [230, 120, 40, 255]), "{:?}", px(&img));
    assert_eq!(report.external_errors.len(), 1);

    let failing = Arc::new(CpuInvert {
        fail: true,
        ..CpuInvert::default()
    });
    let (img, report) = render(&doc, Some(failing));
    assert!(close(px(&img), [230, 120, 40, 255]));
    assert_eq!(
        report.external_errors[0].1,
        "the plugin crashed during Render"
    );
}
