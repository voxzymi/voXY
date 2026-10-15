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
use voxy_core::external::{CpuFrame, ExternalEffects, Region};
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

/// A 4×4 composition with an orange 4×4 solid and the external effect.
fn scene() -> Document {
    scene_at(4.0, 0.0)
}

/// A 4×4 composition with an orange `side`² solid at (`pos`, `pos`) and
/// the external effect.
fn scene_at(side: f64, pos: f64) -> Document {
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
            (size.id, Value::Vec2(DVec2::splat(side))),
            (l.transform.anchor.id, Value::Vec2(DVec2::ZERO)),
            (l.transform.position.id, Value::Vec2(DVec2::splat(pos))),
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

fn render(doc: &Document, ext: Option<Arc<dyn ExternalEffects>>) -> (RgbaImage, RenderReport) {
    let gpu = gpu().unwrap();
    let mut c = Compositor::new(&gpu.device);
    c.set_external(ext);
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
    let (img, report) = render(&doc, Some(ext.clone() as Arc<dyn ExternalEffects>));
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
    let (img, report) = render(&doc, Some(failing as Arc<dyn ExternalEffects>));
    assert!(close(px(&img), [230, 120, 40, 255]));
    assert_eq!(
        report.external_errors[0].1,
        "the plugin crashed during Render"
    );
}

/// Fills its whole buffer with white and reports a region of definition
/// one pixel wider than its input on every side.
#[derive(Debug, Default)]
struct Flood {
    frames: std::sync::Mutex<Vec<(u32, u32, DVec2, DVec2)>>,
}

impl ExternalEffects for Flood {
    fn supports(&self, type_id: &EffectTypeId) -> bool {
        type_id.as_str() == TYPE
    }

    fn render(&self, _fx: &EvaluatedEffect, frame: &mut CpuFrame) -> Result<(), String> {
        self.frames.lock().unwrap().push((
            frame.width,
            frame.height,
            frame.origin,
            frame.layer_size,
        ));
        frame.pixels.fill(1.0);
        Ok(())
    }

    fn region_of_definition(&self, _fx: &EvaluatedEffect, input: Region, _: DVec2) -> Region {
        Region {
            min: input.min - DVec2::ONE,
            max: input.max + DVec2::ONE,
        }
    }
}

#[test]
fn the_providers_region_of_definition_grows_the_layer() {
    if gpu().is_none() {
        return;
    }
    // A 2×2 solid at (1, 1): its effect buffer reaches one pixel further,
    // so the whole 4×4 composition turns white.
    let doc = scene_at(2.0, 1.0);
    let flood = Arc::new(Flood::default());
    let (img, report) = render(&doc, Some(flood.clone() as Arc<dyn ExternalEffects>));
    assert_eq!(report.external_errors, vec![]);
    assert_eq!(
        *flood.frames.lock().unwrap(),
        vec![(4, 4, DVec2::splat(-1.0), DVec2::splat(2.0))]
    );
    for p in img.pixels.as_chunks::<4>().0 {
        assert_eq!(*p, [255; 4]);
    }
}

/// Records the frame each render is told.
#[derive(Debug, Default)]
struct Frames(std::sync::Mutex<Vec<i64>>);

impl ExternalEffects for Frames {
    fn supports(&self, type_id: &EffectTypeId) -> bool {
        type_id.as_str() == TYPE
    }

    fn render(&self, fx: &EvaluatedEffect, _: &mut CpuFrame) -> Result<(), String> {
        self.0.lock().unwrap().push(fx.frame);
        Ok(())
    }
}

#[test]
fn a_plugin_is_told_the_frame() {
    if gpu().is_none() {
        return;
    }
    let doc = scene();
    let rate = doc.main().unwrap().frame_rate;
    let frames = Arc::new(Frames::default());
    let gpu = gpu().unwrap();
    let mut c = Compositor::new(&gpu.device);
    c.set_external(Some(frames.clone() as Arc<dyn ExternalEffects>));
    for n in [3, 3, 7] {
        let req = FrameRequest::export(doc.main_comp.unwrap(), rate.frame_to_time(n));
        let eval = voxy_core::evaluate(&doc, req.comp, req.time).unwrap();
        let plan = RenderPlan::build(&eval, &req.settings);
        c.render_to_image(
            &gpu.device,
            &gpu.queue,
            &doc.assets,
            &NoVideo,
            &plan,
            req.alpha,
        )
        .unwrap();
    }
    // Frame 3's result is reused the second time.
    assert_eq!(*frames.0.lock().unwrap(), vec![3, 7]);
}

/// Replaces its input with the layer it reads.
#[derive(Debug, Default)]
struct CopyB(std::sync::Mutex<Vec<String>>);

impl ExternalEffects for CopyB {
    fn supports(&self, type_id: &EffectTypeId) -> bool {
        type_id.as_str() == TYPE
    }

    fn render(&self, _fx: &EvaluatedEffect, frame: &mut CpuFrame) -> Result<(), String> {
        for (name, pixels) in &frame.layers {
            self.0.lock().unwrap().push(name.to_string());
            frame.pixels.clone_from(pixels);
        }
        Ok(())
    }
}

#[test]
fn an_external_effect_reads_the_layer_its_clip_parameter_names() {
    if gpu().is_none() {
        return;
    }
    let mut doc = scene();
    let comp = doc.main_comp.unwrap();
    // A blue solid below the orange one, and a definition whose extra
    // clip reads it.
    AddSolidLayer {
        comp,
        name: None,
        color: LinearRgba::from_srgb8(20, 60, 200, 255),
        index: 1,
    }
    .apply(&mut doc)
    .unwrap();
    let layers = doc.main().unwrap().layers.clone();
    let (a, b) = (&layers[0], &layers[1]);
    if let voxy_core::LayerKind::Solid { size, .. } = &b.kind {
        for (prop, v) in [
            (size.id, Value::Vec2(DVec2::splat(4.0))),
            (b.transform.anchor.id, Value::Vec2(DVec2::ZERO)),
            (b.transform.position.id, Value::Vec2(DVec2::ZERO)),
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
    let tid = EffectTypeId::new(TYPE);
    let mut with_clip = def();
    with_clip.params = Arc::new([voxy_core::registry::EffectParamDef {
        name: "voxy.clip.SourceTo".into(),
        desc: Arc::new(voxy_core::PropertyDesc {
            key: tid.param_key("voxy.clip.SourceTo").into(),
            label: "Transition To".into(),
            kind: voxy_core::ValueKind::Layer,
            default: Value::Layer(None),
            range: None,
            soft_range: None,
            unit: voxy_core::Unit::None,
            animatable: true,
            spatial: false,
        }),
        choices: None,
        group: None,
        gizmo: None,
        hint: None,
        flags: voxy_core::registry::ParamFlags::default(),
    }]);
    voxy_core::command::RemoveEffects {
        layer: a.id,
        ids: vec![a.effects[0].id],
    }
    .apply(&mut doc)
    .unwrap();
    AddEffect {
        layer: a.id,
        index: None,
        def: Arc::new(with_clip),
    }
    .apply(&mut doc)
    .unwrap();
    let fx = doc.find_layer(a.id).unwrap().1.effects[0].clone();
    SetProperty {
        prop: fx.param("voxy.clip.SourceTo").unwrap().id,
        time: Time::ZERO,
        value: Value::Layer(Some(b.id)),
    }
    .apply(&mut doc)
    .unwrap();

    let copy = Arc::new(CopyB::default());
    let (img, report) = render(&doc, Some(copy.clone() as Arc<dyn ExternalEffects>));
    assert_eq!(report.external_errors, vec![]);
    assert_eq!(*copy.0.lock().unwrap(), vec!["voxy.clip.SourceTo"]);
    assert!(close(px(&img), [20, 60, 200, 255]), "{:?}", px(&img));
}
