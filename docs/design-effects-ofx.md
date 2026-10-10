# Design: effects system and OpenFX hosting

**Status: draft, waiting for the owner's approval. No code has been written
for it.** Milestones: E-A to E-D (built-in effects, the design proposal's
Milestone 5), P-0 (plugin research), and O-A to O-D (the OpenFX host,
Milestones 7 and 8).

The rules from earlier work still hold. Every edit is a command and one undo
step. Time is in flicks. Parameters are `Property` values with
`PropertyDesc` metadata. `voxy-core` has no GPU, FFmpeg or UI code. No Alight
Motion work is done (it is on hold). Icons and wording are original.

Sections marked **Decision needed** list the options with a recommendation.
Nothing behind them is built until the owner chooses.

---

## 0. Starting point

What exists: properties with range, unit and default metadata, keyframes with
the five parametric easings, the per-project working space
(`Linear | SrgbCompat`), the action table and shortcut editor, per-user
preferences, a render plan of composited draws per composition, offscreen
nested compositions with content keys, and the empty `voxy-ofx-*` crates.

What does not exist, and is therefore built here: effects, a registry, blend
modes other than Normal, adjustment layers, per-layer intermediate images,
ROI/RoD, and everything for OpenFX.

What does not exist and is **not** built here: layer masks and track mattes.
**Owner decision (step 0):** each effect has an optional *effect matte*
reference that limits where the effect applies (§1.4). Layer masks and track
mattes are a later milestone. The render order leaves a slot for them, so
adding them later will not change the output of existing projects.

**Owner decision (step 0):** add `Value::Layer(LayerId)` for layer-reference
parameters (§1.2).

---

## 1. Data model (`voxy-core`)

### 1.1 Effect instances

```rust
pub struct EffectId(u64);           // from the document counter, like LayerId

pub struct EffectInstance {
    pub id: EffectId,
    pub type_id: EffectTypeId,      // "voxy.blur.gaussian", "ofx:net.sf.openfx.Invert"
    pub version: EffectVersion,     // built-in: u32; OFX: (major, minor)
    pub name: String,               // user-editable, defaults to the definition's name
    pub enabled: bool,
    pub params: Vec<Property>,      // in schema order; keys are "<param>" (see below)
    pub matte: Option<EffectMatte>,
    pub ofx: Option<OfxInstanceData>, // §6.8, only for OFX effects
    pub state: EffectState,         // Resolved | Missing { raw: serde_json::Value }
}

pub struct Layer { /* … */ pub effects: Vec<EffectInstance> /* index 0 = first applied */ }
```

- Property keys inside an effect are the parameter's own name, for example
  `radius`. They are unique within the effect. The full key shown to the
  graph editor and used in merge keys is `fx:<type_id>/<param>`, or
  `ofx:<pluginId>/<param>` for OFX effects, as the design proposal specifies.
- `Layer::properties()` and `properties_mut()` include effect parameters, so
  the existing commands (set value, add, move or delete keyframes, easing
  presets, the five parametric easings) and the graph editor work on them
  unchanged. That is how "keyframe, undo and save like everything else" is
  met without a parallel code path.
- **`EffectState::Missing`** is a placeholder. When a project names a type
  or version the registry does not have, the effect's JSON object is kept
  verbatim in `raw` and written back unchanged on save. A placeholder renders
  as pass-through, shows a warning badge in the Properties panel and in the
  render report, and its parameters show read-only. It can be removed,
  reordered, enabled or disabled (the only edits that cannot damage data
  voXY does not understand), and those edits only touch the fields voXY
  owns.
- When a plugin becomes available later (installed, then re-scanned), the
  "Resolve missing effects" command turns placeholders into resolved effects
  in one undo step.

### 1.2 Parameter types

All parameter types map onto the existing `Value` and `PropertyDesc`. One
value variant is new (owner-approved): `Value::Layer(Option<LayerId>)`, kind
`ValueKind::Layer`.

| Effect parameter | `Value` | `Unit` | Interpolation |
|---|---|---|---|
| float | `Scalar` | any; `Pixels` scales with preview resolution (§3.6) | all, including the five parametric easings |
| int | `Int` | `None` or `Pixels` | numeric, rounded half-away-from-zero after interpolation |
| bool | `Bool` | — | hold |
| enum | `Enum(u32)` | — | hold |
| color | `Color` (linear, straight alpha) | — | numeric per channel, as solid colour today |
| point | `Vec2` | `Pixels`, layer-local; `spatial: true` | numeric with motion paths |
| angle | `Scalar` | `Degrees` | numeric, unwrapped (720° is two turns) |
| layer reference | `Layer(Option<LayerId>)` | — | hold |

