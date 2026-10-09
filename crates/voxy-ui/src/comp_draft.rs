//! The Composition Settings dialog's logic: presets, the draft being
//! edited, validation and readouts. No drawing here, so every rule is unit
//! tested (see `docs/design-comp-settings.md`).

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};
use voxy_core::command::{
    MAX_COMP_DURATION, MAX_COMP_SIZE, MAX_FRAME_RATE, MAX_START_OFFSET, MIN_COMP_SIZE,
};
use voxy_core::timecode;
use voxy_core::{
    CompId, CompSettings, Composition, FolderId, FrameRate, LinearRgba, PixelAspect,
    PreviewResolution, Time, WorkingSpace, linear_to_srgb,
};

use crate::prefs::{DialogMemory, UserPreset};

// ------------------------------------------------------------- presets

/// A named set of size, rate and length.
#[derive(Debug, Clone, PartialEq)]
pub struct Preset {
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub pixel_aspect: PixelAspect,
    pub frame_rate: FrameRate,
    pub duration: Time,
    pub builtin: bool,
}

impl Preset {
    fn matches(&self, d: &Draft) -> bool {
        let Ok(rate) = d.frame_rate() else {
            return false;
        };
        self.width == d.width
            && self.height == d.height
            && self.pixel_aspect == d.pixel_aspect
            && self.frame_rate == rate
            && rate.time_to_frame(self.duration) == rate.time_to_frame(d.duration)
    }
}

impl From<&UserPreset> for Preset {
    fn from(p: &UserPreset) -> Self {
        Preset {
            name: p.name.clone(),
            width: p.width,
            height: p.height,
            pixel_aspect: p.pixel_aspect,
            frame_rate: p.frame_rate,
            duration: p.duration,
            builtin: false,
        }
    }
}

/// `30` for whole rates, `29.97` for NTSC, else up to three decimals.
pub fn format_fps(rate: FrameRate) -> String {
    if rate.den() == 1 {
        return rate.num().to_string();
    }
    let s = format!("{:.3}", rate.as_f64());
    s.trim_end_matches('0').trim_end_matches('.').to_owned()
}

/// Names follow `<ratio> <height>p<fps>`, e.g. `16:9 1080p30`.
fn preset_name(ratio: &str, height: u32, rate: FrameRate) -> String {
    format!("{ratio} {height}p{}", format_fps(rate))
}

/// The presets that ship with voXY: 16:9 at 720, 1080 and 2160 lines, and
/// vertical, square, 4:5 and 21:9 shapes, each at common rates.
pub fn builtin_presets() -> Vec<Preset> {
    let ten = Time::from_secs(10);
    let mut out = Vec::new();
    let mut add = |ratio: &str, w: u32, h: u32, rates: &[FrameRate]| {
        for &r in rates {
            out.push(Preset {
                name: preset_name(ratio, h, r),
                width: w,
                height: h,
                pixel_aspect: PixelAspect::SQUARE,
                frame_rate: r,
                duration: ten,
                builtin: true,
            });
        }
    };
    let all = [
        FrameRate::FPS_24,
        FrameRate::FPS_25,
        FrameRate::FPS_30,
        FrameRate::FPS_60,
    ];
    let social = [FrameRate::FPS_24, FrameRate::FPS_30, FrameRate::FPS_60];
    add("16:9", 1280, 720, &all);
    add("16:9", 1920, 1080, &all);
    add("16:9", 3840, 2160, &all);
    add("9:16", 1080, 1920, &social);
    add("1:1", 1080, 1080, &social);
    add("4:5", 1080, 1350, &social);
    add("21:9", 2560, 1080, &social);
    out
}

/// Longest preset name.
pub const MAX_PRESET_NAME: usize = 64;

/// Checks a name for a user preset.
pub fn validate_preset_name(name: &str, builtins: &[Preset]) -> Result<String, String> {
    let n = name.trim();
    if n.is_empty() {
        return Err("a preset needs a name".into());
    }
    if n.chars().count() > MAX_PRESET_NAME || n.chars().any(char::is_control) {
        return Err("that preset name is not valid".into());
    }
    if n.eq_ignore_ascii_case("custom") || builtins.iter().any(|p| p.name == n) {
        return Err("that name belongs to a built-in preset".into());
    }
    Ok(n.to_owned())
}

// -------------------------------------------------------- pixel aspect

/// Named pixel aspect ratios.
pub const PIXEL_ASPECTS: [(&str, PixelAspect); 6] = [
    ("Square Pixels (1.0)", PixelAspect { num: 1, den: 1 }),
    ("D1/DV NTSC (0.91)", PixelAspect { num: 10, den: 11 }),
    (
        "D1/DV NTSC Widescreen (1.21)",
        PixelAspect { num: 40, den: 33 },
    ),
    ("D1/DV PAL (1.09)", PixelAspect { num: 59, den: 54 }),
    (
        "D1/DV PAL Widescreen (1.46)",
        PixelAspect { num: 118, den: 81 },
    ),
    ("Anamorphic 2:1 (2.0)", PixelAspect { num: 2, den: 1 }),
];

