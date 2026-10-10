# Design addendum: Project panel, compositions in the pool, no default composition

**Status: approved.** The owner's decisions are in §10. Milestone B (the Project
panel, folders, Reveal actions, action table) is implemented; see
"Implementation notes (Milestone B)" at the end. Milestones C and P-B to P-D
follow, in that order.

This extends [`design-media-comps.md`](design-media-comps.md) (approved). All its
rules stay: every state change is a `Command` run through `History`, `voxy-core`
has no GPU, FFmpeg or UI code, time is in flicks, and the product is spelled
"voXY". The workflow follows After Effects. Icons, artwork, labels and tip text
are voXY's own; nothing is copied from Adobe.

## 0. What exists today

Facts from the code that shape this addendum:

| Area | Today | Consequence |
|---|---|---|
| Media panel | `Tab::Media` in `voxy-ui/src/panels.rs`: a flat list of assets titled "Media". Folders, search, sort, reveal and drag (Milestone B) are not built. | We build the **Project panel** directly; there is no old panel to port. |
| `MediaPool` | `voxy-ui/src/media.rs`: the video decoder/stream pool, unrelated to the panel. | Not renamed. It is about decoding. |
| Dock layout | `default_layout()` builds a fixed layout; it is not saved. | Layout migration (§7) is a defensive alias layer. No shipped file uses the old name yet. |
| Action table, shortcut editor, preferences | None (`design-media-comps.md` Q8 says the table is built; shortcuts are constants in `app.rs`). | The table must exist before tip validation, menu entries and the no-panic test. See §10 Q4 (ordering). |
| Tabs | None. The editor shows `EditorState::comp_id()` = `main_comp`. | "No active composition" is already `Option` there. Milestone E adds tabs. |
| `Document::new(name, settings)` | **Always creates one composition** and sets `main_comp`. `EditorState::untitled()` calls it. | This is the source of the default "Comp 1". Fixed in §3. |
| `EditorState::require_comp` | Already reports "No composition is open. Create one first." | A starting point for the disabled-with-tooltip rule. |
| Format 0.4.0 | `main_comp` is already nullable; `ui_state` is versioned on its own. | **No format version bump** (§7). |

## 1. Rename "Media" to "Project"

**User-facing:** panel title and tab, Window menu entry, shortcut-editor group,
dialogs, tooltips, docs. "Project panel" is the panel. "Project items" are the
things in it (footage, compositions, folders). "Media" stays only for footage
itself ("Import media", "missing media file").

**In code (cheap and safe):**
- `Tab::Media` → `Tab::Project`; `media_ui` → `project_ui`; new module
  `voxy-ui/src/project_panel.rs`; action IDs `project_panel.*`.
- `voxy_core::PoolItem` keeps its name (it is the item enum, not the panel).
- Strings: "Reveal Layer Source in Project", "Reveal in Composition", "Not used
  in any composition", "Create or open a composition first.", status text
  "N missing file(s)" (was "missing media file(s)").

**Alias and migration layer** (one small module, `voxy-ui/src/compat.rs`):

| Old | New | Where it is applied |
|---|---|---|
| panel id `"Media"` / `"media"` / `"media_panel"` | `Project` | Saved dock layouts, when layout saving lands. Unknown panel names are dropped, never an error. |
| action IDs `media_panel.*`, `media.*` | `project_panel.*` | Saved shortcut maps and the action table's `aliases` field. Lookup by an old ID resolves to the new action. |
| `ui_state` key `media_panel` (design-media-comps §9.2) | `project_panel` | `ui_state` v1 → v2 on load. Saving writes only the new key. |

Because nothing shipped uses the old names, the migration is a defensive layer
with tests (§9), not a data rescue.

## 2. Compositions in the Project panel

**Tree.** Folders, footage and compositions are one tree, with the same folder
membership, rename, remove, duplicate, drag, search, sort and multi-select
rules as before (all already in the command layer from Milestone A:
`RenameComposition`, `MoveToFolder`, `RemoveItems`, `DuplicateComposition`).