UI-only metadata does not belong in `PropertyDesc`, which is shared by every
property. It lives next to the descriptor in the registry:

```rust
pub struct EffectParamDef {
    pub desc: Arc<PropertyDesc>,
    pub choices: Option<Arc<[Arc<str>]>>,   // enum labels
    pub group: Option<GroupId>,             // collapsible group (OFX groups and pages map here)
    pub gizmo: Option<GizmoKind>,           // Point → on-canvas handle
    pub hint: Option<Arc<str>>,             // tooltip
    pub flags: ParamFlags,                  // integer_display, hide_alpha, read_only, …
}
```

Deleting a layer clears every `Value::Layer` that references it, in the same
undo step. The `DeleteLayers` command is extended for this. Duplicating a
layer keeps its references unchanged.

**Decision needed (D1): Curves, LUT and gradient map** need values that
`Value` cannot hold.

- **Curves.**
  - **(a, recommended)** Add `Value::Curve(Arc<[DVec2]>)`: a monotone-x list
    of control points per channel, using hold interpolation (like After
    Effects, curves are not animated smoothly).
  - (b) Use a fixed number of `Vec2` control points per channel. Those can
    be keyframed, but the number of points is limited.
- **LUT (.cube).**
  - **(a, recommended)** Add a new `AssetKind::Lut` and a parameter of type
    `Value::Asset(Option<AssetId>)`. LUTs then go through relink and the
    Project panel like media.
  - (b) Store a path as a `String`. That bypasses the asset table.
- **Gradient map.** Use three colour stops plus a midpoint scalar, so no new
  value is needed. A general `Value::Gradient` comes with shapes later.

### 1.3 Registry

```rust
pub struct EffectDef {
    pub type_id: EffectTypeId,
    pub version: EffectVersion,
    pub name: Arc<str>,
    pub category: Category,           // Blur, Stylize, Color, Keying, Distort, Transition, Generate, Ofx(group path)
    pub description: Arc<str>,
    pub params: Arc<[EffectParamDef]>,
    pub groups: Arc<[ParamGroup]>,
    pub inputs: InputSpec,            // extra image inputs come from Layer params
    pub space: EffectSpace,           // Working (default) | Linear, §3.5
    pub seeded: bool,                 // uses the deterministic seed, §3.4
    pub origin: EffectOrigin,         // Builtin | Ofx { bundle: PathBuf, index: u32, contexts }
    pub credit: Option<CreditRef>,    // OFX plugins: author and license, §8
}

pub struct EffectRegistry { by_type: BTreeMap<EffectTypeId, Vec<Arc<EffectDef>>> }  // all known versions
```

- The registry is plain data in `voxy-core` (`voxy_core::effects`). The
  built-in definitions (parameter schemas, ranges and ROI rules) are there
  too, as pure functions, so headless tests and the planner can use them
  without a GPU. The WGSL kernels live in `voxy-render`, keyed by `type_id`.
- OFX definitions are produced by `voxy-ofx-host` from cached Describe
  results (§6.3) and registered when the app starts, before a project loads.
- The registry is *not* stored in the `Document`. It is passed to `load`,
  `evaluate` and `plan` as `&EffectRegistry`. The UI keeps one
  `Arc<EffectRegistry>` and replaces it after a re-scan.
- **Built-in versioning.** A definition's version increases whenever its
  parameters or formula change. Loading an older version runs that type's
  migration function, which maps old values to new ones. If the migration is
  lossy, the command report says so. A *newer* version than this build knows
  loads as a placeholder.

### 1.4 Effect matte

```rust
pub struct EffectMatte { pub layer: LayerId, pub channel: MatteChannel /* Alpha | Luma */, pub invert: bool }
```

The effect's output is mixed with its input by the matte layer's coverage,
rendered in the effect layer's local space at the same time:
`out = mix(in, fx(in), m)` in premultiplied working values. A matte layer
that is deleted, or that would create a cycle, is ignored with a warning,
and the effect then applies everywhere.

### 1.5 Adjustment layers

`LayerKind::Adjustment`. Its bounds are the whole composition. Of its
transform, only opacity is used, and the rest of its transform is ignored
in E-D. Between its in and out points it takes the composite of every layer
below it in the same composition, runs its effect stack on that, and
composites the result back with its opacity and blend mode, replacing what
was below. Outside its time range it does nothing. Its own blend mode
applies to the composite of the result over the original.

### 1.6 Commands (one undo step each)