/// The name of `pa` in the dropdown, or `Custom`.
pub fn pixel_aspect_name(pa: PixelAspect) -> &'static str {
    PIXEL_ASPECTS
        .iter()
        .find(|(_, p)| *p == pa)
        .map_or("Custom", |(n, _)| n)
}

/// A typed decimal as a pixel aspect: a named one if within 0.005,
/// otherwise the closest fraction with a denominator up to 100.
pub fn pixel_aspect_from_decimal(x: f64) -> Option<PixelAspect> {
    if !x.is_finite() || x <= 0.0 {
        return None;
    }
    if let Some((_, p)) = PIXEL_ASPECTS
        .iter()
        .find(|(_, p)| (p.as_f64() - x).abs() < 0.005)
    {
        return Some(*p);
    }
    (1..=100u32)
        .map(|den| (((x * f64::from(den)).round()) as u32, den))
        .filter(|(num, _)| *num > 0)
        .min_by(|a, b| {
            let e = |(n, d): &(u32, u32)| (f64::from(*n) / f64::from(*d) - x).abs();
            e(a).total_cmp(&e(b)).then(a.1.cmp(&b.1))
        })
        .map(|(num, den)| PixelAspect { num, den })
}

fn gcd(a: u64, b: u64) -> u64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

/// `16:9` for common shapes, a reduced ratio for small ones, else `1.37:1`.
pub fn ratio_name(w: f64, h: f64) -> String {
    if w <= 0.0 || h <= 0.0 {
        return "—".into();
    }
    let r = w / h;
    for (n, d) in [
        (1u32, 1u32),
        (16, 9),
        (9, 16),
        (4, 3),
        (3, 4),
        (4, 5),
        (5, 4),
        (3, 2),
        (2, 3),
        (21, 9),
        (2, 1),
        (1, 2),
        (185, 100),
        (239, 100),
    ] {
        if (r - f64::from(n) / f64::from(d)).abs() / r < 0.003 {
            return if d == 100 {
                format!("{:.2}:1", f64::from(n) / 100.0)
            } else {
                format!("{n}:{d}")
            };
        }
    }
    format!("{r:.2}:1")
}

/// The ratio of two sizes in whole pixels, e.g. `16:9 (1.78)`.
#[allow(clippy::many_single_char_names)] // w, h, a, b, g are widths, heights and a gcd.
pub fn ratio_text(w: u32, h: u32) -> String {
    let (a, b) = (u64::from(w), u64::from(h));
    let g = gcd(a, b).max(1);
    let exact = if a / g <= 50 && b / g <= 50 && h > 0 {
        format!("{}:{}", a / g, b / g)
    } else {
        ratio_name(f64::from(w), f64::from(h))
    };
    format!("{exact} ({:.2})", f64::from(w) / f64::from(h.max(1)))
}

/// The frame aspect ratio readout: pixel shape included.
pub fn frame_aspect_text(w: u32, h: u32, pa: PixelAspect) -> String {
    let shown = f64::from(w) * pa.as_f64();
    let r = shown / f64::from(h.max(1));
    format!("{} ({r:.2})", ratio_name(shown, f64::from(h)))
}

// ------------------------------------------------------------- readouts

const MIB: f64 = 1024.0 * 1024.0;

/// `540 x 540, 1.1 MB per 8bpc frame`, plus the float16 figure when the
/// project composites in linear light (which uses 16-bit float buffers).
pub fn memory_text(width: u32, height: u32, res: PreviewResolution, space: WorkingSpace) -> String {
    let (w, h) = res.size(width, height);
    let px = f64::from(w) * f64::from(h);
    let mut s = format!("{w} x {h}, {:.1} MB per 8bpc frame", px * 4.0 / MIB);
    if space == WorkingSpace::Linear {
        let _ = write!(s, ", {:.1} MB at float16", px * 8.0 / MIB);
    }
    s
}

/// `is 0:00:30:27 Base 60`.
pub fn timecode_readout(time: Time, rate: FrameRate, drop: bool) -> String {
    let frames = nearest_frame(rate, time);
    format!(
        "is {} Base {}",
        timecode::format_frames(frames, rate, drop),
        rate.timecode_base()
    )
}

/// A color's name: a few plain names, else `#RRGGBB`.
pub fn color_name(c: LinearRgba) -> String {
    let [r, g, b, _] = c.to_array();
    let byte = |v: f32| (linear_to_srgb(v.clamp(0.0, 1.0)) * 255.0).round() as u8;
    let rgb = (byte(r), byte(g), byte(b));
    match rgb {
        (0, 0, 0) => "Black".into(),
        (255, 255, 255) => "White".into(),
        (255, 0, 0) => "Red".into(),
        (0, 255, 0) => "Green".into(),
        (0, 0, 255) => "Blue".into(),
        (255, 255, 0) => "Yellow".into(),
        (0, 255, 255) => "Cyan".into(),
        (255, 0, 255) => "Magenta".into(),
        (128, 128, 128) => "Gray".into(),
        (r, g, b) => format!("#{r:02X}{g:02X}{b:02X}"),
    }
}