**Row for a composition:** composition icon, name, "1920 x 1080, 30 fps",
duration (timecode), usage count (`Document::usages`, memoized per
`History::revision()`).

**Info header** (one item selected): thumbnail plus settings summary.
- Thumbnail for a composition = the viewport renderer at the tab's current
  time if the composition is open, else frame 0, at thumbnail size. It is
  view-only, cached by `(comp, revision, time)`, and never enters `History`.
- Footage keeps its existing thumbnail/metadata.

**Columns** (toggle from the header context menu; stored in `ui_state`):
Name (always on), Type, Size, Frame Rate, Duration, Used, Date Added.
Date Added is the item ID (IDs only increase; design-media-comps §1.2).

**Filter by type:** All, Compositions, Footage, Folders, Missing. Combines with
search. A filter or search hiding a reveal target is cleared with the existing
status message. **Sort by Type** groups: folders, compositions, footage; ties by
name (case-insensitive), then ID. Sorting by any column is stable and
deterministic.

**Behavior:**
- Double-click a composition: open or switch to its own tab (fresh breadcrumb).
- Drag a composition to the timeline or viewport: `DropItems` with a `Comp`
  item, so the existing `would_cycle` check applies (Milestone D).
- **New compositions are auto-selected and scrolled into view**, from any path.
  The UI reads `CommandReport.created` after `execute_reported`, so this is
  view state and never a command. The "New Composition" *button* additionally
  opens inline rename; other paths (menu, tile, drop, precompose) do not.
- **Pre-compose** (`PrecomposeLayers`) puts the new composition in a root
  folder named "Pre-comps". The command finds a root folder with that exact
  name, or creates it, **inside the same command** (still one undo step).
  The folder is found by name, so if the user renames it, the next pre-compose
  creates a new "Pre-comps". This needs no schema change.

  > **Heads-up (decided):** the "Pre-comps" folder is matched by its exact name
  > at the top level. If you rename or move it, the next pre-compose makes a
  > new "Pre-comps" folder instead of reusing yours. Compositions already in
  > your folder stay where they are.

## 3. No active composition

### 3.1 Representation

- **Core never has an "active" composition.** Every command and query already
  takes an explicit `CompId`. `Document::main_comp: Option<CompId>` stays
  optional (Q9). Nothing in core may call `unwrap` on it.
- **New constructors.** `Document::empty(name)` makes a project with zero
  compositions, no assets and `main_comp = None`. The current
  `Document::new(name, settings)` is renamed `Document::with_comp(name,
  settings)`; it is test and fixture scaffolding, and the app never calls it.
  This is a mechanical rename in tests (about a dozen call sites).
- **UI.** `EditorState::active_comp() -> Option<CompId>`: `main_comp` until tabs
  land, then the active tab's composition. `None` when there are no tabs.
  `EditorState::untitled()` and `new_document()` use `Document::empty`.
- **Nothing creates a composition implicitly.** Not New Project, first launch,
  deleting the last composition (`RemoveItems` promotes the lowest remaining ID
  to `main_comp`, or sets `None`), importing, or opening a file with none.
- **Legacy files.** A 0.3.0 file always has `root_comp`, so it migrates to one
  composition (existing migration test stays). A file with `main_comp: null`
  and no compositions opens with none. A file whose `main_comp` points at a
  missing composition already fails validation; unchanged.

### 3.2 Components that must handle `None`

