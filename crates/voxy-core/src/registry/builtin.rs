//! Definitions of the built-in effects (parameter schemas).
//!
//! The formulas, units and ranges are documented in `docs/effects.md`, and
//! the WGSL kernels live in `voxy-render`. Changing a parameter or a
//! formula means a new version of the definition plus a migration; the old
//! version stays registered so older projects keep their look.

use std::sync::Arc;

use super::{Category, EffectDef, EffectOrigin, EffectSpace, ParamBuilder};
use crate::color::LinearRgba;
use crate::effect::EffectTypeId;
use crate::property::Unit;
use crate::value::Value;

/// Type IDs of the built-in effects.
pub mod ids {
    pub const GAUSSIAN_BLUR: &str = "voxy.blur.gaussian";
    pub const BOX_BLUR: &str = "voxy.blur.box";
    pub const DIRECTIONAL_BLUR: &str = "voxy.blur.directional";
    pub const BRIGHTNESS_CONTRAST: &str = "voxy.color.brightness_contrast";
    pub const EXPOSURE: &str = "voxy.color.exposure";
    pub const INVERT: &str = "voxy.color.invert";
    pub const TINT: &str = "voxy.color.tint";
    pub const HUE_SATURATION: &str = "voxy.color.hue_saturation";
}

/// Every built-in definition, in no particular order.
pub fn builtin_definitions() -> Vec<EffectDef> {
    vec![
        gaussian_blur(),
        box_blur(),
        directional_blur(),
        brightness_contrast(),
        exposure(),
        invert(),
        tint(),
        hue_saturation(),
    ]
}

const DIMENSIONS: &[&str] = &["Horizontal and Vertical", "Horizontal", "Vertical"];

fn def(
    id: &str,
    name: &str,
    category: Category,
    description: &str,
    build: impl FnOnce(&mut ParamBuilder),
) -> EffectDef {
    let type_id = EffectTypeId::new(id);
    let mut p = ParamBuilder::new(&type_id);
    build(&mut p);
    EffectDef {
        type_id,
        version: 1,
        name: name.into(),
        category,
        description: description.into(),
        params: p.finish(),
        groups: Arc::new([]),
        space: EffectSpace::Working,
        seeded: false,
        origin: EffectOrigin::Builtin,
    }
}

fn gaussian_blur() -> EffectDef {
    def(
        ids::GAUSSIAN_BLUR,
        "Gaussian Blur",
        Category::Blur,
        "Smooth blur with a bell-shaped falloff.",
        |p| {
            p.add("radius", "Blurriness", Value::Scalar(10.0), Unit::Pixels)
                .range(0.0, 1000.0)
                .soft(0.0, 100.0)
                .hint("About three standard deviations, in layer pixels.");
            p.add("dimensions", "Blur Dimensions", Value::Enum(0), Unit::None)
                .choices(DIMENSIONS);
            p.add(
                "repeat_edges",
                "Repeat Edge Pixels",
                Value::Bool(false),
                Unit::None,
            )
            .hint("Extend the edge pixels instead of blurring into transparency.");
        },
    )
}

fn box_blur() -> EffectDef {
    def(
        ids::BOX_BLUR,
        "Box Blur",
        Category::Blur,
        "Averages a square of pixels; more passes look smoother.",
        |p| {
            p.add("radius", "Blur Radius", Value::Scalar(5.0), Unit::Pixels)
                .range(0.0, 1000.0)
                .soft(0.0, 50.0);
            p.add("passes", "Passes", Value::Int(1), Unit::None)
                .range(1.0, 5.0);
            p.add("dimensions", "Blur Dimensions", Value::Enum(0), Unit::None)
                .choices(DIMENSIONS);
            p.add(
                "repeat_edges",
                "Repeat Edge Pixels",
                Value::Bool(false),
                Unit::None,
            );
        },
    )
}