// --------------------------------------------------------------- draft

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tab {
    #[default]
    Basic,
    Advanced,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    New { folder: Option<FolderId> },
    Edit(CompId),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Name,
    Width,
    Height,
    PixelAspect,
    FrameRate,
    Start,
    Duration,
    Preview,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Issue {
    pub field: Field,
    pub message: String,
    /// Errors disable OK; warnings only show.
    pub is_error: bool,
}

/// The dialog's two stages: editing, then (for an edit that needs a
/// decision) the prompt about layers and off-grid keyframes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stage {
    Editing,
    Confirm,
}

/// What the dialog knows about the project it edits.
#[derive(Debug, Clone)]
pub struct Context {
    pub existing_names: Vec<String>,
    /// The GPU's largest texture side, when a GPU is available.
    pub max_texture: Option<u32>,
}

/// The settings being edited. Times are authoritative as [`Time`]; the text
/// fields are edit buffers that are read back as the user types.
#[allow(clippy::struct_excessive_bools)] // independent checkboxes of the dialog
#[derive(Debug, Clone)]
pub struct Draft {
    pub mode: Mode,
    pub stage: Stage,
    pub tab: Tab,
    pub name: String,
    pub width: u32,
    pub height: u32,
    pub lock: bool,
    lock_ratio: f64,
    pub pixel_aspect: PixelAspect,
    pub pa_text: String,
    pub fps_text: String,
    pub drop_frame: bool,
    pub resolution: PreviewResolution,
    pub start: Time,
    pub start_text: String,
    pub duration: Time,
    pub duration_text: String,
    pub background: LinearRgba,
    pub keep_layers: bool,
    pub anchor: (u8, u8),
    pub snap_keys: bool,
    pub live_preview: bool,
    /// The name field still has to take focus and select its text.
    pub fresh: bool,
    /// The preset name being typed for "Save preset".
    pub saving_preset: Option<String>,
    pub ctx: Context,
    original: Option<CompSettings>,
}

impl Draft {
    /// A draft of `settings`. For a new composition `original` is `None`.
    pub fn new(
        mode: Mode,
        settings: &CompSettings,
        memory: &DialogMemory,
        tab: Tab,
        ctx: Context,
    ) -> Draft {
        let rate = settings.frame_rate;
        let mut d = Draft {
            mode,
            stage: Stage::Editing,
            tab,
            name: settings.name.clone(),
            width: settings.width,
            height: settings.height,
            lock: memory.lock_aspect,
            lock_ratio: f64::from(settings.width) / f64::from(settings.height.max(1)),
            pixel_aspect: settings.pixel_aspect,
            pa_text: format!("{:.3}", settings.pixel_aspect.as_f64()),
            fps_text: format_fps(rate),
            drop_frame: settings.drop_frame && rate.supports_drop_frame(),
            resolution: settings.preview_resolution,
            start: settings.start_offset,
            start_text: String::new(),
            duration: settings.duration,
            duration_text: String::new(),
            background: settings.background,
            keep_layers: memory.keep_layers,
            anchor: memory.anchor,
            snap_keys: memory.snap_keys,
            live_preview: memory.live_preview,
            fresh: true,
            saving_preset: None,
            ctx,
            original: matches!(mode, Mode::Edit(_)).then(|| settings.clone()),
        };
        d.reformat_times();
        d
    }

    pub fn is_edit(&self) -> bool {
        matches!(self.mode, Mode::Edit(_))
    }

    /// The frame rate typed so far.
    pub fn frame_rate(&self) -> Result<FrameRate, String> {
        let t = self.fps_text.trim().replace(',', ".");
        let x: f64 = t
            .parse()
            .map_err(|_| "enter a frame rate such as 30".to_owned())?;
        if x <= 0.0 {
            return Err("the frame rate must be above 0".into());
        }
        if x > MAX_FRAME_RATE {
            return Err(format!(
                "the frame rate can be at most {MAX_FRAME_RATE} fps"
            ));
        }
        FrameRate::from_decimal(x).ok_or_else(|| "that frame rate is not valid".to_owned())
    }

    /// The timecode rate to display with: the typed rate, or 30 while the
    /// field holds something unusable.
    fn display_rate(&self) -> FrameRate {
        self.frame_rate().unwrap_or(FrameRate::FPS_30)
    }

    /// Rewrites the Start and Duration text from their times, using the
    /// current rate and drop-frame setting.
    pub fn reformat_times(&mut self) {
        let (rate, drop) = (self.display_rate(), self.drop_frame);
        self.start_text = timecode::format_frames(nearest_frame(rate, self.start), rate, drop);
        self.duration_text =
            timecode::format_frames(nearest_frame(rate, self.duration), rate, drop);
    }

    /// The typed rate was edited. Times stay as they are (in real time);
    /// only their display changes. Drop frame switches off where it does
    /// not exist.
    pub fn set_fps_text(&mut self, text: String) {
        self.fps_text = text;
        if self.frame_rate().is_ok_and(|r| !r.supports_drop_frame()) {
            self.drop_frame = false;
        }
        self.reformat_times();
    }

    pub fn set_frame_rate(&mut self, rate: FrameRate) {
        self.set_fps_text(format_fps(rate));
    }

    /// Whether the Drop Frame dropdown is usable: only 29.97 and 59.94.
    pub fn drop_frame_enabled(&self) -> bool {
        self.frame_rate().is_ok_and(FrameRate::supports_drop_frame)
    }

