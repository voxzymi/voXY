# Design note: UX overhaul (U-A … U-I)

Goal: make voXY feel like a friendly, browsable, customizable motion editor —
After Effects workflow, Alight Motion approachability — without professional
clutter. This note is the plan the owner approves before U-I is built. U-A …
U-D may start now because nothing in them contradicts an approved decision.

Ground rules carried from the rest of the project, unchanged:

- Every document change goes through the command / undo system.
- `voxy-core` stays free of UI code. Everything in this note that is view
  state or UI preference lives in `voxy-ui`, never in the document or the
  project format.
- Original icons (painted from strokes, no image files) and original wording.
  "voXY" is spelled that way. No Adobe artwork, icons or wording is copied;
  only workflow and behaviour are matched.
- CI green on all four platforms. No PR until the owner asks.
- Nothing in the browsing and customization sections affects rendering or the
  document format. The one exception that *would* touch the format — markers —
  is flagged below and waits for the owner's go-ahead.

---

## 1. What already exists (so nothing is duplicated)

Surveyed on the fresh branch at `origin/main`. The overhaul is mostly
**extension and consolidation**, not green-field: a large amount is already
built and tested.

### Import
- **Single "Import File…" already exists.** `Action::FileImport`
  (`file.import`, bound to **Ctrl/Cmd+I**) opens one picker
  (`EditorState::import_dialog` → `dialogs.pick_media`) that accepts every
  supported type with multi-select, and `import_paths` adds them as **one undo
  step** (`ImportAssets`). Type is detected from contents/extension
  (`media_kind` / `probe_asset`), not chosen by the user. Unreadable files are
  skipped and **named in the status bar**, never a crash.
- **Legacy entries still present:** `Action::LayerImportImage` ("Import Image…")
  and `Action::LayerImportVideo` ("Import Video…") are still wired into the
  Layer menu (`app.rs`) and the action table. These are what section 1 asks to
  remove.
- **OS drag-and-drop largely built** in `drops.rs`: `dropped_files` /
  `hovered_files` are handled onto the **Project panel** (`project_panel::os_drops`,
  import only), the **timeline** and **viewport** (`drops::viewport_drop`,
  import + add layers, snapping, Ctrl sequence / Alt replace / Shift no-snap /
  Esc cancel), and the **empty-state screen** (`empty_state::pool_drop`, creates
  a composition). Live drop highlight and a plan summary label already draw.
  Drop logic itself is decided in `voxy_core::command::resolve_drop` and tested.

### Effects drag-and-drop
- **Built** in `effects_panel.rs`: `EffectDrag` carries an effect; dragging onto
  a **layer in the timeline** applies it; dropped on the **viewport** it applies
  to the selected layer. **Double-click** applies to the selection. **User effect
  presets** exist (`effect_presets.rs`, `effect_presets.toml`).

### Context menus
- **Layer** context menu exists (timeline `name_resp.context_menu`: Open in New
  Tab, etc.) and **effect** context menu exists (`panels.rs`: copy, paste,
  duplicate, remove, save preset). Project panel rows have menus too.

### Panels, layout, preferences
- **Preferences** exist: `prefs.rs`, TOML in the OS config dir, **tolerant
  loader** (missing / unreadable / corrupt → defaults, save failures logged not
  fatal). Today it holds precomp-double-click behaviour, composition presets,
  easing presets, show-tips.
- **Dock layout** persists (`layout.rs`): one JSON string in `preferences.toml`,
  soundness-checked, `egui_dock` docking/floating/tabbing. A **Window menu**
  exists (focus a panel, precomp-open radio, Show Tips, **Reset Layout**).
- **Per-project layout** lives in `ui_state` (`persist.rs`, versioned: open tabs
  and their views).
- **Shortcut editor** exists (`shortcut_editor.rs`): rebind, Reset / Reset All,
  **conflict detection** (`keymap.conflicts()`).
