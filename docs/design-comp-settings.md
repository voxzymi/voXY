# Design note: Composition Settings dialog (Milestone C)

**Status: implemented.** Extends [`design-media-comps.md`](design-media-comps.md)
and [`design-project-panel.md`](design-project-panel.md); every approved
decision there still holds. All changes go through commands and undo,
`voxy-core` has no UI code, and time stays in flicks. Layout and workflow follow
After Effects; icons, wording and artwork are voXY's own.

## 1. What opens it

| Entry point | Mode |
|---|---|
| File > New > Composition, `Ctrl/⌘+N`, the Project panel buttons (toolbar, footer, context menu) | New |
| Composition > Composition Settings…, `Ctrl/⌘+K`, Project panel item menu > Composition Settings… | Edit |

The empty-state tile (P-C) will use the same New mode.

## 2. Fields

One modal window, **Basic** and **Advanced** tabs. There is no 3D Renderer tab
because voXY has no 3D; the tab will appear with the feature.

**Name** is focused and fully selected on open. The default is `Comp N` with the
next unused number (from the project's default name). Duplicate names are
allowed (Q1); the dialog shows a note.

**Basic**

| Field | Behavior |
|---|---|
| Preset | Built-in presets plus the user's. Choosing one fills width, height, pixel aspect, frame rate and duration. Editing any field shows `Custom`, or the preset the fields now match exactly. Save preset names the current values; Delete preset works only on user presets (built-ins show why it is disabled). |
| Width, Height | 16 to 16 384 px (`MAX_COMP_SIZE`). Click to type, or drag the label or value. Shift scrubs ×10, Ctrl/⌘ ×0.1. Beyond the GPU's texture limit (queried from wgpu) a warning shows; it does not block OK. |
| Lock Aspect Ratio | The label shows the ratio (`16:9 (1.78)`). On: editing one side recomputes the other from the ratio captured when the box was ticked, rounded, never below 1. |
| Pixel Aspect Ratio | Square 1.0, D1/DV NTSC 0.91 (10/11), NTSC Widescreen 1.21 (40/33), PAL 1.09 (59/54), PAL Widescreen 1.46 (118/81), Anamorphic 2:1, or Custom (decimal, 0.25 to 4, stored as a fraction with a denominator up to 100). Read-only **Frame Aspect Ratio** shows width × pixel aspect : height. |
| Frame Rate | Typed decimal, a dropdown of 23.976, 24, 25, 29.97, 30, 50, 59.94, 60, "frames per second", and Drop Frame / Non-Drop Frame (enabled only at 29.97 and 59.94; switches off by itself at any other rate). Max 240. |
| Resolution | Preview resolution: Full, Half, Third, Quarter, Custom (1/n, 2 to 64). Readout: `540 x 540, 1.1 MB per 8bpc frame`, from width × height × 4 bytes (MiB), plus `, X MB at float16` (×8 bytes) when the project works in linear light. |
| Start Timecode | Stored as a start offset in flicks. Display only. |
| Duration | At least one frame, at most 3 hours (`MAX_COMP_DURATION`). Rounded to a whole frame of the chosen rate on OK. |
| Background Color | Swatch with the color picker, the name (`Black`, `White`, a few plain names, else `#RRGGBB`). The eyedropper is disabled with a tooltip: egui cannot sample pixels outside voXY. |

Readouts: `is 0:00:30:27 Base 60` under Start and Duration. The base is the rate
rounded (24 for 23.976, 30 for 29.97, 60 for 59.94) and updates live. Drop-frame
uses `;` before the frames. **When the frame rate changes, times keep their real
duration**; only the displayed frames change.

**Advanced** holds only what voXY supports today: whether a size change keeps
layers (with the 3×3 anchor grid, default centre) or scales them to fit, and
whether frame-rate changes snap keyframes and layer edges to the nearest frame.
These choices are remembered, and the prompt after OK starts from them.

**Footer.** *Preview* (edit mode): the viewport shows what OK would produce,
without touching the document; Cancel therefore has nothing to revert. *OK*
(default, Enter from any single-line field) and *Cancel* (Esc, or the close
button). Tab order follows the visual order and egui draws the focus ring.

## 3. Behavior

- **New:** OK runs one `CreateComposition`, opens the composition, selects it in
  the Project panel (no inline rename: the name was set). One undo removes it and
  restores the earlier panel selection and open composition. Cancel changes
  nothing.
- **Edit:** one `SetCompositionSettings` on OK, with the preview or without.
- **Size changed on a composition with layers**, or **frame rate changed with
  keyframes or layer edges that would fall between frames:** OK first shows one
  prompt (keep or scale layers, with the anchor grid; snap or leave off-grid)
  with the counts. *Apply* runs the command; *Back* returns to the fields.
- **Summary line** (edit mode): `Layers scaled: no. Keyframes off-grid: 12.`
- **Validation is live.** Invalid fields get a red outline and a message and OK is
  disabled. Warnings (GPU limit, duplicate name) do not block.
- Dialog state lives in `EditorState`, so a window resize never loses edits.

## 4. Timecode parsing

`voxy_core::timecode::parse(text, rate, drop)` returns a frame number.

1. Spaces are ignored; a leading `-` makes it negative (a start may be before 0).
2. With separators (`:`, `;`, `.`) the fields are read **from the right**:
   frames, seconds, minutes, hours. Missing left fields are 0 and a field may
   exceed its range (it carries). `1.5` is 1 second 5 frames; `0:90` is 90 frames;
   `0:90:00` is 90 seconds.
3. Digits only: split into pairs from the right as frames, seconds, minutes,
   hours. `30` is 30 frames, `130` is 1 s 30 f, `1000` is 10:00, `10000` is 1:00:00
   minutes.
4. At most 8 digits and four fields; anything else is an error with a message.
5. Drop-frame: the fields are read as a drop-frame timecode (2 numbers skipped
   per minute except every tenth at 29.97, 4 at 59.94). A label that does not
   exist (`0:01:00;00`) maps to the frame two before `;02`.

Formatting is `H:MM:SS:FF` (`;` before FF for drop-frame); parse and format
round-trip for every supported rate.

## 5. Frame rate from a decimal

`FrameRate` was already a rational, so there is **no schema change for rates**.
`FrameRate::from_decimal`:

- within 0.0006 of 23.976, 29.97, 59.94 or 119.88: the exact `N/1001`;
- within 0.0001 of a whole number: that integer;
- otherwise the closest fraction with a denominator up to 1001 (continued
  fractions): 12.5 is 25/2, 7.123 is 7123/1000.

The dialog shows `Stored as n/d fps` when the result is not a whole or NTSC rate.
*Alternative considered:* storing the typed decimal. Rejected: it cannot be
exact and breaks frame↔time rounding.

## 6. Preview resolution

Stored on the composition (`preview_resolution`) and saved, but it is a
**render-time scale for the viewport only**: the viewport renders at
`min(fit-to-panel, 1/n)`. It never changes the composition's size, and exports
use `RenderSettings::default()` (full size) unless the export dialog overrides
it. Evaluation ignores it, so an export's content key cannot depend on it; the
viewport's frame key holds the output size, so changing it re-renders previews
(tests in `viewport.rs` and `tests/comp_settings.rs`).
*Note:* the prompt calls it "a view setting" and also lists it among the
composition's fields; it is a composition field so it is saved with the project.

## 7. Data model and format 0.5.0

Added to `Composition` and `CompSettings` (so also the project's new-composition
defaults), all optional with defaults: `start_offset` (flicks), `drop_frame`,
`preview_resolution`. `pixel_aspect` already existed. Defaults are not written.
The format version is **0.5.0**; 0.4.0 files open unchanged. See `voxy-format.md`.
Drop-frame is rejected unless the rate is 29.97 or 59.94, and a start offset is
limited to ±24 h.

## 8. Persistence and preferences

- `ui_state` gains `comp_settings.tab` (the last tab).
- User presets and the dialog's remembered choices (lock aspect, keep or scale,
  anchor, snap, Preview) live in `preferences.toml` in the per-user config
  directory (`$XDG_CONFIG_HOME` or `~/.config`, `~/Library/Application Support`,
  `%APPDATA%`, then `voXY/`). A missing, unreadable or corrupt file gives the
  defaults. This is the `Preferences` store approved for P-D; "Show tips" will
  join it. It adds the `toml` crate.
- New mode starts from the project defaults, not from the last values used.

## 9. Flags

- **Undo:** unchanged. Each OK is one command; the preview never touches
  `History`. `EditorState::undo` also restores the panel selection after undoing a
  create.
- **Determinism:** none. Preview scaling is viewport-only.
- **Color space and OFX sandbox:** untouched.
- **Timecode display** now reads the composition's start offset and drop-frame
  flag in the transport, ruler and playhead readouts.

## 10. Tests

`comp_settings.rs` (core): decimals to rationals, drop-frame rules, bases,
flick durations, validation, one-step undo, preview independence, 0.4.0 files.
`timecode.rs`: formatting, drop-frame boundaries, shorthand, round trips.
`comp_draft.rs`: presets and matching, aspect lock, frame rate text, drop-frame
enable rules, shorthand, limits, readouts. `state.rs`: OK, Cancel, Preview,
confirm prompt, snap. `comp_dialog.rs`: drawn at two window sizes in both modes
and tabs, and fitted to a small window. `prefs.rs`: config paths, corrupt files,
round trip.

## 11. Not done

- Eyedropper (disabled with a tooltip).
- 3D Renderer tab (no 3D yet).
- Layer-timing edits when the start timecode moves (it is display-only by design).