| Component | Behavior with no active composition |
|---|---|
| Timeline panel | Empty-state screen (§4). No ruler, no layers. |
| Viewport | "No composition open" message and compact buttons (§4.4). No render call, no frame cache lookup. |
| Properties / Graph editor | A weak "No composition open" label. |
| Transport and playback | Play, step and seek are disabled. Playhead resets to 0. Audio engine stays idle. |
| Render graph | `RenderPlan::build` and `evaluate` already take a `&Composition`; callers must not be reached with `None`. Thumbnails of a composition use that composition's own ID. |
| Export | The dialog does not exist yet. Contract: export takes an explicit `CompId`, defaulting to the active one, then `main_comp`. If there is neither, it shows "Nothing to export: create or open a composition first." and returns without running. |
| Autosave (deferred) | `save_with_ui` writes an empty project fine. No code path may require a composition. |
| Menus and shortcuts | Every action declares `requires: Requires::{Nothing, Composition, Layers}`. A `Composition`/`Layers` action with none active is disabled with the tooltip "Create or open a composition first." and, if invoked by shortcut, shows the same text in the status bar. It never panics. |
| Reveal | `reveal.in_composition` stays enabled for a selected project item (it opens a tab); layer-based reveals need a selection. |

### 3.3 The no-panic test

The action table is the single registry of everything in menus and shortcuts.
One test builds `EditorState` over `Document::empty`, iterates **every**
registered action, evaluates `enabled()`, and invokes `run()` on each one with
a stub file picker and dialog (each returns "cancel"). It asserts: no panic,
the document and revision are unchanged except for actions that are allowed to
change it (New Composition, Import, New Folder), and `comps` is still empty
after every action that is not a create. Adding an action without a `requires`
declaration fails to compile (field is mandatory).

## 4. Empty state

### 4.1 When it shows

`active_comp().is_none()` (no tabs). It replaces the timeline panel content
and the viewport content, appears the frame the last tab closes, and disappears
the frame a composition opens. No timers, no animation state to get stuck.

### 4.2 Tile 1: New Composition

Click, Enter or Space opens the Composition Settings dialog (new mode) seeded
from `ProjectSettings.new_comp`. OK runs one `CreateComposition` command, opens
the tab, and selects the item in the Project panel. Cancel changes nothing.

### 4.3 Tile 2: New Composition From Footage

**Input sources, in priority order:**
1. Footage selected in the Project panel (assets only; folders expand to their
   assets via `expand_items`; compositions are ignored by this tile): use it
   with no file picker. The tooltip says: "Uses the footage selected in the
   Project panel. With nothing selected, opens a file picker."
2. Otherwise the file picker (multi-select). Files can also be dropped on the
   tile. Cancel does nothing: no command, no import, no status error.

**Rules ("composition from item", design-media-comps §5):**
- **One item:** size, frame rate and duration from the item. Stills use the
  project defaults for rate and duration. Audio-only is rejected with
  "Audio-only footage can't make a composition yet."
- **Several items with the same size, rate and duration:** same as one item.
- **Several items that differ:** a small dialog.
  - "Use settings from:" list, defaulting to the first item (the first in the
    Project panel's display order, or the picker's order for new files).
  - "Create a single composition" / "Create one composition per item".
  - **Default: "Create a single composition" (decided, Q1).** With
    "one per item", each composition takes its own item's settings and the
    "Use settings from" list is disabled.
- **Placement:** layers use the existing `resolve_drop` stack rule, so the
  result is the same as dragging onto an empty timeline. A single composition
  stacks all items at time 0, first item on top.
- **One undo step.** Files not yet in the project are probed in the UI (I/O,
  outside core), then one command (`DropItems` with `DropTarget::NewComp`,
  plus a `per_item` flag) imports the assets, creates the composition(s) and
  places the layers in a single `apply`. The UI then opens the tab(s) and
  selects the new items in the Project panel (view state).
- **Failure.** A file that cannot be probed is skipped and listed in one error
  line; if every file fails, nothing happens. No half-created state: the command
  validates before it mutates.

### 4.4 Layout and breakpoints

Widths are measured on the panel's own available rect, in egui points.

| Available width | Layout |
|---|---|
| ≥ 560 | Two tiles side by side, 16 gap. |
| < 560 | Tiles stacked vertically, 12 gap, each full width up to 320. |