- **Action table** (`actions.rs`) is the single source for menus, tooltips, the
  shortcut dispatcher and the shortcut editor. Every menu item already comes
  from it.
- **Tip of the Day**, **About dialog**, **Project panel** tree with sort / filter
  (`project_view.rs`: `Column`, `SortKey`, `TypeFilter`) / search / missing-file
  detection / details header all exist.

### Gaps this overhaul fills (new work)
- Remove separate Import Image/Video; **content-hash reuse** ("reused … " message —
  `Asset.content_hash` exists but `probe_asset` leaves it `None` and
  `ImportAssets` does not dedup); **paste import** (Ctrl/Cmd+V); **folder drop**
  (no `read_dir` today) with skipped count; **background import** with progress +
  cancel.
- Effect drop: viewport **hit-test** target, multiple selected layers, drop into
  the **Effect Controls** stack at a position, "Select or drop onto a layer"
  message, **not-allowed cursor** on an incompatible layer.
- **Central text-selection fix** + guard test.
- **Empty-area context menus** (timeline / viewport / project) with a **New ▸**
  submenu; **Apply Effect ▸** on a layer; viewport context menu.
- Browsing: Project-panel **grid / compact views + thumbnails**, **breadcrumbs /
  folder navigation**, **smart bins**, **favorites**, **hover preview**, extended
  **details strip**; Effects panel **favorites / recents / previews**; timeline
  **filters / label colours / row height / minimap / quick switcher**;
  **markers** (deferred, needs owner decision); **command palette**; **Help ▸
  shortcuts cheat sheet**.
- Customization: **appearance** prefs (theme / accent / scale / font / density),
  **behavior** prefs, **interface level**, **toolbar customization**, **named
  workspaces**, reopen-closed-panel Window menu, shortcut-editor upgrades
  (search / import / export / presets), one searchable **Settings dialog**, a
  **version field** for migration.

---

## 2. Drop-target table

Who owns each OS/effect drop, what it does, the undo step, and whether it is
built. "New" = this overhaul adds it.

| Source → target | Action | Undo | Status |
|---|---|---|---|
| OS files → Project panel | Import only, into the hovered folder | 1 step (import) | Built |
| OS files → timeline | Import + add layers at drop X / row | 1 step (import + layers) | Built; **folder drop + hash-reuse: new** |
| OS files → viewport | Import + add a layer centred at drop point | 1 step | Built; **folder drop + hash-reuse: new** |
| OS files → empty state | Import + create a composition from the footage | 1 step | Built |
| OS **folder** → any target | Import supported files, report skipped count | 1 step | **New** |
| Clipboard image / paths (Ctrl/Cmd+V) | Import (+ add layers if a comp is open) | 1 step | **New** |
| Project item(s) → timeline / viewport | Add as layer(s); Ctrl sequence / Alt replace | 1 step | Built |
| Effect → layer in timeline | Apply to that layer | 1 step | Built |
| Effect → layer in viewport (hit-test) | Apply to the layer under the cursor | 1 step | **New** |
| Effect → empty area, with selection | Apply to the selection (multi-layer) | 1 step | **New** |
| Effect → Effect Controls stack | Apply at the drop position in the stack | 1 step | **New** |
| Effect → nothing selected | Status: "Select or drop onto a layer" | — | **New** |
| Effect → incompatible layer (e.g. audio-only) | Not-allowed cursor, no apply | — | **New** |

All drops keep the existing feedback: target highlight, a label
("Import 3 files", "Add Gaussian Blur to Layer 3"), snapping, Esc to cancel.

---

## 3. Menu table

Every entry comes from the action table, shows its shortcut, and is rebindable.
Menus open at the cursor, stay on screen, close on Esc / outside click. Items
needing a composition are disabled with a tooltip (existing `Requires`
mechanism). Layers created from a menu are placed at the playhead, on top of the
selection, become selected, one undo step each.