| Command | Notes |
|---|---|
| `AddEffect { layer, index, def }` | Creates parameters from the defaults and assigns new IDs. The command holds an `Arc<EffectDef>`. |
| `RemoveEffects { layer, ids }` | |
| `MoveEffect { layer, id, to_index }` | Drag-reorder. |
| `DuplicateEffects { layer, ids }` | New IDs, inserted after each original. |
| `SetEffectEnabled { layer, ids, enabled }` | |
| `RenameEffect` | |
| `SetEffectMatte` | |
| `PasteEffects { layer, index, items }` | `items` are serialized instances from the clipboard. They get new IDs, layer references to layers that do not exist are cleared, and the report says so. Pasting onto several layers in one action is one step. |
| `ApplyEffectPreset { layer, index, preset }` | A preset is a list of serialized instances (§4.3). |
| `ResolveMissingEffects` | §1.1. |
| `SetBlendMode { layers, mode }` | |
| `NewAdjustmentLayer` | |

Parameter edits reuse the existing property commands, with their merge keys
for scrubbing and drags.

---

## 2. Project format

**Format 0.7.0** (flagged change). New, all optional:

- the layer field `effects`, an array that is omitted when empty;
- `blend_mode` values beyond `"normal"`;
- the layer kind `"adjustment"`;
- the value type `"layer"` (and `"curve"` and `"asset"` if D1 picks them).

**A project is written as 0.7.0 only if it uses one of these.** Otherwise
the existing rule applies (0.6.0 if it uses parametric easings, else 0.5.0).
Older builds refuse 0.7.0 files with the "newer than supported" message.
This goes slightly beyond "only when a project contains effects": blend
modes and adjustment layers also bump the version, because a 0.6.0 build
would misread them.

```json
{
  "id": 42, "type": "voxy.blur.gaussian", "version": 1,
  "name": "Gaussian Blur", "enabled": true,
  "params": [ { "key": "radius", "id": 43, "animation": { "static": { "type": "scalar", "value": 12.0 } } } ],
  "matte": { "layer": 7, "channel": "alpha", "invert": false }
}
```

OFX effects add `"ofx": { "plugin": "<pluginIdentifier>", "version": [1, 2],
"context": "filter", "bundle": "<bundle file name, as a hint only>" }`.
Parameter values are ordinary properties (§6.8).

`docs/voxy-format.md` gets the 0.7.0 row and the effect schema. A fuzz
target exercises the effect JSON.

---

## 3. Render graph (`voxy-render`)

### 3.1 Per-layer chain

The current plan composites each layer straight onto the working target. A
layer with no effects and the Normal blend mode keeps exactly that path, so
**existing goldens cannot change**. Any other layer becomes a small chain:

```
source raster (layer-local space, premultiplied, working space, preview scale)
  → [layer masks: reserved slot, later milestone]
  → effect 1 → effect 2 → … (each mixed by its effect matte, if any)
  → [track matte: reserved slot, later milestone]
  → transform + composite with blend mode and opacity
```

Effects run in **layer-local space**, before the layer's transform, as
specified in design proposal §4.2. A blur radius is therefore in layer
pixels and grows when the layer is scaled up, which matches what users of
other compositors expect. A precomp layer's source raster is its offscreen
child render, which already exists.

`DrawSource` gains `Chain { source, effects: Vec<EffectNode>, blend }`. Each
`EffectNode` holds the evaluated parameters (`Value`s at the frame time),
the input and output rectangles from §3.2, and its content key from §3.3.

### 3.2 Region of definition and region of interest

Each built-in definition provides two pure functions of its evaluated
parameters and the render scale:

- `rod(input_rect) -> Rect`: how far the output can extend. A blur grows it
  by about 3σ, a drop shadow by its offset plus softness, a transform by its
  mapped bounds. Colour effects return the input.
- `roi(output_rect) -> Rect`: which input pixels are needed for an output
  rectangle.

Planning works both ways:

1. **Forward RoD.** The buffer for effect *k* covers `rod_k(rod_{k-1})`,
   starting from the source bounds. A blur near the layer edge therefore
   spreads outside the layer instead of being clipped. Sampling outside an
   input buffer returns transparent (clamp-to-border).
2. **Backward ROI**, from what is visible. The composition rectangle is
   mapped into layer space and padded by the downstream effects' ROIs. Each
   buffer is the RoD intersected with that region, so a 10 000 px glow off
   canvas does no work. Adjustment layers use the same rule in composition
   space: the stack below is rendered into a buffer padded by the
   adjustment chain's ROI. A blur on an adjustment layer at the canvas edge
   therefore sees off-canvas content.
3. **Size cap.** If a buffer would exceed the device's texture limit, that
   layer's chain renders at a reduced scale. The render report records a
   warning, and it never fails.