- Tile size: width = clamp(available / 2 − gap, 220, 320); height = 0.7 ×
  width. Stacked tiles use width = clamp(available − 32, 160, 320).
- Never overlap: if even the stacked layout does not fit the height (< 2 tile
  heights + gap), tiles shrink to a minimum of 120 × 84, then the tip hides
  (available height < 340), then the tiles become two compact buttons with a
  label and no icon (available height < 200).
- **Viewport variant (decided, Q3):** only the centered text "No composition
  open" and a weak hint "Use the timeline area to create one". The tiles live
  in the timeline area only; the viewport has no buttons.
- Tile content: a line icon drawn with egui `Painter` strokes (outlined frame
  with a small shape trio for New Composition; the same frame plus a small
  film-strip outline for From Footage), a label below. No image files, so the
  artwork is ours and scales with the tile. Colors come from the theme.
- States: normal, hover (brighter stroke and fill), keyboard focus (visible
  2 pt ring), pressed. Tab order: New Composition, From Footage, "Next tip".
  Enter and Space activate. Each tile has an accessible label.

### 4.5 Tip of the Day

- **Storage:** `crates/voxy-ui/assets/tips.toml`, embedded with `include_str!`
  and parsed once. Needs the `toml` crate (new dependency; MIT/Apache-2.0,
  passes `deny.toml`). Alternative is RON; I propose TOML for translators.
  ```toml
  [[tip]]
  id = "reveal-source"
  text = "Select a layer and use {action:reveal.source_in_project} to find its source in the Project panel."
  ```
  `{action:<id>}` is replaced at runtime by the action's menu label and current
  shortcut, so a rebound key or a localized label is always correct.
  Localization: later files `tips.<locale>.toml` with the same `id`s; the
  English file is the fallback.
- **Content:** at least 30 original tips (shortcuts, Reveal, precomps, snapping,
  work area, drag modifiers, composition settings, folders, search and filters,
  and so on). No attribution to people. Every tip describes a feature that is
  built when the tip ships; tips for later milestones are added with those
  milestones (the validator enforces this).
- **Selection (decided, Q2):** random once per launch, never the same tip as
  the previous launch.
  - `Preferences.last_tip: Option<String>` stores the last shown tip `id`
    (saved with the preferences, never in a project file).
  - `pick_tip(tips, last, rng_seed) -> usize` is a pure function: it draws from
    all tips except `last` (unless there is only one). The seed comes from the
    system clock at launch, so tests pass fixed seeds and the result is
    reproducible in tests only.
  - "Next tip" moves to the next tip in file order and updates `last_tip`.
  - The seed is never used for anything saved or rendered, so determinism of
    projects and exports is unaffected.
- **Layout:** a lightbulb line icon (painter strokes), bold "Tip of the Day",
  one or two lines. Length limit **160 characters after substitution**, at most
  2 lines at the narrowest tile layout (checked by measuring in a headless
  egui context at width 320).
- **Preference** "Show tips", default on. There is no preferences store today,
  so this adds a minimal `Preferences` struct in `voxy-ui` saved as TOML in the
  platform config dir, separate from project files. Missing or corrupt file =
  defaults. The tip hides when off; the tiles are unaffected. The store lives
  in the operating system's per-user config directory (never in a project
  file) and a missing or corrupt file falls back to the defaults.
- **Validation test (§9):** parse the file; every `{action:id}` resolves in the
  action table; no raw `{` or `}` left; length limit; unique `id`s; at least 30;
  no tip text is empty.

## 5. Additional touches

- **Project panel, no items:** faint hint "Double-click or drag files here to
  import" and an Import button. No composition is created.
- **Footer:** "N items, M compositions" and a "Create New Composition" button
  (the same action as the toolbar button). The count excludes folders from
  "items" and says so in a tooltip.
- **Context menu in empty space:** New Composition, New Folder, Import,
  Select All.
