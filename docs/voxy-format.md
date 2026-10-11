# The `.voxy` project format

**Format version: 0.7.0** (effects, Milestone E-A; written only when used, see below)

A voXY project is a single UTF-8 JSON file with the extension `.voxy`. Media
the project uses lives in a sibling folder named `<name>.assets/`:

```
MyProject.voxy
MyProject.assets/
    logo.png
    ...
```

`.voxyz` (a zip of both, for sharing) is reserved and arrives with packaging.
`.voxypreset` uses the same schema with a preset root and arrives with presets.

## Envelope

```json
{
  "format": "voxy-project",
  "format_version": "0.4.0",
  "generator": "voXY 0.1.0",
  "project": { ... },
  "ui_state": { ... }
}
```

| Field | Meaning |
|---|---|
| `format` | Always `"voxy-project"`. Anything else is not a voXY project. |
| `format_version` | Semantic version of the schema below. |
| `generator` | The program that wrote the file. Informational only. |
| `project` | The document. |
| `ui_state` | Optional (0.4.0). Editor state: open composition tabs, playheads, zoom, selection, panel state. It never affects rendering, is versioned on its own (`ui_state.version`), and a loader that does not understand it ignores it. Tabs pointing at compositions that no longer exist are dropped. Version 2 (voXY with the Project panel) stores the panel as `project_panel` (`sort`, `descending`, `filter`, `search`, `columns`, `expanded`); version 1 stored it as `media_panel` and still loads. Invalid fields are dropped one by one. |

### Version policy

| File's version vs. this build's | What happens |
|---|---|
| Equal | The file loads. |
| Newer | Refused with a clear message, never guessed at or partially loaded. |
| Older | Migrated forward one version at a time, in memory. The file on disk is rewritten only when the user saves. |