Tests check that a large gaussian blur on a solid touching the layer edge
spreads past it, and that a blur on an adjustment layer at the composition
edge picks up content from off canvas.

### 3.3 Cache keys

`key(node) = hash(input key, type_id, version, every evaluated parameter as
exact bits, render scale, working space, buffer rectangle, matte key, seed
inputs if seeded)`. The layer's final chain output is cached in a GPU LRU
with a byte budget, like the nested-composition cache. A static effect on a
static layer has the same key at every frame and renders once. A video
layer's key includes the source frame time. Tests change each parameter,
the effect version and the resolution in turn, and check that the key
changes and the frame is re-rendered.

### 3.4 Determinism (flagged change: a new rule for randomness)

- Effects that need randomness use
  `seed = hash(layer id, seed parameter, frame number)`. The frame number is
  `floor(t × fps)` in the layer's composition, computed with exact integer
  flick arithmetic. Grain with "animate" turned off uses frame 0. The
  per-pixel PRNG is an integer hash (PCG) in WGSL, which is bit-exact.
  Effects never read the time of day, thread order or atomics.
- Kernel weights (gaussian taps and similar) are computed on the CPU in
  `f64` with `libm` and uploaded as `f32`. Their order and pass count are
  fixed.
- Tests: render twice and compare bit for bit, and for seeded effects,
  check that different frames give different noise and the same frame gives
  the same noise.

### 3.5 Colour (flagged change: interaction with the working-space setting)

Effects compute on working-space values, as blending already does: linear
light in Linear projects, and sRGB-encoded values in SrgbCompat projects.
Definitions marked `space: Linear` (exposure, glow, and gaussian blur
optionally later) decode to linear, apply, and re-encode, so they mean the
same thing in both modes. `docs/effects.md` lists the space for each
effect. Buffers stay `Rgba16Float` premultiplied.

### 3.6 Preview resolution

Preview and export use the same `evaluate → plan → execute` path. Parameters
with `Unit::Pixels` (radius, offset, stroke width, point positions) are
multiplied by `RenderSettings::scale`. The scaled values stay continuous and
are not rounded, so the look does not change. Only kernels whose taps must
be whole numbers (box blur, pixelate cell size) round, deterministically,
and the docs mark them as "approximate at reduced preview".

### 3.7 Blend modes

Normal keeps the hardware "over" blend. Every other mode draws through a
blend shader that reads the accumulated target (ping-pong between two
working textures). The formulas follow **W3C Compositing and Blending
Level 1**, applied to unpremultiplied colours and then composited
source-over:

`co = cs·(1 − αb) + cb·(1 − αs) + αs·αb·B(Cb, Cs)` (premultiplied result).

| Mode | B(Cb, Cs) |
|---|---|
| normal | Cs |
| multiply | Cb·Cs |
| screen | Cb + Cs − Cb·Cs |
| overlay | hard_light(Cs, Cb) |
| hard light | Cs ≤ 0.5 ? multiply(Cb, 2Cs) : screen(Cb, 2Cs − 1) |
| soft light | W3C formula with D(Cb) |
| add | Cb + Cs (not clamped) |
| subtract | max(Cb − Cs, 0) |
| difference | \|Cb − Cs\| |
| color dodge | Cb = 0 → 0; Cs ≥ 1 → 1; else min(1, Cb / (1 − Cs)) |
| color burn | Cb ≥ 1 → 1; Cs ≤ 0 → 0; else 1 − min(1, (1 − Cb) / Cs) |
| lighten / darken | max / min |
| hue, saturation, color, luminosity | W3C SetLum, SetSat and ClipColor |

- Add and subtract are not in W3C; their formulas above are voXY's.
- Modes defined for [0, 1] (screen, overlay, hard and soft light, dodge,
  burn and the four HSL modes) clamp their unpremultiplied inputs to [0, 1].
  The others also work on HDR values.
- The HSL modes use the W3C luma weights (0.30, 0.59, 0.11), so test values
  can be checked against the published reference.

Tests compare each mode with a CPU reference implementation of the same
formulas over a grid of inputs, plus a handful of hand-computed values.

---

## 4. UI (`voxy-ui`)

### 4.1 Effects & Presets panel

A new dock tab with a search field and a category tree: built-ins by
category, OFX plugins under their plugin group path, and user presets. Each
category has an original icon. Double-click applies the effect to the
selected layers, and dragging it onto a timeline layer or the viewport
applies it to that layer. An OFX entry shows the plugin's author and
license from the credits data (§8), and a warning icon if it is blocklisted.

### 4.2 Effect Controls in the Properties panel