- **Menus:** File > New > Composition and File > New > Composition From
  Footage work from any state (they are `Requires::Nothing`). Proposed
  shortcuts in the action table: New Composition `Ctrl/⌘+N`, New Project
  `Ctrl/⌘+Alt+N`, because `Ctrl/⌘+N` is New Project today (decided, Q5). New Composition From Footage has no default shortcut.

## 6. Commands added or changed

| Command | Change |
|---|---|
| `CreateComposition` | Unchanged. Returns the new ID in `CommandReport.created`. |
| `DropItems` (Milestone D) | Gains `DropTarget::NewComp { per_item: bool }` and `DropItem::File`. Import + create + place = one `apply`. |
| `PrecomposeLayers` | Find-or-create the "Pre-comps" root folder inside the command. |
| `RemoveItems` | Removing the last composition leaves `main_comp = None` (already so). Test added. |
| `Document::empty` | New. `Document::new` renamed `with_comp`. |

**Flags:**
- **Undo:** every create path is one command, so one undo. Undoing a
  create-from-footage also removes the imported assets it added (they were
  added in the same step). Opened tabs for a vanished composition close per
  design-media-comps §11.
- **Determinism:** unaffected. Composition thumbnails use the existing
  renderer for the panel only; export never sees them. Tip selection uses the
  date, never RNG, and affects nothing saved.
- **Color space:** unaffected.
- **OFX sandbox:** unaffected.

## 7. Persistence

- **Format stays 0.4.0.** `main_comp` is already nullable, so a project with
  zero compositions already round-trips. `docs/voxy-format.md` gets a note
  confirming that zero compositions and `main_comp: null` are valid, and that
  a new project is saved that way.
- **`ui_state` version 1 → 2** (it is versioned independently of the format):
  `media_panel` becomes `project_panel` and gains `columns`, `filter`, and
  `sort_by` (type). The loader reads v1 and v2; the writer writes v2. Unknown
  or invalid fields are dropped per field, as before.
- An empty project saves with `main_comp: null`, `comps: {}`, and
  `ui_state.tabs = []`, `active = null`, and reloads with no composition and
  no tabs.
- **Preferences** ("Show tips") are not in the project file.

## 8. Milestones (each ends with working software, `cargo clippy`, tests)

Dependencies: the panel, the action table and Composition Settings need to
exist first, and today they do not (§0). **Ordering (decided, Q4):** P-A is
folded into Milestone B, so the panel is born as the Project panel. P-B to P-D
run after the action table (B) and the Composition Settings dialog (C) exist.
Each P milestone still ends with clippy and tests passing.

- **P-A:** Project panel rename plus alias layer; compositions in the tree with
  columns, filters, sort, info header, auto-select/scroll, Pre-comps folder.
- **P-B:** `Document::empty`, remove every default composition, `Option`
  handling in all components (§3.2), disabled-with-tooltip actions, the
  no-panic test.
- **P-C:** Empty-state screen, tiles, From Footage flow, menu entries, panel
  empty hints, footer, context menu.
- **P-D:** Tip of the Day (data file, 30+ tips, preference, validator),
  polish, docs.

## 9. Tests

- **Core:** `Document::empty` has zero comps and `main_comp == None`; deleting
  the last composition leaves zero; importing into an empty project creates no
  composition; 0.3.0 migration still gives one composition; empty project
  round-trips; `main_comp: null` file opens with none; create-from-footage with
  one item, with mixed items (single and per-item); cancel (no command run);
  one undo reverts import + create + place; Pre-comps folder created once and
  reused, undone together with the precompose.
- **UI logic (command layer):** no-panic test (§3.3); panel sort/filter/column
  logic including Type grouping; alias resolution for old action IDs, panel
  names and the `media_panel` ui_state key; tile-layout function
  `empty_state_layout(width, height) -> Layout` tested at widths 200, 359, 559,
  560, 800 and 1600 (no overlap, correct stacking, tip hidden when short).
  Layout is a pure function of the size, so no egui snapshot is needed; a
  headless egui context only measures tip text.