    pub fn set_drop_frame(&mut self, on: bool) {
        self.drop_frame = on && self.drop_frame_enabled();
        self.reformat_times();
    }

    /// The Duration text was edited; keeps the old time while it cannot be
    /// read.
    pub fn set_duration_text(&mut self, text: String) {
        self.duration_text = text;
        if let Ok(f) = timecode::parse(&self.duration_text, self.display_rate(), self.drop_frame) {
            self.duration = self.display_rate().frame_to_time(f);
        }
    }

    pub fn set_start_text(&mut self, text: String) {
        self.start_text = text;
        if let Ok(f) = timecode::parse(&self.start_text, self.display_rate(), self.drop_frame) {
            self.start = self.display_rate().frame_to_time(f);
        }
    }

    // -- size and aspect lock

    pub fn set_lock(&mut self, on: bool) {
        self.lock = on;
        if on {
            self.lock_ratio = f64::from(self.width) / f64::from(self.height.max(1));
        }
    }

    pub fn set_width(&mut self, w: u32) {
        self.width = w;
        if self.lock && self.lock_ratio > 0.0 {
            self.height = ((f64::from(w) / self.lock_ratio).round() as u32).max(1);
        }
    }

    pub fn set_height(&mut self, h: u32) {
        self.height = h;
        if self.lock && self.lock_ratio > 0.0 {
            self.width = ((f64::from(h) * self.lock_ratio).round() as u32).max(1);
        }
    }

    /// `Lock Aspect Ratio to W:H (1.78)`.
    pub fn lock_label(&self) -> String {
        let ratio = if self.lock {
            // Show the locked ratio, not the one drifted by rounding.
            let h = 1000.0;
            ratio_text((self.lock_ratio * h).round() as u32, h as u32)
        } else {
            ratio_text(self.width, self.height)
        };
        format!("Lock Aspect Ratio to {ratio}")
    }

    pub fn set_pixel_aspect(&mut self, pa: PixelAspect) {
        self.pixel_aspect = pa;
        self.pa_text = format!("{:.3}", pa.as_f64());
    }

    /// Reads the custom pixel aspect text, if usable.
    pub fn set_pa_text(&mut self, text: String) {
        self.pa_text = text;
        if let Some(pa) = self
            .pa_text
            .trim()
            .replace(',', ".")
            .parse::<f64>()
            .ok()
            .and_then(pixel_aspect_from_decimal)
        {
            self.pixel_aspect = pa;
        }
    }

    // -- presets

    /// Index into `presets` of the preset the fields match exactly.
    pub fn matching_preset(&self, presets: &[Preset]) -> Option<usize> {
        presets.iter().position(|p| p.matches(self))
    }

    pub fn apply_preset(&mut self, p: &Preset) {
        self.width = p.width;
        self.height = p.height;
        self.lock_ratio = f64::from(p.width) / f64::from(p.height.max(1));
        self.set_pixel_aspect(p.pixel_aspect);
        self.duration = p.duration;
        self.set_frame_rate(p.frame_rate);
    }

    /// The fields as a user preset called `name`.
    pub fn as_user_preset(&self, name: String) -> Result<UserPreset, String> {
        let s = self
            .settings()
            .map_err(|_| "fix the highlighted fields first".to_owned())?;
        Ok(UserPreset {
            name,
            width: s.width,
            height: s.height,
            pixel_aspect: s.pixel_aspect,
            frame_rate: s.frame_rate,
            duration: s.duration,
        })
    }

    // -- validation

    /// Every problem with the fields, errors first.
    pub fn issues(&self) -> Vec<Issue> {
        let mut out = Vec::new();
        let mut err = |field, m: String| {
            out.push(Issue {
                field,
                message: m,
                is_error: true,
            });
        };
        if self.name.trim().is_empty() {
            err(Field::Name, "a composition needs a name".into());
        }
        for (field, label, v) in [
            (Field::Width, "width", self.width),
            (Field::Height, "height", self.height),
        ] {
            if !(MIN_COMP_SIZE..=MAX_COMP_SIZE).contains(&v) {
                err(
                    field,
                    format!("the {label} must be {MIN_COMP_SIZE} to {MAX_COMP_SIZE} px"),
                );
            }
        }
        let rate = match self.frame_rate() {
            Ok(r) => Some(r),
            Err(m) => {
                err(Field::FrameRate, m);
                None
            }
        };
        let pa = self.pixel_aspect.as_f64();
        if !(0.25..=4.0).contains(&pa) {
            err(
                Field::PixelAspect,
                "the pixel aspect must be 0.25 to 4".into(),
            );
        }
        let r = rate.unwrap_or(FrameRate::FPS_30);
        match timecode::parse(&self.duration_text, r, self.drop_frame) {
            Err(e) => err(Field::Duration, e.to_string()),
            Ok(f) => {
                let t = r.frame_to_time(f);
                if f < 1 {
                    err(
                        Field::Duration,
                        "a composition must be at least one frame".into(),
                    );
                } else if t > MAX_COMP_DURATION {
                    err(
                        Field::Duration,
                        "a composition can be at most 3 hours".into(),
                    );
                }
            }
        }
        match timecode::parse(&self.start_text, r, self.drop_frame) {
            Err(e) => err(Field::Start, e.to_string()),
            Ok(f) if r.frame_to_time(f).flicks().abs() > MAX_START_OFFSET.flicks() => {
                err(
                    Field::Start,
                    "the start must be within 24 hours of zero".into(),
                );
            }
            Ok(_) => {}
        }
        if let PreviewResolution::Custom(n) = self.resolution
            && !(2..=PreviewResolution::MAX_CUSTOM).contains(&n)
        {
            err(
                Field::Preview,
                format!("divide by 2 to {}", PreviewResolution::MAX_CUSTOM),
            );
        }
        // Warnings.
        if let Some(limit) = self.ctx.max_texture
            && (self.width > limit || self.height > limit)
        {
            out.push(Issue {
                field: if self.width > limit { Field::Width } else { Field::Height },
                message: format!(
                    "larger than this GPU's {limit} px texture limit: the viewport shows a reduced preview and full-size renders may fail"
                ),
                is_error: false,
            });
        }
        let edited = self.original.as_ref().map(|o| &o.name);
        if self
            .ctx
            .existing_names
            .iter()
            .any(|n| *n == self.name.trim())
            && edited != Some(&self.name.trim().to_owned())
        {
            out.push(Issue {
                field: Field::Name,
                message: "another item already has this name; that is allowed".into(),
                is_error: false,
            });
        }
        out.sort_by_key(|i| !i.is_error);
        out
    }