| Menu | Entries | Status |
|---|---|---|
| Timeline, empty area | New ▸ (Solid, Shape, Text, Null/Adjustment); Import File…; Paste; Select All; New Composition | **New** |
| Viewport, empty area | New ▸ (layers at the clicked point); Import File…; Paste; Fit to Window; zoom shortcuts | **New** |
| Project panel, empty area | Import File…; New Composition; New Folder; Select All | **New** |
| On a layer | existing menu + Apply Effect ▸ (categorized, recents on top); Reveal in Project; Duplicate; Rename; Precompose | Extend existing |
| On an effect | copy, paste, duplicate, remove, save preset | Built |

The `New ▸` submenu reuses the existing layer-creation commands
(`Action::NewSolid`, shape / text / null / adjustment as they exist) so each is
already one undo step; the menus only add a *position* (playhead + clicked
point) to them.

---

## 4. Central text-selection approach

Problem: egui `Label`s become selectable in bulk once any selectable text
exists, so click-drag across panels highlights titles, readouts and layer
names.

Approach — do it once, not per widget:
1. A shared helper `ui::label` module in `voxy-ui` with small constructors —
   `noninteractive(text)`, `heading(text)`, `readout(text)` — that build an
   egui `Label` with `.selectable(false)` and the right text style. All new UI
   uses these; existing bare `Label` / `ui.label` calls are migrated in U-C.
2. Set `style.interaction.selectable_labels = false` globally in `theme::apply`,
   so the default for every label is non-selectable. Real inputs (`TextEdit`,
   rename fields, search boxes, number drags while editing, the shortcut editor,
   dialog fields) are unaffected because they are not `Label`s; where selection
   *is* wanted inside a label-like surface (error text, file paths, About), an
   explicit **Copy** button is provided rather than re-enabling selection.
3. **Guard test** (lint-style, section 3 + 10): a test walks the `voxy-ui`
   sources and fails if a `Label::new`/`ui.label(` appears with
   `.selectable(true)` outside an allowlist of real inputs, so new labels can't
   regress. (Source-scan test, since egui has no runtime registry of labels.)

Cross-platform check (Windows/macOS/Linux) that drag across the timeline never
starts a selection, and Ctrl/Cmd+A / Ctrl+C don't copy UI labels, is part of
U-C; what can only be verified under the Linux virtual display is documented.

---

## 5. Preferences schema and storage

Storage stays as today: TOML in the OS config dir, tolerant loader. Two files:
`preferences.toml` (settings + global layout + workspaces) and
`effect_presets.toml` (large). **Per-project** layout stays in `ui_state`.
Browsing/customization preferences never touch the document or project format.

Schema additions (serde, `#[serde(default)]` everywhere so old files load;
unknown keys **preserved** via a catch-all map; invalid values fall back to the
default with a status message). A **`version`** integer gates migration.

```
[meta]            version = 1            # schema version for migration

[appearance]      theme = "dark"         # dark | light | high_contrast
                  accent = "#7C5CFF"     # voXY violet default
                  ui_scale = "auto"      # auto (follow OS) | 1.0 | 1.25 | 1.5 | 2.0
                  font_size = "normal"
                  timeline_density = "normal"   # compact | normal | large

[behavior]        default_still_seconds = 5.0
                  new_layer_at = "playhead"
                  new_layer_length = "comp"
                  snapping = true
                  precomp_double_click = "current_tab"   # existing
                  new_comp_in_tab = true
                  autosave_minutes = 5                   # existing autosave
                  confirmations = true
                  show_tips = true                       # existing

[interface]       level = "standard"     # simple | standard | advanced

[effects]         favorites = [...]      # effect ids
                  recents = [...]
                  hidden_categories = [...]

[browsing]        project_view = "list"  # list | grid | compact
                  thumbnail_size = 96
                  sort_key = "name"
                  sort_ascending = true
                  favorites = [...]       # pool item keys
                  smart_bins = [ { name, filter } ]

[toolbar]         buttons = [...]        # which buttons show

[workspaces]      active = "Edit"
                  list = [ { name, layout = "<dock json>" } ]   # see §6

[shortcuts]       preset = "voxy"        # voxy | ae_style  (plus per-action overrides, existing keymap file)
```