- **Tips:** §4.5 validator.
- **Persistence:** empty project, migrated `ui_state` v1 → v2.

## 10. Decisions

| # | Question | Decision |
|---|---|---|
| Q1 | Default for several different footage items | **Single composition**, items stacked, first item's settings; "one per item" is offered in the dialog. |
| Q2 | Tip selection | **Random per launch, never the same as last launch** (§4.5). |
| Q3 | Tiles in the viewport | **No.** Message only (§4.4). |
| Q4 | Ordering | **Fold P-A into Milestone B**; P-B to P-D follow the action table and settings dialog (§8). |
| Q5 | Shortcuts | **New Composition `Ctrl/⌘+N`, New Project `Ctrl/⌘+Alt+N`**, as in After Effects. Both are in the action table, so the shortcut editor can rebind them; a test checks that no default shortcut collides (§5). |

Also approved with notes: the "Pre-comps" folder is found by exact name (§2);
`toml` as a new dependency and a small `Preferences` store, kept apart from
the project file in the per-user config directory and tolerant of a missing
or corrupt file (§4.5); `Document::new` renamed `Document::with_comp`, with
`Document::empty` for the app, which never calls `with_comp` (§3.1).

## Implementation notes (Milestone B)

What landed, and where it refines the design above.

- **Core (`voxy-core`).**
  - `Document::empty(name)` and `Document::with_comp(name, settings)`
    (the old `Document::new`). Only tests and fixtures call `with_comp`.
  - `ImportAssets { assets, folder }` adds footage with no layer and no
    composition; `CommandReport` gained `created_assets`.
  - `PrecomposeLayers` files the new composition in the top-level
    "Pre-comps" folder, creating it in the same command (one undo step).
    Pre-composed compositions no longer inherit the parent's folder.
- **Action table** (`voxy-ui/src/actions.rs`). Every menu item and shortcut is
  an `Action` with a stable id, label, default shortcuts, a scope (global,
  editor, Project panel), what it `requires` (nothing, a composition, a layer,
  a Project panel selection) and aliases for older ids. `Keymap` holds the
  bindings in effect (defaults plus overrides), reports collisions, and
  dispatches the keys pressed in a frame, most specific first, so
  `Ctrl+Alt+N` is never taken by `Ctrl+N`. Unavailable actions are disabled in
  menus with a tooltip ("Create or open a composition first."); pressed by
  shortcut they say the same in the status bar and do nothing.
  Saved bindings and the shortcut editor come with Preferences (P-D); the
  table already supports rebinding and loading old ids.
- **Project panel** (`project_view.rs` for logic, `project_panel.rs` for
  drawing). One tree of folders, compositions and footage with search, type
  filter (All, Compositions, Footage, Folders, Missing), sort (Name, Type,
  Size, Frame Rate, Duration, Used, Date Added; descending toggle), toggleable
  columns (left out from the right when the panel is narrow), multi-select
  (click, Ctrl/⌘, Shift), inline rename, duplicate (compositions), remove with
  a confirmation when layers use the items or a folder has contents, drag
  into folders, import (button, menu, drag from the operating system), a
  footer with counts and "Create New Composition", and context menus
  (items, and empty space: New Composition, New Folder, Import…, Select All).
  Missing files are checked at most every two seconds and on every document
  change; the badge, the Missing filter and Relink… use that result.
- **New items.** New Composition (button, footer, `Ctrl/⌘+N`, menu) creates a
  composition from the project defaults named "Comp 1", "Comp 2"…, opens it,
  selects it in the panel, scrolls to it and starts inline rename. It goes in
  the selected folder. The Composition Settings dialog (Milestone C) will
  open first.
