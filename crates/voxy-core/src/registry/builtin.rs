//! Definitions of the built-in effects (parameter schemas).
//!
//! The formulas, units and ranges are documented in `docs/effects.md`, and
//! the WGSL kernels live in `voxy-render`. Changing a parameter or a
//! formula means a new version of the definition plus a migration; the old
//! version stays registered so older projects keep their look.

use std::sync::Arc;

use super::{Category, EffectDef, EffectOrigin, EffectSpace, ParamBuilder, ParamGroup};
use crate::color::LinearRgba;
use crate::effect::EffectTypeId;
use crate::property::Unit;
use crate::value::Value;
use glam::DVec2;

/// Type IDs of the built-in effects.
pub mod ids {
    pub const GAUSSIAN_BLUR: &str = "voxy.blur.gaussian";
    pub const BOX_BLUR: &str = "voxy.blur.box";
    pub const DIRECTIONAL_BLUR: &str = "voxy.blur.directional";
    pub const RADIAL_BLUR: &str = "voxy.blur.radial";
    pub const ZOOM_BLUR: &str = "voxy.blur.zoom";
    pub const BRIGHTNESS_CONTRAST: &str = "voxy.color.brightness_contrast";
    pub const EXPOSURE: &str = "voxy.color.exposure";
    pub const INVERT: &str = "voxy.color.invert";
    pub const TINT: &str = "voxy.color.tint";
    pub const HUE_SATURATION: &str = "voxy.color.hue_saturation";
    pub const LEVELS: &str = "voxy.color.levels";
    pub const POSTERIZE: &str = "voxy.color.posterize";
    pub const CURVES: &str = "voxy.color.curves";
    pub const COLOR_BALANCE: &str = "voxy.color.color_balance";
    pub const GRADIENT_MAP: &str = "voxy.color.gradient_map";
    pub const LUT: &str = "voxy.color.lut";
    pub const VIGNETTE: &str = "voxy.stylize.vignette";
    pub const PIXELATE: &str = "voxy.stylize.pixelate";
    pub const GRAIN: &str = "voxy.stylize.grain";
    pub const CHROMATIC_ABERRATION: &str = "voxy.stylize.chromatic_aberration";
    pub const DROP_SHADOW: &str = "voxy.stylize.drop_shadow";
    pub const INNER_SHADOW: &str = "voxy.stylize.inner_shadow";
}