fn directional_blur() -> EffectDef {
    def(
        ids::DIRECTIONAL_BLUR,
        "Directional Blur",
        Category::Blur,
        "Smears the layer along one direction, like motion.",
        |p| {
            p.add("direction", "Direction", Value::Scalar(0.0), Unit::Degrees)
                .soft(-180.0, 180.0)
                .hint("0° blurs horizontally; positive angles turn clockwise.");
            p.add("length", "Blur Length", Value::Scalar(10.0), Unit::Pixels)
                .range(0.0, 2000.0)
                .soft(0.0, 200.0);
        },
    )
}

fn brightness_contrast() -> EffectDef {
    def(
        ids::BRIGHTNESS_CONTRAST,
        "Brightness & Contrast",
        Category::Color,
        "Lifts or lowers all tones, and spreads or squeezes them around mid grey.",
        |p| {
            p.add(
                "brightness",
                "Brightness",
                Value::Scalar(0.0),
                Unit::Normalized,
            )
            .range(-1.0, 1.0);
            p.add("contrast", "Contrast", Value::Scalar(0.0), Unit::Normalized)
                .range(-1.0, 1.0);
        },
    )
}

fn exposure() -> EffectDef {
    let mut d = def(
        ids::EXPOSURE,
        "Exposure",
        Category::Color,
        "Changes exposure in photographic stops, in linear light.",
        |p| {
            p.add(
                "exposure",
                "Exposure (stops)",
                Value::Scalar(0.0),
                Unit::None,
            )
            .range(-20.0, 20.0)
            .soft(-5.0, 5.0);
            p.add("offset", "Offset", Value::Scalar(0.0), Unit::None)
                .range(-1.0, 1.0)
                .soft(-0.1, 0.1);
            p.add("gamma", "Gamma Correction", Value::Scalar(1.0), Unit::None)
                .range(0.01, 10.0)
                .soft(0.2, 5.0);
        },
    );
    d.space = EffectSpace::Linear;
    d
}

fn invert() -> EffectDef {
    def(
        ids::INVERT,
        "Invert",
        Category::Color,
        "Turns each selected channel into its opposite.",
        |p| {
            p.add("channel", "Channel", Value::Enum(0), Unit::None)
                .choices(&["RGB", "Red", "Green", "Blue", "Alpha"]);
            p.add(
                "mix",
                "Blend With Original",
                Value::Scalar(0.0),
                Unit::Normalized,
            )
            .range(0.0, 1.0);
        },
    )
}

fn tint() -> EffectDef {
    def(
        ids::TINT,
        "Tint",
        Category::Color,
        "Maps dark tones to one colour and light tones to another.",
        |p| {
            p.add(
                "map_black",
                "Map Black To",
                Value::Color(LinearRgba::new(0.0, 0.0, 0.0, 1.0)),
                Unit::None,
            )
            .hide_alpha();
            p.add(
                "map_white",
                "Map White To",
                Value::Color(LinearRgba::new(1.0, 1.0, 1.0, 1.0)),
                Unit::None,
            )
            .hide_alpha();
            p.add(
                "amount",
                "Amount to Tint",
                Value::Scalar(1.0),
                Unit::Normalized,
            )
            .range(0.0, 1.0);
        },
    )
}

fn hue_saturation() -> EffectDef {
    def(
        ids::HUE_SATURATION,
        "Hue / Saturation / Lightness",
        Category::Color,
        "Rotates hues and changes saturation and lightness, or colourises.",
        |p| {
            p.add("hue", "Hue", Value::Scalar(0.0), Unit::Degrees)
                .soft(-180.0, 180.0);
            p.add(
                "saturation",
                "Saturation",
                Value::Scalar(0.0),
                Unit::Normalized,
            )
            .range(-1.0, 1.0);
            p.add(
                "lightness",
                "Lightness",
                Value::Scalar(0.0),
                Unit::Normalized,
            )
            .range(-1.0, 1.0);
            p.add("colorize", "Colorize", Value::Bool(false), Unit::None);
            p.add(
                "colorize_hue",
                "Colorize Hue",
                Value::Scalar(0.0),
                Unit::Degrees,
            )
            .soft(0.0, 360.0);
            p.add(
                "colorize_saturation",
                "Colorize Saturation",
                Value::Scalar(0.25),
                Unit::Normalized,
            )
            .range(0.0, 1.0);
        },
    )
}
