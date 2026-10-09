# The `.voxy` project format

**Format version: 0.1.0** (written by voXY 0.1.0, Milestone 1)

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
  "format_version": "0.1.0",
  "generator": "voXY 0.1.0",
  "project": { ... }
}
```

| Field | Meaning |
|---|---|
| `format` | Always `"voxy-project"`. Anything else is not a voXY project. |
| `format_version` | Semantic version of the schema below. |
| `generator` | The program that wrote the file. Informational only. |
| `project` | The document. |

### Version policy

| File's version vs. this build's | What happens |
|---|---|
| Equal | The file loads. |
| Newer | Refused with a clear message, never guessed at or partially loaded. |
| Older | Migrated forward one version at a time, in memory. The file on disk is rewritten only when the user saves. |

0.1.0 is the first version, so no migrations exist yet.

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
| `assets` | array of Asset | |
| `comps` | array of Composition | |
| `root_comp` | ID | The composition the viewport and export show. |
| `foreign` | object | Optional. |
| `next_id` | integer | Greater than every ID in the file. |

### Asset

| Field | Type | Notes |
|---|---|---|
| `id` | ID | |
| `name` | string | |
| `kind` | `"image"` | Video and audio arrive in Milestone 3. |
| `path` | string | See conventions. |
| `pixel_size` | `[w, h]` | Optional. |

### Composition

| Field | Type | Notes |
|---|---|---|
| `id`, `name` | ID, string | |
| `width`, `height` | integer | Pixels, greater than 0. |
| `frame_rate` | `{ "num", "den" }` | For example `{ "num": 30000, "den": 1001 }`. |
| `duration` | flicks | |
| `background` | Property (`comp.background`) | Color. |
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
| `blend_mode` | `"normal"` | More modes arrive in Milestone 5. |
| `flags` | `{ "visible", "locked" }` | |
| `foreign` | object | Optional. |

`kind` is one of:

```json
{ "type": "solid", "color": <Property solid.color>, "size": <Property solid.size> }
{ "type": "image", "asset": <ID> }
```

A solid fills the rectangle from its local origin to `size`. An image is drawn
at its native pixel size from its local origin.

### Transform

| Field | Property key | Value | Unit |
|---|---|---|---|
| `anchor` | `transform.anchor` | vec2 | px |
| `position` | `transform.position` | vec2 | px |
| `scale` | `transform.scale` | vec2 | factor (1.0 = 100%) |
| `rotation` | `transform.rotation` | scalar | degrees, clockwise |
| `opacity` | `transform.opacity` | scalar | 0–1 |

The layer-to-parent matrix is `T(position) · R(rotation) · S(scale) · T(−anchor)`.

### Property

```json
{ "id": 17, "key": "transform.rotation", "static": { "type": "scalar", "value": 45.0 } }
```

- `key` names the property's descriptor: its type, range and unit. Descriptors
  are defined by voXY, not stored in the file.
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

- Non-finite numbers are invalid.
- In 0.1.0 every property is `static`. Keyframed animation arrives in
  Milestone 2 with format 0.2.0.