/// Every built-in definition, in no particular order.
pub fn builtin_definitions() -> Vec<EffectDef> {
    vec![
        gaussian_blur(),
        box_blur(),
        directional_blur(),
        radial_blur(),
        zoom_blur(),
        brightness_contrast(),
        exposure(),
        invert(),
        tint(),
        hue_saturation(),
        levels(),
        posterize(),
        curves(),
        color_balance(),
        gradient_map(),
        lut(),
        vignette(),
        pixelate(),
        grain(),
        chromatic_aberration(),
        shadow(ids::DROP_SHADOW),
        shadow(ids::INNER_SHADOW),
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

fn radial_blur() -> EffectDef {
    def(
        ids::RADIAL_BLUR,
        "Radial Blur",
        Category::Blur,
        "Spins the layer around a centre point, like a turning wheel.",
        |p| {
            p.add("angle", "Angle", Value::Scalar(10.0), Unit::Degrees)
                .range(0.0, 360.0)
                .soft(0.0, 90.0)
                .hint("How far the blur turns, split evenly both ways.");
            p.add("center", "Center", Value::Vec2(DVec2::ZERO), Unit::Pixels)
                .point()
                .hint("In layer pixels. Drag the handle in the viewer.");
        },
    )
}

fn zoom_blur() -> EffectDef {
    def(
        ids::ZOOM_BLUR,
        "Zoom Blur",
        Category::Blur,
        "Streaks the layer out from a centre point, like a fast zoom.",
        |p| {
            p.add("amount", "Amount", Value::Scalar(0.2), Unit::Factor)
                .range(0.0, 0.9)
                .soft(0.0, 0.5)
                .hint("How far each streak reaches towards the centre (0.5 = halfway).");
            p.add("center", "Center", Value::Vec2(DVec2::ZERO), Unit::Pixels)
                .point()
                .hint("In layer pixels. Drag the handle in the viewer.");
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

fn levels() -> EffectDef {
    def(
        ids::LEVELS,
        "Levels",
        Category::Color,
        "Sets the black and white points, then bends the midtones.",
        |p| {
            p.add("channel", "Channel", Value::Enum(0), Unit::None)
                .choices(&["RGB", "Red", "Green", "Blue", "Alpha"]);
            p.add(
                "input_black",
                "Input Black",
                Value::Scalar(0.0),
                Unit::Normalized,
            )
            .range(0.0, 1.0)
            .hint("Tones at or below this become the output black.");
            p.add(
                "input_white",
                "Input White",
                Value::Scalar(1.0),
                Unit::Normalized,
            )
            .range(0.0, 1.0)
            .hint("Tones at this become the output white; brighter ones keep going.");
            p.add("gamma", "Gamma", Value::Scalar(1.0), Unit::Factor)
                .range(0.1, 10.0)
                .soft(0.2, 5.0)
                .hint("Above 1 brightens the midtones, below 1 darkens them.");
            p.add(
                "output_black",
                "Output Black",
                Value::Scalar(0.0),
                Unit::Normalized,
            )
            .range(0.0, 1.0);
            p.add(
                "output_white",
                "Output White",
                Value::Scalar(1.0),
                Unit::Normalized,
            )
            .range(0.0, 1.0);
        },
    )
}

fn posterize() -> EffectDef {
    def(
        ids::POSTERIZE,
        "Posterize",
        Category::Color,
        "Reduces each colour channel to a few flat steps.",
        |p| {
            p.add("levels", "Levels", Value::Int(6), Unit::None)
                .range(2.0, 256.0)
                .soft(2.0, 32.0)
                .hint("Steps per channel, black and white included.");
        },
    )
}

fn curves() -> EffectDef {
    def(
        ids::CURVES,
        "Curves",
        Category::Color,
        "Reshapes the tones with a curve per channel.",
        |p| {
            let identity = || Value::Curve(Arc::new(crate::value::Curve::identity()));
            p.add("master", "RGB", identity(), Unit::None)
                .hint("Applied after the red, green and blue curves.");
            p.add("red", "Red", identity(), Unit::None);
            p.add("green", "Green", identity(), Unit::None);
            p.add("blue", "Blue", identity(), Unit::None);
            p.add("alpha", "Alpha", identity(), Unit::None);
        },
    )
}

fn color_balance() -> EffectDef {
    const RANGES: [(&str, &str, u32); 3] = [
        ("shadow", "Shadows", 0),
        ("mid", "Midtones", 1),
        ("high", "Highlights", 2),
    ];
    let mut d = def(
        ids::COLOR_BALANCE,
        "Color Balance",
        Category::Color,
        "Pushes the shadows, midtones and highlights towards a colour.",
        |p| {
            for (prefix, _, group) in RANGES {
                for (axis, label) in [
                    ("red", "Cyan / Red"),
                    ("green", "Magenta / Green"),
                    ("blue", "Yellow / Blue"),
                ] {
                    p.add(
                        &format!("{prefix}_{axis}"),
                        label,
                        Value::Scalar(0.0),
                        Unit::Normalized,
                    )
                    .range(-1.0, 1.0)
                    .soft(-0.5, 0.5)
                    .in_group(group);
                }
            }
            p.add(
                "preserve_luminosity",
                "Preserve Luminosity",
                Value::Bool(true),
                Unit::None,
            )
            .hint("Keep each pixel's brightness; only its colour moves.");
        },
    );
    d.groups = RANGES
        .iter()
        .map(|(_, label, id)| ParamGroup {
            id: *id,
            label: (*label).into(),
            parent: None,
            open_by_default: true,
        })
        .collect();
    d
}

fn gradient_map() -> EffectDef {
    def(
        ids::GRADIENT_MAP,
        "Gradient Map",
        Category::Color,
        "Replaces each tone with a colour from a three-stop gradient.",
        |p| {
            for (name, label, c) in [
                ("shadow", "Shadows", LinearRgba::from_srgb8(0, 0, 0, 255)),
                (
                    "midtone",
                    "Midtones",
                    LinearRgba::from_srgb8(128, 128, 128, 255),
                ),
                (
                    "highlight",
                    "Highlights",
                    LinearRgba::from_srgb8(255, 255, 255, 255),
                ),
            ] {
                p.add(name, label, Value::Color(c), Unit::None).hide_alpha();
            }
            p.add(
                "midpoint",
                "Midtone Position",
                Value::Scalar(0.5),
                Unit::Normalized,
            )
            .range(0.01, 0.99)
            .hint("The brightness that gets the midtone colour.");
            p.add("amount", "Amount", Value::Scalar(1.0), Unit::Normalized)
                .range(0.0, 1.0);
        },
    )
}

fn lut() -> EffectDef {
    def(
        ids::LUT,
        "Apply LUT",
        Category::Color,
        "Recolours the layer with a 3D lookup table (.cube file).",
        |p| {
            p.add("lut", "LUT", Value::Asset(None), Unit::None)
                .hint("A LUT from the Project panel, or import one.");
            p.add("input", "LUT Input", Value::Enum(0), Unit::None)
                .choices(&["sRGB-encoded", "Linear light"])
                .hint("What the LUT expects; most creative LUTs expect sRGB-encoded values.");
            p.add("amount", "Amount", Value::Scalar(1.0), Unit::Normalized)
                .range(0.0, 1.0);
        },
    )
}

fn vignette() -> EffectDef {
    def(
        ids::VIGNETTE,
        "Vignette",
        Category::Stylize,
        "Darkens (or tints) the layer towards its edges.",
        |p| {
            p.add("amount", "Amount", Value::Scalar(0.5), Unit::Normalized)
                .range(0.0, 1.0);
            p.add("size", "Size", Value::Scalar(0.6), Unit::Normalized)
                .range(0.0, 1.5)
                .hint("Where the darkening starts: 1 is the middle of each edge.");
            p.add("feather", "Feather", Value::Scalar(0.6), Unit::Normalized)
                .range(0.0, 1.5)
                .hint("How gradually it fades in.");
            p.add(
                "color",
                "Color",
                Value::Color(LinearRgba::new(0.0, 0.0, 0.0, 1.0)),
                Unit::None,
            )
            .hide_alpha();
        },
    )
}

fn pixelate() -> EffectDef {
    def(
        ids::PIXELATE,
        "Pixelate",
        Category::Stylize,
        "Turns the layer into large square blocks of colour.",
        |p| {
            p.add("cell", "Block Size", Value::Scalar(10.0), Unit::Pixels)
                .range(1.0, 1000.0)
                .soft(1.0, 100.0)
                .hint("In layer pixels; the blocks line up with the layer's top-left corner.");
        },
    )
}

fn grain() -> EffectDef {
    let mut d = def(
        ids::GRAIN,
        "Grain",
        Category::Stylize,
        "Adds film-like noise that changes every frame.",
        |p| {
            p.add("amount", "Amount", Value::Scalar(0.1), Unit::Normalized)
                .range(0.0, 1.0)
                .soft(0.0, 0.5);
            p.add("size", "Grain Size", Value::Scalar(1.0), Unit::Pixels)
                .range(0.5, 50.0)
                .soft(0.5, 10.0)
                .hint("The side of one grain, in layer pixels.");
            p.add("monochrome", "Monochrome", Value::Bool(true), Unit::None)
                .hint("The same noise in every channel; off gives coloured grain.");
            p.add("animate", "Animate", Value::Bool(true), Unit::None)
                .hint("New noise every frame; off keeps one still pattern.");
            p.add("seed", "Random Seed", Value::Int(0), Unit::None)
                .range(0.0, 1_000_000.0)
                .hint("Another number gives another, equally random pattern.");
        },
    );
    d.seeded = true;
    d
}

fn chromatic_aberration() -> EffectDef {
    def(
        ids::CHROMATIC_ABERRATION,
        "Chromatic Aberration",
        Category::Stylize,
        "Splits red and blue apart towards the edges, like a cheap lens.",
        |p| {
            p.add("amount", "Amount", Value::Scalar(0.01), Unit::Factor)
                .range(0.0, 0.2)
                .soft(0.0, 0.05)
                .hint(
                    "Red is drawn this much larger and blue this much smaller, around the centre.",
                );
            p.add("center", "Center", Value::Vec2(DVec2::ZERO), Unit::Pixels)
                .point()
                .hint("In layer pixels. Drag the handle in the viewer.");
        },
    )
}

/// Drop Shadow and Inner Shadow share their parameters.
fn shadow(id: &str) -> EffectDef {
    let inner = id == ids::INNER_SHADOW;
    def(
        id,
        if inner { "Inner Shadow" } else { "Drop Shadow" },
        Category::Stylize,
        if inner {
            "Shades the inside of the layer's edges, as if it were cut in."
        } else {
            "Casts a soft shadow of the layer behind it."
        },
        |p| {
            p.add(
                "color",
                "Shadow Color",
                Value::Color(LinearRgba::new(0.0, 0.0, 0.0, 1.0)),
                Unit::None,
            )
            .hide_alpha();
            p.add("opacity", "Opacity", Value::Scalar(0.5), Unit::Normalized)
                .range(0.0, 1.0);
            p.add("direction", "Direction", Value::Scalar(45.0), Unit::Degrees)
                .soft(-180.0, 180.0)
                .hint("Where the shadow falls: 0° is right, positive angles turn clockwise.");
            p.add("distance", "Distance", Value::Scalar(5.0), Unit::Pixels)
                .range(0.0, 1000.0)
                .soft(0.0, 100.0);
            p.add("softness", "Softness", Value::Scalar(5.0), Unit::Pixels)
                .range(0.0, 500.0)
                .soft(0.0, 50.0)
                .hint("Blur radius of the shadow, in layer pixels.");
            if !inner {
                p.add("shadow_only", "Shadow Only", Value::Bool(false), Unit::None);
            }
        },
    )
}