| Version | Change | Migration from the previous version |
|---|---|---|
| 0.1.0 | First format (Milestone 1). Static properties only. | — |
| 0.2.0 | Adds keyframed properties. | None needed: every 0.1.0 file is valid 0.2.0 content. |
| 0.3.0 | Adds video assets and layers, and the optional asset fields `duration` and `frame_rate`. | None needed: every 0.2.0 file is valid 0.3.0 content. |
| 0.4.0 | Media pool and compositions: folders, the optional `main_comp` (replacing `root_comp`), project settings, composition pixel aspect and work area, `precomp` and `placeholder` layers, asset `content_hash`, `audio` and `folder`, and the `ui_state` envelope field. | `root_comp` is renamed `main_comp`. Everything else is new and optional: no folders, square pixels, no work area, default project settings. A 0.3.0 project opens as one main composition with its media at the top level. |
| 0.5.0 | Composition Settings: optional composition fields `start_offset` (flicks), `drop_frame` and `preview_resolution` (`full`, `half`, `third`, `quarter`, `{"custom": n}`), and the same three in `meta.settings.new_comp`. `ui_state.comp_settings.tab`. | None needed: missing fields take their defaults (0, false, full). A 0.4.0 project opens unchanged. |
| 0.6.0 | Parametric easings: a keyframe's `interp` may also be an object with one key: `spring`, `bounce`, `oscillation`, `steps` or `stepped_ramp`, whose value holds that easing's parameters (see `docs/design-graphs.md` §2 and §3 for names and ranges; missing parameters take their defaults; out-of-range values make the file invalid). | None needed: every 0.5.0 file is valid 0.6.0 content. **A project is written as 0.6.0 only if a keyframe uses one of these easings; otherwise it is still written as 0.5.0**, so projects that do not use the feature keep opening in older builds. Older builds refuse 0.6.0 files with the "newer than supported" message. |
| 0.7.0 | Effects: the optional layer field `effects` (see [Effect](#effect)), and the parameter value types `layer`, `curve` and `asset`. | None needed: every 0.6.0 file is valid 0.7.0 content. Also the blend modes other than `normal` and the `adjustment` layer kind. Also the asset kind `lut`. **A project is written as 0.7.0 only if a layer has at least one effect, a blend mode other than `normal`, or is an adjustment layer, or the project has a `lut` asset**; otherwise 0.6.0 or 0.5.0 as above. Older builds refuse 0.7.0 files with the "newer than supported" message. |

Before writing, voXY validates the document and refuses to write an invalid
one. Saving is atomic: the file is written to a temporary file in the same
folder, flushed, then renamed over the target.

## Conventions

- **Time** is an integer count of *flicks*. One second is 705 600 000 flicks.
  Every common frame rate and audio sample rate divides a second of flicks
  exactly.
- **IDs** are unsigned integers taken from one per-document counter
  (`next_id`). They are unique across every node type and stable for the life
  of the document.
- **Colors** are linear-light RGBA objects `{ "r", "g", "b", "a" }` with
  straight (non-premultiplied) alpha.
- **Coordinates** are composition pixels: origin at the top-left, y pointing
  down, positive rotation clockwise.
- **Paths** to assets use `/` separators. Relative paths resolve against the
  folder that contains the `.voxy` file.
- **Unknown data:**
  - Any node may carry a `foreign` object of data voXY does not model, such as
    Alight Motion attributes from Compat 2a. It is preserved verbatim.
  - Properties with unknown `key`s are also preserved.

## `project`

| Field | Type | Notes |
|---|---|---|
| `meta.name` | string | Project name. |
| `meta.working_space` | `"linear"` \| `"srgb_compat"` | Color space that blending and opacity are computed in. Defaults to `linear`. |
| `meta.settings` | Project settings | Optional. Defaults for new items, below. |
| `assets` | array of Asset | |
| `comps` | array of Composition | |
| `folders` | array of Folder | Optional (0.4.0). Project panel folders. |
| `main_comp` | ID or `null` | The composition shown when no other is open, and the default for export. `null` when the project has no compositions: a new project is saved that way (`comps` empty, no tabs) and opens without creating one. Called `root_comp` before 0.4.0. |
| `foreign` | object | Optional. |
| `next_id` | integer | Greater than every ID in the file. |

### Project settings

| Field | Type | Notes |
|---|---|---|
| `new_comp` | object | Settings of a new composition: `name` (base of new names), `width`, `height`, `pixel_aspect` (optional), `frame_rate`, `duration`, `background` (color). |
| `default_still_duration` | flicks | Optional. Length of a still image layer. Absent: until the composition ends. |

### Folder

| Field | Type | Notes |
|---|---|---|
| `id`, `name` | ID, string | |
| `parent` | ID | Optional. Absent: top level. Folders may not contain themselves. |

Items inside a folder have no stored order; the project panel sorts them.

### Asset

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `name` | string | Display name, independent of the file name. |
| `kind` | `"image"`, `"video"` or (0.7.0) `"lut"` | Audio assets arrive later. A video's audio track is not used yet. A `lut` is a 3D `.cube` table for the LUT effect: it is never a layer source, has no pixel size, and its file is read when rendered (a missing or damaged file makes the effect pass the image through). Removing a LUT sets every `asset` parameter that used it to `null`. |
| `path` | string | See conventions. |
| `pixel_size` | `[w, h]` | Optional. |
| `duration` | flicks | Optional. Length of a video. |
| `frame_rate` | `{ "num", "den" }` | Optional. Nominal frame rate of a video. |
| `audio` | `{ "channels", "sample_rate" }` | Optional (0.4.0). |
| `content_hash` | `{ "algo", "size", "digest" }` | Optional (0.4.0). A fingerprint of the file used to suggest relink candidates; never an identity. |
| `folder` | ID | Optional (0.4.0). Absent: top level. |

### Composition

| Field | Type | Notes |
|---|---|---|
| `id`, `name` | ID, string | |
| `width`, `height` | integer | Pixels, greater than 0. voXY creates 16–16384; larger files still load. |
| `pixel_aspect` | `{ "num", "den" }` | Optional (0.4.0). Absent: square pixels. |
| `frame_rate` | `{ "num", "den" }` | For example `{ "num": 30000, "den": 1001 }`. |
| `duration` | flicks | |
| `work_area` | `{ "start", "end" }` | Optional (0.4.0). Flicks, `0 ≤ start < end ≤ duration`. Absent: the whole composition. |
| `background` | Property (`comp.background`) | Color. Shown when the composition is viewed or exported directly; a nested composition is transparent behind its layers. |
| `folder` | ID | Optional (0.4.0). |
| `layers` | array of Layer | Index 0 is the **top** of the stack. |

### Layer

| Field | Type | Notes |
|---|---|---|
| `id`, `name` | ID, string | |
| `kind` | object tagged by `type` | See below. |
| `in_point`, `out_point` | flicks | The layer is active for `in_point ≤ t < out_point`. |
| `start_offset` | flicks | Source time at `in_point`. |
| `parent` | ID | Optional. Must be a layer in the same composition. Cycles are invalid. |
| `transform` | Transform | |
| `blend_mode` | `"normal"`, or (0.7.0) `"multiply"`, `"screen"`, `"overlay"`, `"soft_light"`, `"hard_light"`, `"add"`, `"subtract"`, `"difference"`, `"color_dodge"`, `"color_burn"`, `"lighten"`, `"darken"`, `"hue"`, `"saturation"`, `"color"`, `"luminosity"` | Default `"normal"`. Formulas: `docs/design-effects-ofx.md` §3.7. |
| `effects` | array of Effect | Optional (0.7.0). The effect stack: the first entry is applied first. Absent when empty. |
| `flags` | `{ "visible", "locked" }` | |
| `foreign` | object | Optional. |

`kind` is one of:

```json
{ "type": "solid", "color": <Property solid.color>, "size": <Property solid.size> }
{ "type": "image", "asset": <ID> }
{ "type": "video", "asset": <ID> }
{ "type": "precomp", "comp": <ID>, "speed": 1.0, "looping": false,
  "time_remap": <Property precomp.time_remap, optional>, "collapse": false }
{ "type": "adjustment" }
{ "type": "placeholder", "was": { "kind": "image" | "video" | "comp", "name": "…",
  "pixel_size": [w, h] (optional), "duration": <flicks> (optional) } }
```

A solid fills the rectangle from its local origin to `size`. An image is drawn
at its native pixel size from its local origin, and so is a video. At
composition time `t` a video layer shows the frame on screen at source time
`t − in_point + start_offset`, measured from the video's first frame. Before
the first frame it shows the first frame, after the last frame the last.

A **precomp** layer (0.4.0) shows another composition, drawn at that
composition's size from its local origin. At time `t` it shows child time:
- with `time_remap`: the remap property's value at `t`, in seconds, rounded
  to the nearest flick;
- otherwise `start_offset + (t − in_point) × speed` (`speed` > 0);
- with `looping`, wrapped into `[0, child duration)`.

Child times outside the child composition show nothing. A composition may
not contain itself, directly or through other compositions. `collapse`
renders the child's layers directly into this composition (2D).

A **placeholder** layer (0.4.0) stands in for a source removed from the
project and draws a grey rectangle of `pixel_size` if known.

### Effect

Format 0.7.0. One entry in a layer's `effects` array:

```json
{
  "id": 42, "type": "voxy.blur.gaussian", "version": 1,
  "name": "Gaussian Blur", "enabled": true,
  "params": [ { "key": "radius", "id": 43, "static": { "type": "scalar", "value": 12.0 } } ],
  "matte": { "layer": 7, "channel": "alpha", "invert": false }
}
```

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `type` | string | The effect definition: `voxy.<group>.<name>` for built-ins, `ofx:<pluginIdentifier>` for OpenFX plugins. |
| `version` | integer | The definition's version (built-ins), or the plugin's major version (OpenFX). |
| `name` | string | Shown in the Properties panel; the user can rename it. |
| `enabled` | bool | Default `true`. |
| `params` | array of Property | Optional. Parameters in definition order. Each `key` is the parameter's own name, unique within the effect; its full property key is `fx:<type>/<key>` (built-ins) or `<type>/<key>` (OpenFX). |
| `matte` | `{ "layer", "channel": "alpha" \| "luma", "invert" }` | Optional. Limits where the effect applies. A matte layer that does not exist is ignored. |
| `ofx` | `{ "plugin", "version": [major, minor], "context", "bundle" }` | Optional; OpenFX effects only. `context` is `filter`, `generator`, `transition` or `general`. `bundle` is a hint for the user. |

Any other field is kept and written back unchanged.

An effect this build cannot use (an unknown `type`, a newer `version`, or
parameters it cannot read) loads as a **placeholder**: it renders as
pass-through with a warning, its parameters are read-only, and it is saved
exactly as it was loaded, except for the fields voXY owns (`id`, `name`,
`enabled`). The values are kept exactly, but object keys are written in
sorted order, as for `foreign`. It still needs `id`, `type`, `version` and
`name`; an effect without them makes the file invalid.

When a project loads, each effect is matched with its definition (same
`type` and `version`):

- a parameter the file does not have is added at its default;
- a parameter whose saved value has the wrong type is reset to its
  default, with a warning;
- a parameter the definition does not know makes the effect a placeholder,
  so that no data is dropped;
- a placeholder whose definition has since been installed (for example a
  plugin) is restored.

### Transform

| Field | Property key | Value | Unit |
|---|---|---|---|
| `anchor` | `transform.anchor` | vec2 | px |
| `position` | `transform.position` | vec2 | px |
| `scale` | `transform.scale` | vec2 | factor (1.0 = 100%) |
| `rotation` | `transform.rotation` | scalar | degrees, clockwise |
| `opacity` | `transform.opacity` | scalar | 0–1 |

Other built-in property keys: `solid.color` (color), `solid.size` (vec2,
px), `comp.background` (color), `precomp.time_remap` (scalar, seconds of
child time).

The layer-to-parent matrix is `T(position) · R(rotation) · S(scale) · T(−anchor)`.

### Property

A property is either static:

```json
{ "id": 17, "key": "transform.rotation", "static": { "type": "scalar", "value": 45.0 } }
```

or keyframed:

```json
{
  "id": 9,
  "key": "transform.position",
  "keyframed": [
    {
      "time": 0,
      "value": { "type": "vec2", "value": [100.0, 500.0] },
      "interp": "bezier",
      "ease_in": [0.6666666666666666, 0.6666666666666666],
      "ease_out": [0.3333333333333333, 0.0],
      "spatial": { "in": [0.0, 0.0, 0.0], "out": [50.0, -600.0, 0.0] }
    },
    {
      "time": 1411200000,
      "value": { "type": "vec2", "value": [900.0, 500.0] },
      "interp": "linear",
      "ease_in": [0.6666666666666666, 1.0],
      "ease_out": [0.3333333333333333, 0.3333333333333333]
    }
  ]
}
```

- `key` names the property's descriptor: its type, range, unit, and whether
  it is *spatial* (`transform.position` and `transform.anchor` are).
  Descriptors are defined by voXY, not stored in the file.
- Value types:

  | `type` | `value` |
  |---|---|
  | `bool` | `true` / `false` |
  | `int` | integer |
  | `scalar` | number (finite) |
  | `vec2` | `[x, y]` |
  | `vec3` | `[x, y, z]` |
  | `color` | `{ "r", "g", "b", "a" }` |
  | `enum` | integer |
  | `string` | string |
  | `layer` (0.7.0) | layer ID or `null` |
  | `curve` (0.7.0) | `{ "points": [[x, y], …] }`: 2 to 64 points, `x` strictly increasing, `x` and `y` in `[0, 1]`. Anything else makes the file invalid. |
  | `asset` (0.7.0) | asset ID or `null` |

- Non-finite numbers are invalid.
- `layer`, `curve` and `asset` values, like `bool`, `enum` and `string`,
  hold between keyframes and are never interpolated.

#### Keyframes

| Field | Type | Default | Meaning |
|---|---|---|---|
| `time` | flicks | required | Strictly increasing within a track. A track has at least one key. |
| `value` | value | required | Same kind as the property. |
| `interp` | `"hold"` \| `"linear"` \| `"bezier"`, or (0.6.0) `{"spring": {...}}`, `{"bounce": {...}}`, `{"oscillation": {...}}`, `{"steps": {...}}`, `{"stepped_ramp": {...}}` | `"linear"` | Timing of the segment that **starts** at this key. The parametric forms carry their own parameters and ignore this key's and the next key's ease handles (which are kept). Parameters are validated; see `docs/design-graphs.md`. |
| `ease_in` | `[x, y]` | `[2/3, 2/3]` | Control point `P2` of the segment **arriving** at this key. |
| `ease_out` | `[x, y]` | `[1/3, 1/3]` | Control point `P1` of the segment **leaving** this key. |
| `spatial` | `{ "in": [x,y,z], "out": [x,y,z] }` | absent | Motion-path tangents, relative to the key's value. Used only by spatial properties. |

Ease handles live in normalized segment space: `x` is the fraction of the
segment's duration and must be in `[0, 1]`; `y` is the fraction of the value
change and may leave `[0, 1]` to overshoot. The defaults make a segment
exactly linear.

**Evaluation** of the segment from key `k0` to the next key `k1` at time `t`:

1. `u = (t − t0) / (t1 − t0)`, computed from the integer times.
2. Progress `p`:
   - `hold`: the value is `k0.value` until `t1`;
   - `linear`: `p = u`;
   - `bezier`: `p = y(s)` where `x(s) = u` on the cubic `(0,0)`,
     `k0.ease_out`, `k1.ease_in`, `(1,1)`.
3. Value:
   - Scalars, vectors and colors: component-wise `v0·(1−p) + v1·p`. Colors
     interpolate in linear light with straight alpha.
   - Integers: the same, then rounded half away from zero.
   - Booleans, enums and strings: hold.
   - **Spatial properties with tangents:** the value travels along the cubic
     Bézier `v0, v0 + k0.spatial.out, v1 + k1.spatial.in, v1`, and `p` is
     the fraction of the path's **arc length**. Speed along the path
     therefore follows the ease curve. Progress outside `[0, 1]` continues
     in a straight line along the end tangent.
4. The result is clamped to the property's hard range (for example opacity
   to `0..1`).

Before the first key the value is the first key's value; from the last key
on, it is the last key's. At a key's exact time the result is that key's
value, bit for bit.