    pub fn has_errors(&self) -> bool {
        self.issues().iter().any(|i| i.is_error)
    }

    pub fn error_for(&self, field: Field) -> Option<String> {
        self.issues()
            .into_iter()
            .find(|i| i.field == field && i.is_error)
            .map(|i| i.message)
    }

    /// The settings to apply. Times are rounded to whole frames of the
    /// chosen rate.
    pub fn settings(&self) -> Result<CompSettings, Vec<Issue>> {
        if self.has_errors() {
            return Err(self.issues().into_iter().filter(|i| i.is_error).collect());
        }
        let rate = self.frame_rate().unwrap_or(FrameRate::FPS_30);
        let frames = nearest_frame(rate, self.duration).max(1);
        Ok(CompSettings {
            name: self.name.trim().to_owned(),
            width: self.width,
            height: self.height,
            pixel_aspect: self.pixel_aspect,
            frame_rate: rate,
            duration: rate.frame_to_time(frames),
            background: self.background,
            start_offset: rate.frame_to_time(nearest_frame(rate, self.start)),
            drop_frame: self.drop_frame && rate.supports_drop_frame(),
            preview_resolution: self.resolution,
        })
    }

    // -- edit-mode summary

    pub fn size_changed(&self, comp: &Composition) -> bool {
        (self.width, self.height) != (comp.width, comp.height)
    }

    /// What OK will change, e.g. `Layers scaled: no. Keyframes off-grid: 12`.
    pub fn summary(&self, comp: &Composition) -> String {
        let scaled = self.size_changed(comp) && !comp.layers.is_empty() && !self.keep_layers;
        let off = match self.frame_rate() {
            Ok(r) if r != comp.frame_rate => voxy_core::command::off_grid(comp, r).keyframes,
            _ => 0,
        };
        format!(
            "Layers scaled: {}. Keyframes off-grid: {off}.",
            if scaled { "yes" } else { "no" }
        )
    }

    /// Whether OK must first ask about layers or keyframes.
    pub fn needs_confirm(&self, comp: &Composition) -> bool {
        let layers = self.size_changed(comp) && !comp.layers.is_empty();
        let keys = match self.frame_rate() {
            Ok(r) if r != comp.frame_rate => {
                let og = voxy_core::command::off_grid(comp, r);
                og.keyframes + og.layer_edges > 0
            }
            _ => false,
        };
        layers || keys
    }

    pub fn resize_mode(&self) -> voxy_core::command::ResizeMode {
        if self.keep_layers {
            voxy_core::command::ResizeMode::KeepLayers(voxy_core::command::Anchor::grid(
                self.anchor.0,
                self.anchor.1,
            ))
        } else {
            voxy_core::command::ResizeMode::ScaleToFit
        }
    }

    /// The choices to remember for next time.
    pub fn memory(&self) -> DialogMemory {
        DialogMemory {
            lock_aspect: self.lock,
            keep_layers: self.keep_layers,
            anchor: self.anchor,
            snap_keys: self.snap_keys,
            live_preview: self.live_preview,
        }
    }
}