There is one collapsible section per effect. Its header has the enable
toggle, the name (double-click to rename), a solo button, a reset button and
a drag handle for reordering. OFX groups and pages become nested
collapsible sections. Parameter widgets reuse the existing stopwatch,
diamond and arrow controls. The right-click menu offers copy, paste,
duplicate, remove and "Save as effect preset…".

- **Solo** and the **before/after split** are view state. They are not
  stored in the document and not undoable.
  - Solo shows only that effect, with the others bypassed.
  - The split shows the layer with its effects off on the left of a
    draggable divider in the viewport.
  - Both travel in a preview-only field of `RenderSettings`. Export always
    passes the default, so export never sees them and preview and export
    still share one path.
- **Gizmos.** Parameters with `gizmo: Point` draw a handle on the canvas,
  mapped through the layer's world transform. Dragging the handle issues the
  normal set-value command with a merge key.

### 4.3 Presets and actions

- **User effect presets** go in `effect_presets.toml` in the per-user config
  directory. It is a separate file because presets can be large. The loader
  is tolerant like the easing presets: invalid entries are dropped one by
  one and listed in the log. A preset stores type, version, name and the
  parameter animations, without IDs.
- **Actions** in the action table, so they can be rebound: open the effects
  panel, apply the last effect, copy, paste, duplicate, remove, enable or
  disable, move up and move down, save as preset, solo, the before/after
  split, and reset parameters.

---

## 5. Built-in effects: batch order (please confirm)

| Group | Effects | Milestone |
|---|---|---|
| 1 | Blend modes (all 17). Gaussian, box and directional blur. Brightness/contrast, exposure, invert, tint, hue/saturation/lightness. | E-B |
| 2 | Radial and zoom blur. Levels, curves, color balance, LUT (.cube), gradient map, posterize. | E-D |
| 3 | Glow, drop shadow, inner shadow, stroke/outline, pixelate/mosaic, vignette, grain/noise, chromatic aberration. | E-D |
| 4 | Chroma key with spill suppression, luma key, matte choker. | E-D |
| 5 | Transform, mirror, wave, displacement map, ripple, bulge/pinch, tile. | E-D |
| 6 | Adjustment layers, then the crossfade, wipe, slide and zoom transitions. | E-D |

Group 1 comes first because it exercises the whole graph: ROI growth
(blurs), multipass (separable gaussian), plain per-pixel effects (colour)
and the blend ping-pong. Everything later reuses that machinery.

**Transitions.** A transition effect has a `progress` parameter (0–1,
keyframable) and an `auto` toggle (default on), which sets progress to
`(t − in) / (out − in)` over the layer's duration. Input A is the effect's
input: the layer itself, or the stack below for an adjustment layer. Input B
is a layer-reference parameter, and transparent if empty. So a transition
works on an adjustment layer (below to B) or on a pair of layers (A to B).

`docs/effects.md` documents each effect's formula, parameter ranges, units,
colour space and ROI rule. It grows with each group.

---

## 6. OpenFX host

### 6.1 Headers and bindings