- **Reveal** (`reveal.rs`). `reveal.source_in_project`, `reveal.in_composition`
  and `reveal.in_file_manager`, with the target rules of
  `design-media-comps.md` §6 as pure functions and the per-OS launch plans
  tested on every host. They change view state only, never `History`.
  Until composition tabs land the "open composition" is view state in
  `EditorState` (falls back to `main_comp`), and a Reveal selects the first
  matching layer (the editor has one layer selection until Milestone E).
  The playhead moves to the layer's in-point when it is outside the layer, and
  the timeline scrolls to the layer.
- **Rename.** The panel, tab, Window menu, action ids (`project_panel.*`),
  tooltips and status strings say "Project". `compat.rs` maps the old panel
  names ("Media", `media_panel`…) and `ui_state` v1 `media_panel` to the new
  names; action aliases cover `media_panel.*`, `media.*` and
  `reveal.source_in_media`.
- **`ui_state` version 2** (the format version stays 0.4.0): the panel is
  saved as `project_panel` with `sort`, `descending`, `filter`, `search`,
  `columns` and `expanded`. Loading reads version 1 (`media_panel`) and
  version 2, drops invalid fields one by one, and never fails.
- **A new project is empty already in this milestone.** `EditorState::untitled()`
  uses `Document::empty`, with a test that fails if it creates a composition,
  so the New Composition button has something to do. The full audit of "no
  active composition" (every menu and shortcut exercised with none, export,
  autosave) is still P-B.
- **Deferred.**
  - Thumbnails in the Project panel's info header: it shows a large icon and
    the settings summary until the preview renderer can render a composition
    other than the open one. (Tracked in [`deferred.md`](deferred.md).)
  - Saving the dock layout and shortcut overrides (Preferences, P-D).
  - Dragging items onto the timeline or viewport (Milestone D).

## Implementation notes (P-B audit and P-C)

- **P-B audit** (`crates/voxy-ui/src/audit.rs`, tests only). Every registered
  action is run against an empty project, an empty composition and an animated
  project with every kind of easing and all keys selected, with the file
  pickers answering "cancel". Asserted: no panic; an empty project is not
  changed by any action except New Folder; no action creates a composition;
  an action that cannot run says why in the status bar; the document still
  validates afterwards. Every panel is also drawn headlessly for several
  frames in each situation (and with media in the pool but no composition).
  The audit found no panics; it exists to keep it that way as actions and
  panels are added.