/// The frame of `rate` nearest to `t`.
fn nearest_frame(rate: FrameRate, t: Time) -> i64 {
    let near = voxy_core::command::nearest_frame_time(rate, t);
    rate.time_to_frame(near)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx() -> Context {
        Context {
            existing_names: vec!["Taken".into()],
            max_texture: Some(8192),
        }
    }

    fn draft() -> Draft {
        Draft::new(
            Mode::New { folder: None },
            &CompSettings::default(),
            &DialogMemory::default(),
            Tab::Basic,
            ctx(),
        )
    }

    #[test]
    fn built_in_presets_follow_the_naming_pattern() {
        let p = builtin_presets();
        assert!(p.len() >= 20);
        let names: Vec<&str> = p.iter().map(|p| p.name.as_str()).collect();
        for n in [
            "16:9 720p24",
            "16:9 1080p25",
            "16:9 1080p30",
            "16:9 2160p60",
            "9:16 1920p30",
            "1:1 1080p60",
            "4:5 1350p24",
            "21:9 1080p30",
        ] {
            assert!(names.contains(&n), "{n}");
        }
        let unique: std::collections::HashSet<_> = names.iter().collect();
        assert_eq!(unique.len(), names.len());
        assert!(p.iter().all(|p| p.builtin));
        // 16:9 1080p30 is 1920x1080 at 30 fps, square pixels.
        let hd = p.iter().find(|p| p.name == "16:9 1080p30").unwrap();
        assert_eq!(
            (hd.width, hd.height, hd.frame_rate),
            (1920, 1080, FrameRate::FPS_30)
        );
    }

    #[test]
    fn choosing_a_preset_fills_the_fields_and_matching_follows_edits() {
        let presets = builtin_presets();
        let mut d = draft();
        let vertical = presets.iter().find(|p| p.name == "9:16 1920p60").unwrap();
        d.apply_preset(vertical);
        assert_eq!((d.width, d.height), (1080, 1920));
        assert_eq!(d.fps_text, "60");
        assert_eq!(d.duration, Time::from_secs(10));
        assert_eq!(
            d.matching_preset(&presets).map(|i| &presets[i].name[..]),
            Some("9:16 1920p60")
        );
        // Editing any field leaves the preset.
        d.set_width(1000);
        assert_eq!(d.matching_preset(&presets), None, "Custom");
        d.set_width(1080);
        assert!(d.matching_preset(&presets).is_some(), "back to the match");
        d.set_fps_text("30".into());
        assert_eq!(
            d.matching_preset(&presets).map(|i| &presets[i].name[..]),
            Some("9:16 1920p30")
        );
        d.set_pixel_aspect(PIXEL_ASPECTS[5].1);
        assert_eq!(d.matching_preset(&presets), None);
    }

    #[test]
    fn user_presets_validate_and_cannot_shadow_built_ins() {
        let b = builtin_presets();
        assert_eq!(validate_preset_name("  Reels  ", &b).unwrap(), "Reels");
        assert!(validate_preset_name("", &b).is_err());
        assert!(validate_preset_name("16:9 1080p30", &b).is_err());
        assert!(validate_preset_name("custom", &b).is_err());
        assert!(validate_preset_name(&"x".repeat(65), &b).is_err());
        let d = draft();
        let up = d.as_user_preset("Mine".into()).unwrap();
        let p = Preset::from(&up);
        assert!(!p.builtin);
        assert_eq!(d.matching_preset(&[p]), Some(0));
    }

    #[test]
    fn aspect_lock_keeps_the_ratio() {
        let mut d = draft(); // 1920x1080
        d.set_width(1000);
        assert_eq!((d.width, d.height), (1000, 1080), "unlocked: independent");
        d.set_width(1920);
        d.set_height(1080);
        d.set_lock(true);
        d.set_width(1280);
        assert_eq!((d.width, d.height), (1280, 720));
        d.set_height(540);
        assert_eq!((d.width, d.height), (960, 540));
        assert_eq!(d.lock_label(), "Lock Aspect Ratio to 16:9 (1.78)");
        d.set_lock(false);
        d.set_width(100);
        assert_eq!(d.height, 540);
        // Rounding never reaches zero.
        d.set_lock(true);
        d.set_width(1);
        assert!(d.height >= 1);
    }

    #[test]
    fn frame_rate_text_to_rational() {
        let mut d = draft();
        for (text, num, den) in [
            ("23.976", 24_000, 1001),
            ("29.97", 30_000, 1001),
            ("59.94", 60_000, 1001),
            ("24", 24, 1),
            ("25", 25, 1),
            ("12,5", 25, 2),
            ("7.123", 7123, 1000),
        ] {
            d.set_fps_text(text.into());
            let r = d.frame_rate().unwrap();
            assert_eq!((r.num(), r.den()), (num, den), "{text}");
        }
        for bad in ["", "abc", "0", "-3", "300", "NaN"] {
            d.set_fps_text(bad.into());
            assert!(d.frame_rate().is_err(), "{bad:?}");
            assert!(d.error_for(Field::FrameRate).is_some());
        }
    }

    #[test]
    fn drop_frame_is_enabled_only_for_ntsc_rates() {
        let mut d = draft();
        for (fps, enabled) in [
            ("23.976", false),
            ("24", false),
            ("25", false),
            ("29.97", true),
            ("30", false),
            ("50", false),
            ("59.94", true),
            ("60", false),
            ("12.5", false),
            ("", false),
        ] {
            d.set_fps_text(fps.into());
            assert_eq!(d.drop_frame_enabled(), enabled, "{fps}");
        }
        d.set_fps_text("29.97".into());
        d.set_drop_frame(true);
        assert!(d.drop_frame);
        assert!(
            d.start_text.contains(';'),
            "semicolons in drop-frame: {}",
            d.start_text
        );
        d.set_fps_text("30".into());
        assert!(!d.drop_frame, "switches off where it cannot exist");
        assert!(!d.start_text.contains(';'));
        d.set_drop_frame(true);
        assert!(!d.drop_frame, "cannot be turned on at 30");
    }

    #[test]
    fn times_stay_real_when_the_rate_changes() {
        let mut d = draft(); // 10 s at 30
        assert_eq!(d.duration_text, "0:00:10:00");
        d.set_fps_text("60".into());
        assert_eq!(d.duration, Time::from_secs(10));
        assert_eq!(d.duration_text, "0:00:10:00");
        d.set_fps_text("25".into());
        assert_eq!(
            timecode_readout(d.duration, FrameRate::FPS_25, false),
            "is 0:00:10:00 Base 25"
        );
        d.set_fps_text("59.94".into());
        // 10 s is 599.4 frames; the display rounds to 599 -> 0:00:09:59.
        assert!(d.duration_text.starts_with("0:00:10:") || d.duration_text.starts_with("0:00:09:"));
        assert_eq!(
            timecode_readout(Time::ZERO, FrameRate::FPS_59_94, false),
            "is 0:00:00:00 Base 60"
        );
    }

    #[test]
    fn duration_and_start_shorthand() {
        let mut d = draft(); // 30 fps
        d.set_duration_text("30".into());
        assert_eq!(d.duration, FrameRate::FPS_30.frame_to_time(30));
        d.set_duration_text("1000".into());
        assert_eq!(d.duration, Time::from_secs(10));
        d.set_duration_text("1.5".into());
        assert_eq!(d.duration, FrameRate::FPS_30.frame_to_time(35));
        d.set_start_text("1:00:00:00".into());
        assert_eq!(d.start, Time::from_secs(3600));
        // Unreadable text keeps the last good time and is an error.
        d.set_duration_text("oops".into());
        assert_eq!(d.duration, FrameRate::FPS_30.frame_to_time(35));
        assert!(d.error_for(Field::Duration).is_some());
        assert!(d.has_errors() && d.settings().is_err());
    }

    #[test]
    fn duration_in_flicks_for_each_common_rate() {
        for (fps, base) in [("24", 24), ("25", 25), ("30", 30), ("60", 60)] {
            let mut d = draft();
            d.set_fps_text(fps.into());
            d.set_duration_text("10:00".into()); // 10 s 0 f
            let s = d.settings().unwrap();
            assert_eq!(s.duration, Time::from_secs(10), "{fps}");
            assert_eq!(s.frame_rate.timecode_base(), base);
        }
        let mut d = draft();
        d.set_fps_text("29.97".into());
        d.set_duration_text("30:00".into()); // 30 s at base 30 = 900 frames
        let s = d.settings().unwrap();
        assert_eq!(s.frame_rate.time_to_frame(s.duration), 900);
    }

    #[test]
    fn validation_limits() {
        let mut d = draft();
        d.width = 15;
        assert!(d.error_for(Field::Width).is_some());
        d.width = 16_385;
        assert!(d.error_for(Field::Width).is_some());
        d.width = 16;
        d.height = 16_384;
        assert!(d.error_for(Field::Width).is_none() && d.error_for(Field::Height).is_none());
        d.set_fps_text("240".into());
        assert!(d.error_for(Field::FrameRate).is_none());
        d.set_fps_text("241".into());
        assert!(d.error_for(Field::FrameRate).is_some());
        d.set_fps_text("30".into());
        d.set_duration_text("0".into());
        assert!(d.error_for(Field::Duration).is_some(), "under one frame");
        d.set_duration_text("4:00:00:00".into());
        assert!(d.error_for(Field::Duration).is_some(), "over 3 hours");
        d.set_duration_text("3:00:00:00".into());
        assert!(d.error_for(Field::Duration).is_none());
        d.name = "  ".into();
        assert!(d.error_for(Field::Name).is_some());
        d.name = "ok".into();
        d.resolution = PreviewResolution::Custom(1);
        assert!(d.error_for(Field::Preview).is_some());
        d.resolution = PreviewResolution::Custom(64);
        assert!(d.error_for(Field::Preview).is_none());
        d.set_pa_text("9".into());
        d.pixel_aspect = PixelAspect { num: 9, den: 1 };
        assert!(d.error_for(Field::PixelAspect).is_some());
    }

    #[test]
    fn warnings_do_not_block_ok() {
        let mut d = draft();
        d.name = "Taken".into();
        d.width = 9000;
        d.height = 1080;
        let issues = d.issues();
        assert!(issues.iter().any(|i| i.field == Field::Name && !i.is_error));
        assert!(
            issues
                .iter()
                .any(|i| i.field == Field::Width && !i.is_error)
        );
        assert!(!d.has_errors(), "duplicate names and big sizes are allowed");
        assert!(d.settings().is_ok());
    }

    #[test]
    fn pixel_aspects() {
        assert_eq!(
            pixel_aspect_name(PixelAspect::SQUARE),
            "Square Pixels (1.0)"
        );
        assert_eq!(pixel_aspect_name(PixelAspect { num: 3, den: 7 }), "Custom");
        assert_eq!(pixel_aspect_from_decimal(0.91), Some(PIXEL_ASPECTS[1].1));
        assert_eq!(pixel_aspect_from_decimal(1.46), Some(PIXEL_ASPECTS[4].1));
        assert_eq!(pixel_aspect_from_decimal(2.0), Some(PIXEL_ASPECTS[5].1));
        let c = pixel_aspect_from_decimal(1.333).unwrap();
        assert!((c.as_f64() - 1.333).abs() < 0.01);
        assert_eq!(pixel_aspect_from_decimal(-1.0), None);
        let mut d = draft();
        d.set_pa_text("1.21".into());
        assert_eq!(d.pixel_aspect, PIXEL_ASPECTS[2].1);
    }

    #[test]
    fn frame_aspect_readouts() {
        assert_eq!(
            frame_aspect_text(1080, 1080, PixelAspect::SQUARE),
            "1:1 (1.00)"
        );
        assert_eq!(
            frame_aspect_text(1920, 1080, PixelAspect::SQUARE),
            "16:9 (1.78)"
        );
        assert_eq!(
            frame_aspect_text(1080, 1920, PixelAspect::SQUARE),
            "9:16 (0.56)"
        );
        // D1 NTSC 720x480 at 0.91 is just under 4:3 (1.33), so it keeps its decimal.
        assert_eq!(
            frame_aspect_text(720, 480, PIXEL_ASPECTS[1].1),
            "1.36:1 (1.36)"
        );
        assert_eq!(ratio_text(1920, 1080), "16:9 (1.78)");
        assert_eq!(ratio_text(1000, 1000), "1:1 (1.00)");
        assert_eq!(ratio_text(1234, 987), "5:4 (1.25)");
        assert_eq!(ratio_text(1500, 1000), "3:2 (1.50)");
        assert_eq!(ratio_text(1731, 1000), "1.73:1 (1.73)");
    }

    #[test]
    fn memory_readout() {
        // The reference example: 1080 x 1080 at half is 540 x 540, 1.1 MB.
        assert_eq!(
            memory_text(
                1080,
                1080,
                PreviewResolution::Half,
                WorkingSpace::SrgbCompat
            ),
            "540 x 540, 1.1 MB per 8bpc frame"
        );
        assert_eq!(
            memory_text(1920, 1080, PreviewResolution::Full, WorkingSpace::Linear),
            "1920 x 1080, 7.9 MB per 8bpc frame, 15.8 MB at float16"
        );
        assert!(
            memory_text(
                1920,
                1080,
                PreviewResolution::Quarter,
                WorkingSpace::SrgbCompat
            )
            .starts_with("480 x 270")
        );
    }

    #[test]
    fn color_names() {
        assert_eq!(color_name(LinearRgba::BLACK), "Black");
        assert_eq!(color_name(LinearRgba::WHITE), "White");
        assert_eq!(color_name(LinearRgba::from_srgb8(255, 0, 0, 255)), "Red");
        assert_eq!(
            color_name(LinearRgba::from_srgb8(18, 52, 86, 255)),
            "#123456"
        );
    }

    #[test]
    fn new_mode_settings_round_trip_through_the_draft() {
        let s = CompSettings {
            name: "Reel".into(),
            width: 1080,
            height: 1350,
            frame_rate: FrameRate::FPS_29_97,
            drop_frame: true,
            start_offset: FrameRate::FPS_29_97.frame_to_time(107_892),
            preview_resolution: PreviewResolution::Third,
            duration: FrameRate::FPS_29_97.frame_to_time(900),
            ..CompSettings::default()
        };
        let d = Draft::new(
            Mode::New { folder: None },
            &s,
            &DialogMemory::default(),
            Tab::Basic,
            ctx(),
        );
        assert_eq!(d.start_text, "1:00:00;00");
        assert_eq!(d.settings().unwrap(), s);
    }

    #[test]
    fn edit_summary_and_confirmation() {
        use voxy_core::command::AddSolidLayer;
        use voxy_core::{Document, History};
        let mut h = History::new(Document::with_comp("T", CompSettings::default()));
        let id = h.current().main_comp.unwrap();
        let comp = h.current().comp(id).unwrap().clone();
        let mem = DialogMemory::default();
        let mut d = Draft::new(
            Mode::Edit(id),
            &CompSettings::of(&comp),
            &mem,
            Tab::Basic,
            ctx(),
        );
        assert!(!d.needs_confirm(&comp), "nothing changed");
        assert_eq!(
            d.summary(&comp),
            "Layers scaled: no. Keyframes off-grid: 0."
        );
        // An empty composition resizes without asking.
        d.set_width(1280);
        assert!(!d.needs_confirm(&comp));
        h.execute(AddSolidLayer {
            comp: id,
            name: None,
            color: LinearRgba::WHITE,
            index: 0,
        })
        .unwrap();
        let comp = h.current().comp(id).unwrap().clone();
        assert!(
            d.needs_confirm(&comp),
            "layers and a new size need a choice"
        );
        d.keep_layers = false;
        assert_eq!(
            d.summary(&comp),
            "Layers scaled: yes. Keyframes off-grid: 0."
        );
    }
}