- **OpenFX 1.5.1**, tag `OFX_Release_1.5.1`, commit
  `ab779510b2655b4d11a7e01e5c521f9aa8c88976`, BSD-3-Clause ("Copyright (c)
  2025, OpenFX and contributors to the OpenFX project").
- Only `include/*.h` and `LICENSE.md` are vendored into `third_party/openfx/`,
  with a `VERSION` file recording the tag, the commit and a SHA-256 of each
  file. BSD-3 allows copying them.
- `cargo xtask bindgen` writes `crates/voxy-ofx-sys/src/bindings.rs`, which
  is checked in. The headers only use fixed-width C types, `size_t` and
  pointers, so one file is correct on all four 64-bit targets. bindgen's
  layout tests run on every CI platform and would catch a difference.
- A Linux CI job installs libclang, re-runs the generation and fails on any
  diff.

### 6.2 Discovery (per the spec's "Installation Location" section)

| OS | Order |
|---|---|
| Windows | `OFX_PLUGIN_PATH` (split on `;`), then `CSIDL_PROGRAM_FILES_COMMON\OFX\Plugins`, then `C:\Program Files\Common Files\OFX\Plugins` |
| macOS | `OFX_PLUGIN_PATH` (split on `;`), then `/Library/OFX/Plugins` |
| Linux | `OFX_PLUGIN_PATH` (split on `:`), then `/usr/OFX/Plugins` |

- After those comes the user's extra path from Preferences. Directories are
  scanned recursively, and names starting with `@` are skipped, as the spec
  requires.
- Binaries are at `<name>.ofx.bundle/Contents/<arch>/<name>.ofx`, with
  `<arch>` = `Win64`, `Linux-x86-64`, or `MacOS` (a universal binary). On
  Intel Macs the deprecated `MacOS-x86-64` is the fallback. **This corrects
  the design proposal**, which listed `MacOS-arm-64`: OpenFX 1.5 says there
  is no arm64 folder.
- Building the path list is a pure function `roots(os, env, prefs)` that
  never touches the file system. It is tested for every OS on every CI
  platform.
- The discovery cache is `ofx-cache.json` in the config directory, keyed by
  binary path, mtime and size. It holds the Describe results. Re-scanning on
  demand only re-describes binaries whose key changed.

### 6.3 Loading and Describe happen only in the sandbox

The editor process **never loads a plugin binary**. The sandbox runs `Load`,
`Describe` and `DescribeInContext` and returns a serializable description
(plugin identifier, version, label, group, contexts, clips, and the
parameter tree with all properties). `voxy-ofx-host` turns that into
`EffectDef`s. O-A builds and tests the host suites in-process inside a test
harness only. The same code then runs inside the sandbox in O-B. The
`--ofx-in-process` debug flag from the design proposal stays limited to
that harness.

### 6.4 Suites and actions

- **Suites:** property, image effect, parameter (including parameter sets,
  animation, and keyframe queries at a time), memory, multi-thread, message,
  progress and time line, as in the prompt. The interact suite comes in O-D.
  `fetchSuite` returns null for any suite or version the host does not
  implement.
- **Actions:** all the ones listed in the prompt.
- **Contexts:** filter and generator in O-B, then transition and general in
  O-C.

| OFX context | voXY mapping |
|---|---|
| Filter | `Source` = the effect's input. |
| Generator | Output replaces the input; best on a solid or adjustment layer. |
| Transition | `SourceFrom` = input, `SourceTo` = a layer-reference parameter, `Transition` = progress, as in §5. |
| General | `Source` = input; each extra clip becomes a layer-reference parameter; an optional mask clip uses the effect matte. |

### 6.5 Host decisions (the full list goes in `docs/ofx-host.md`)

Each of these is checked against the spec text in O-A. Where the spec
leaves a choice to the host, the choice is written down there.

- **Pixel depths:** float, short and byte RGBA on the CPU. The host prefers
  float. Conversion to 8 or 16 bit happens on the host side with
  round-half-to-even, which is deterministic. RGB-only and Alpha-only clips
  are converted by the host where the plugin supports them.
- **Premultiplication: Decision needed (D2).**
  - **(a, recommended)** Input clips are always delivered premultiplied
    (`kOfxImagePreMultiplied`), matching voXY's buffers. The host honours
    the output premultiplication the plugin declares in GetClipPreferences:
    if it declares unpremultiplied, the host premultiplies on return.
    Opaque inputs are declared opaque.
  - (b) Deliver unpremultiplied images to plugins that ask for them. The
    spec does not let a plugin choose its input state, so this needs
    guessing.
- **Render scale:** if the plugin sets
  `kOfxImageEffectPropSupportsMultiResolution`, the host passes the preview
  scale. Otherwise that node renders at scale 1 and is resampled.
- **Field order:** always `kOfxImageFieldNone`. voXY is progressive only.
- **Pixel aspect:** the composition's pixel aspect ratio.
- **Colour management:** the host does not advertise any
  (`kOfxImageEffectPropColourManagementStyle` is none). Pixels arrive in
  the project's working space. `docs/ofx-host.md` notes that many plugins
  assume display-referred values, so SrgbCompat projects may match them
  better.
- **Thread safety:** each sandbox runs one action at a time per instance,
  which is valid for every `kOfxImageEffectPluginRenderThreadSafety` level.
  Concurrency across instances is allowed only for `instance safe` and
  `fully safe` plugins. The multi-thread suite runs a fixed-size pool
  (`available_parallelism`) inside the sandbox. A plugin's internal
  threading is not covered by voXY's determinism promise, and the docs say
  so.
- **Abort and cancellation:** the image effect suite's `abort()` and the
  progress suite read an abort flag in shared memory. A plugin that ignores
  it is killed when the timeout expires.

### 6.6 Parameter mapping

| OFX param | voXY |
|---|---|
| Double (by double type: plain, angle, scale, time, X/Y/XY absolute and normalized) | `Scalar` with a matching unit (`Degrees`, `Factor`, `Seconds`, `Pixels`). Normalized coordinates are converted to pixels per the spec's coordinate-system rules. |
| Integer | `Int` |
| Boolean | `Bool` |
| Choice | `Enum` plus labels |
| StrChoice (1.5) | `String` plus the list of options |
| String | `String` |
| RGBA / RGB | `Color` (RGB hides alpha) |
| Double2D/3D, Integer2D/3D | `Vec2`/`Vec3`, with `integer_display` for the integer types |
| Custom | `String`, hold-only (the plugin's own state) |
| Group / Page | Collapsible groups (pages are flattened into sections) |
| PushButton | A button that sends `InstanceChanged` (reason: user edit); it has no value |
| Parametric (curves) | Read-only, with an "unsupported in this version" note |

Animation follows `kOfxParamPropAnimates`. Ranges follow
`kOfxParamPropMin`/`Max` (hard) and `DisplayMin`/`Max` (soft).

### 6.7 Sandbox process and protocol

One process runs per plugin bundle:
`voxy-ofx-sandbox --bundle <path> --channel <name> --shm <path>`.

- **Control channel.** Length-prefixed `postcard` frames carry a versioned
  `Request`/`Reply` enum.
  - Requests from the host: `Hello`, `Describe`, `CreateInstance`,
    `DestroyInstance`, `SetParams`, `Render`, `GetRoD`, `GetRoIs`,
    `IsIdentity`, `GetClipPreferences`, `GetTimeDomain`,
    `InstanceChanged`, `BeginSequence`/`EndSequence`, `Unload`.
  - During an action the sandbox can send nested *queries* back, and the
    host answers them before the action's reply: a parameter value at a
    time outside the snapshot, a clip image at another time, a message, or
    progress.
- **Parameter snapshot.** Each render carries every parameter's value at
  the render time, plus key times per parameter. Keyframe queries are
  therefore local, and other times cost one round trip, as the design
  proposal specifies.
- **Shared memory.** One file-backed mapping per sandbox:
  - a 4 KiB control header: magic, layout version, abort flag, heartbeat
    counter, and a slot table;
  - image slots allocated by the host per render. Each slot is a
    64-byte-aligned buffer described by bounds, row bytes (bottom-up rows,
    as OFX requires), depth, components and premultiplication.
  - The mapping grows by being replaced, and the new name is sent in the
    next request.
- **Timeouts** (defaults, adjustable in Preferences): Describe 10 s, create
  instance 5 s, render 30 s, every other action 5 s. A missing heartbeat
  counts as a hang.
- **Crash handling.** On a crash, a hang or a protocol violation, the host
  kills the process, the effect renders as pass-through with an error
  badge, and the plugin is **blocklisted for the session**. The Effects &
  Presets panel and the effect header show the reason and a "Retry" action,
  which restarts the sandbox cleanly. The design proposal's persistent
  blocklist after three failures is kept, stored in preferences, and
  cleared by Retry.
- **Our own test plugins.** Two deliberately broken plugins, written by us
  in Rust under `crates/voxy-ofx-host/tests/plugins/`: `crash` (null
  dereference in Render) and `hang` (an endless loop in Render). Tests check
  that the editor process is unaffected and that the error reported is
  correct.

**Decision needed (D3): IPC mechanism.**
- **(a, recommended)** Local sockets via the `interprocess` crate (MIT or
  Apache-2.0): Unix domain sockets, or named pipes on Windows. This keeps
  stdout free, because plugins often print to it.
- (b) The child's stdin and stdout pipes, with no extra crate. The sandbox
  would have to move stdout to stderr before loading a plugin, and that is
  fiddly on Windows.

**Decision needed (D4): shared-memory crate.**
- **(a, recommended)** `memmap2` (MIT or Apache-2.0, widely used) over a
  file in a private per-session directory: `/dev/shm` on Linux, the user
  temp directory elsewhere. Both processes open it by path. It is simple
  and portable.
- (b) The `shared_memory` crate, which wraps `shm_open` and
  `CreateFileMapping`. It is less actively maintained.
- (c) Our own wrapper over `rustix` and `windows-sys`. It is the most code.

**Decision needed (D5): GPU render suites** (OpenGL, CUDA, Metal).
- **Recommendation: out of scope for O-A to O-D.** Doing them safely needs
  a GL context in the sandbox and readback, and cross-process GPU sharing is
  not portable. The host advertises no GPU support, so plugins take their
  CPU path.

### 6.8 Persistence

An OFX effect stores the plugin identifier, the plugin version
(major.minor), the context, and its parameters as ordinary properties.
**OpenFX has no general "state blob" API.** A plugin's private state lives
in its Custom parameters (strings) and is saved with them. That is the only
state voXY persists, and it is written down rather than invented. A missing
plugin, or a newer major version, loads as a placeholder (§1.1). A newer
minor version loads normally, as the spec's versioning rules allow.

### 6.9 Render-time behaviour

The host follows GetRegionOfDefinition and GetRegionsOfInterest in the
§3.2 model. When a plugin reports identity, the host passes the input
through (or uses the reported time). The host sends
`BeginSequenceRender`/`EndSequenceRender` around export and playback runs,
with the `interactive` and `sequential` hints set. Every render can be
cancelled and has a timeout. The image round trip is: GPU readback, then
shared memory, then the plugin, then shared memory, then upload.

### 6.10 Not in scope

voXY never bundles commercial plugins. `docs/ofx-host.md` and the README
will say that many commercial plugins expect a specific host and may not
work correctly in voXY.

---

## 7. Third-party plugin licenses

These are the prompt's hard rules, and P-0 applies them.

- Permissive licenses (MIT, BSD, Apache-2.0) can be built in CI and are
  candidates for the optional download.
- Copyleft licenses (GPL, LGPL, AGPL) are never bundled in voXY or linked
  into it. Any use, including CI fixtures, waits for the owner's
  explicit approval with the implications written down.
- Non-commercial, no-redistribution, unclear or commercial licenses are
  rejected.
- Plugin source code is never copied into voXY's tree. Plugins are fetched
  and built at a pinned commit in CI, with checksums recorded.
- CI binaries are test fixtures only. Offering them to users is a separate
  owner decision.

Context: Natron's `openfx-misc` and `openfx-io` are GPL-2.0, as far as I
know, and P-0 will verify that from the license files. The OpenFX
repository's `Examples` and `Support/Plugins` are BSD-3. That would make
the OpenFX samples the main permissive fixtures, with Natron's sets needing
a copyleft decision.

---

## 8. Credits data

- **One file, `third_party/credits.toml`.** Each entry has: id, name,
  authors, upstream URL, version and commit, SPDX license, the license file
  path or URL, the copyright line, its use (`fixture`, `download` or
  `host-api`), and a checksum. OpenFX itself is an entry (`host-api`).
- `cargo xtask credits` generates the README section and the LICENSING.md
  section between marker comments. The About & Credits dialog reads the
  same file through `include_str!`, so there is no third copy.
- `cargo xtask credits --check` runs in CI. It fails if either generated
  block is stale, or if a plugin in the CI fixture list
  (`third_party/ofx-fixtures.toml`) or in the download catalogue is missing
  from `credits.toml`.
- The README states that the plugin projects are not affiliated with or
  endorsed by voXY, and that voXY only hosts them.

---

## 9. Testing (summary)

- **Effects.**
  - New goldens for each effect, with a small tolerance. Linux only, like
    the existing GPU tests. Existing goldens must not change.
  - Range and finiteness grids: no NaN or infinity anywhere across the
    parameter grid.
  - ROI tests (§3.2), cache-key tests (§3.3) and determinism tests (§3.4).
  - A proptest of random effect-stack edits followed by a full undo, which
    must restore the document exactly. It extends `history_props.rs`.
  - Load, save and placeholder round trips that are byte-identical for
    unknown effects.
- **Blend modes:** the CPU reference grid (§3.7).
- **OFX.**
  - The conformance harness runs Describe, the parameter mapping, rendering
    against goldens, and identity detection on each approved plugin.
  - The crash and hang plugins.
  - `cargo-fuzz` targets for IPC frame decoding, property-suite call
    sequences and Describe-result parsing.
  - Discovery path logic for every OS.
  - Sandbox launch tests on all four CI platforms.
- **Credits:** the CI check in §8.
- The PR will say what was verified only on Linux.

---

## 10. Deferred

These go into `docs/deferred.md` when the design is approved:

- layer masks and track mattes (with the reserved slots in §3.1);
- the OFX interact suite (O-D);
- the GPU render suites (D5);
- OFX colour management;
- effects in non-layer-local space;
- adjustment-layer transforms and bounds;
- `Value::Gradient`.

---

## 11. Decisions requested

| # | Question | Recommendation |
|---|---|---|
| D1 | Curves, LUT and gradient-map values | Add `Value::Curve` and `AssetKind::Lut` with `Value::Asset`. Gradient map uses colour stops. |
| D2 | OFX premultiplication | Always deliver premultiplied, and convert the output according to the plugin's declared state. |
| D3 | Sandbox IPC | `interprocess` local sockets with `postcard`. |
| D4 | Shared-memory crate | `memmap2` over a per-session file (`/dev/shm` on Linux). |
| D5 | GPU render suites | Out of scope for O-A to O-D. |
| D6 | Built-in effect batch order (§5) | As listed, group 1 first. |
| D7 | Blend and adjustment layers also trigger format 0.7.0 (§2) | Yes. |
| D8 | HSL blend-mode luma weights | W3C (0.30, 0.59, 0.11). |