Loading is defensive: a corrupt file → all defaults + a status line; a readable
file with a bad value → that key defaults, others kept; unknown keys survive a
round trip; `version` lower than current runs migration.

---

## 6. Workspace model

- A **workspace** is a named dock layout (panel arrangement), global, stored in
  `preferences.toml` under `[workspaces]`. It reuses the existing `layout.rs`
  JSON encode/decode and soundness check — a workspace is just a named instance
  of what Reset Layout already restores.
- Ship **original default workspaces**: **Edit**, **Effects**, **Animate**,
  **Minimal**. (Names/arrangement are voXY's own.)
- The **Window menu** gains: switch workspace, **Save Workspace…**, **Reset
  Layout** (exists), and — new — reopen a **closed** panel (today it only
  *focuses* an open one; closed panels can't be brought back). Panels can dock /
  float / tab / close via `egui_dock` (already capable).
- **Per-project** layout stays in `ui_state`; **workspaces are global**. On open,
  the project's `ui_state` layout wins if present, else the active workspace.

---

## 8. After Effects-style familiarity — proposal (needs owner approval before U-I)

Each item with a recommendation. Nothing here is built until the owner confirms
which to do; they will be built in small groups in U-I. No Adobe artwork/wording.

| # | Candidate | Recommendation |
|---|---|---|
| 8.1 | **Default panel arrangement**: Project, viewport, timeline, Effect Controls/Properties, Effects & Presets, + Reset Layout | **Do.** Ship as the "Edit" default workspace. Low risk, high familiarity. |
| 8.2 | **Timeline**: trim/drag layer bars, zoom slider, work-area bar, Space play/pause, J/K/L shuttle, I/O in/out, `[` `]` move in/out to playhead, Home/End, editable current-time field | **Do in groups.** Verify which already exist first (transport/`nav_time` has some). Work-area bar is the biggest new piece — propose as its own small step. |
| 8.3 | **Layer properties**: `U` reveal animated, `P/S/R/T/A` for position/scale/rotation/opacity/anchor, stopwatch to keyframe | **Do.** Stopwatch keyframing likely exists; the letter reveals are cheap and very AE-familiar. |
| 8.4 | **Viewport**: Space+drag pan, scroll / Ctrl+scroll zoom, magnification dropdown, safe-area + grid toggles | **Do, mostly.** Confirm current pan/zoom bindings. Safe-area/grid are view-only toggles, no format impact. |
| 8.5 | **Beginner help**: tooltips with shortcuts on every button | **Do.** Falls out of the action table (shortcut already formatted for menus); extend to toolbar buttons. |

Recommended **default set for U-I**: 8.1, 8.3, 8.5 first (cheap, safe, high
familiarity), then 8.2 and 8.4 as small confirmed groups. Awaiting owner
confirmation of the list before building any of U-I.

---

## Markers (flag — needs owner decision)

Markers (section 5, timeline) are in `docs/deferred.md` ("After Milestone F")
and a browsable markers panel would require a **project-format change** (markers
are document data). Per the ground rules this must be confirmed with the owner
before touching the format. Recommendation: schedule markers as a dedicated
step with its own format bump and migration, **after** the owner approves;
until then the markers panel and timeline markers are left out of U-H. The drop
snapping already takes a list of snap times, so markers slot in as one more
source when they land.

## Milestone order

U-A … U-D start now (nothing contradicts an approved decision). U-E, U-F
(customization) build on §5/§6 above. U-G, U-H (browsing) are view-state only.
U-I waits for the §8 approval. Each milestone ends with working software,
`cargo clippy -D warnings`, tests passing, CI green; progress.md updated and a
short per-job CI report (Windows and macOS first) once per milestone.