- **Empty-state screen** (`empty_state.rs`). Shows in the timeline area while
  no composition is open. `empty_state_layout(width, height)` is the pure
  layout of §4.4 (side by side from 560 points, stacked below, tiles shrink to
  120 × 84, the tip hides when it cannot fit with the smallest tiles, which is 200 points side by side and 276 stacked (the design said under 340, but the default layout's timeline pane is only about 220 points tall), compact buttons under 200) and is
  tested for overlap and fit over a grid of sizes. The viewport shows only
  "No composition open" and the hint (Q3); the graph editor a weak notice.
  Tiles are painter-drawn line art, focusable, activated by click, Enter or
  Space, and files can be dropped on the second tile.
- **New Composition From Footage** (`footage.rs`, action
  `file.new_composition_from_footage`, no default shortcut; File > New and the
  Project panel's context menu). Selected footage (folders expanded,
  compositions ignored) is used without a picker; otherwise a multi-select
  picker (cancel does nothing, not even a status line). One item, or several
  that agree on size, rate and duration, make one composition with that item's
  settings; stills keep the project's rate and duration. Items that differ ask
  "Use settings from" and single versus one-per-item (default single). The
  composition takes the first item's name, numbered if taken. Unreadable
  files are skipped and named in the status line; nothing happens if none can
  be read.
  - **Deviation from §4.3, to fold into Milestone D.** The design has one
    `DropItems { target: NewComp }` command; that command does not exist yet,
    so the UI builds the same result as one history transaction (import,
    create, place) using a new core command, `AddAssetLayer` (a layer for an
    asset already in the project; no format change). It is still one undo
    step, covered by a test. When `DropItems` lands, this flow becomes a call
    to it.
  - Items are placed in the project's own asset order (the Project panel's
    display order is not available to the command layer yet).
- **Not in P-C:** the Tip of the Day (P-D). The layout already reserves its
  space and hides it when short.

## Implementation notes (P-D, part 1: Tip of the Day)

- `crates/voxy-ui/assets/tips.toml` holds 36 original tips, embedded with
  `include_str!` and parsed once (`tips.rs`). The `toml` crate was already a
  dependency (preferences). `{action:<id>}` becomes the action's menu label
  plus its current shortcut; `{key:<id>}` (added) is the shortcut alone, for
  actions whose menu label reads badly in a sentence ("Composition", "Folder").
- **Selection.** `pick_tip(tips, last, seed)` is pure: it draws from every tip
  except the last one shown. The seed is the clock at launch and is used for
  nothing else, so projects and renders are unaffected. "Next tip" moves to
  the next tip in file order. Both write `last_tip` to the preferences.
- **Preferences.** `show_tips` (default on) and `last_tip`, in
  `preferences.toml`, never in a project. Window > Show Tips toggles the
  first. Older preference files load with the defaults.
- **Validator** (tests in `tips.rs`): at least 30 tips; unique, non-empty ids
  and text; every placeholder resolves to a real action; no raw braces left;
  every `{key:…}` action has a default shortcut; at most 160 characters after
  substitution; at most two lines when laid out at 320 points in a headless
  egui context. A tip must describe something that exists in this build.
- **Placement.** Under the tiles on the empty-state screen, with a lightbulb,
  "Tip of the Day" and a Next tip button; it hides when it cannot fit with the
  smallest tiles. Tab order is New Composition, From Footage, Next tip.
- **Still to do in P-D:** saving the dock layout, saved shortcut overrides and
  the shortcut editor (see [`deferred.md`](deferred.md)).

## Implementation notes (P-D, part 2: shortcuts and dock layout)

All of this lives in `preferences.toml` (per user), never in a project file,
and none of it changes the project format.

- **Saved shortcuts.** `[shortcuts]` maps an action id to a list of shortcuts
  as text: `"file.save" = ["Ctrl+Alt+S"]`, an empty list unbinds. `Ctrl` is
  the platform's command key (Cmd on macOS); `Cmd`, `Command` and `Control`
  are accepted when reading. Only what differs from the defaults is written.
  Ids resolve through the aliases (so `media_panel.*` still loads). Unknown
  ids, unreadable shortcuts and wrongly typed entries are dropped one at a
  time and never cost the rest of the file.
- **Keyboard Shortcuts dialog** (Edit > Keyboard Shortcuts…, an action with no
  default shortcut). Every action in the table, filterable. "+ Add" captures
  the next key press (Esc cancels the capture only; Esc itself can't be bound
  this way), × removes one shortcut, Reset and Reset All restore defaults.
  Changes save immediately. **Decision taken (conventional default, say if you
  want it otherwise):** assigning a shortcut that another action already uses
  *in a scope that can be active at the same time* moves it to the new action
  and the dialog says so, so there is never an ambiguous key. Delete is
  legitimately shared between Delete Layer (editor scope) and Remove from
  Project (Project panel scope) and stays that way. While the dialog is open
  the normal shortcut dispatch is off.
- **Dock layout.** `dock_layout` holds the arrangement as one JSON string
  (egui_dock's own serialization, with the per-frame rectangles stripped so
  resizing the window does not rewrite the file). It is written when a drag
  ends and the arrangement changed, and read at launch. A saved layout is used
  only if it is sound: one surface, a well-formed tree, every panel exactly
  once; otherwise the default layout is used and a warning is logged. Old
  panel names ("Media") load as "Project". Window > Reset Layout restores the
  default and saves it.
- **New dependency feature.** egui_dock's `serde` feature, which adds one
  small crate to the lockfile: `enumn` (MIT OR Apache-2.0, a derive macro by
  the author of serde), allowed by `deny.toml`.
