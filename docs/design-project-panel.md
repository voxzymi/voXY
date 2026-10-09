# Addendum: the Project panel, compositions in the pool, and the empty state

**Status: DRAFT, awaiting approval. No code has been written.**

This extends `docs/design-media-comps.md` and the rules in the follow-up brief
(rename to "Project", compositions in the tree, no default composition, empty
state with two tiles, Tip of the Day). It follows the working style set there:
original icons, artwork, wording and tips; "voXY" spelled that way in
user-facing text; all document changes through commands.

## 0. Read this first: the baseline is smaller than the brief assumes

`docs/design-media-comps.md` is **not in the repository** (not on this branch,
not on `main`, not in any commit), and the work it describes has not been
built. Checked against the code at `8226e9c`:

| The brief assumes | The repository has |
|---|---|
| An approved `design-media-comps.md` | No such file. `docs/` holds `design-proposal.md`, `voxy-format.md`, `am-format.md`, `third-party-licenses.md`. |
| A Media panel with folders, rename, remove, duplicate, drag, search, sort, multi-select | `Panels::media_ui` (`crates/voxy-ui/src/panels.rs`): two import buttons and a flat, read-only list of assets. |
| Compositions as pool items, usage counts, "Date Added" | Assets and compositions are unrelated tables. No folders, no usage query, no timestamps. |
| Composition tabs, Composition Settings dialog, "composition from item" rules | One `Document::root_comp`; the timeline and viewport always show it. No tabs, no dialog. |
| Precomp layers, Pre-compose, cycle checks | `LayerKind` is `Solid`, `Image`, `Video`. Precomps are planned for Milestones 4–5 (README). |
| Commands to create, rename, duplicate or delete compositions | None. `command.rs` has layer, keyframe, asset-path and working-space commands only. |
| An action table, a shortcut editor, "Reveal Layer Source in Project" | Menu items and shortcuts are inline code in `app.rs`; `KeyboardShortcut` constants. No table, no editor, no Reveal actions. |
| `ui_state`, saved panel layouts, saved shortcuts | Nothing is persisted but the document. The dock layout is rebuilt by `default_layout()` on every launch. |
| A "main composition" field (Q9) | `root_comp` exists but is **required**. |
| A Settings dialog and preferences | None. |

Two naming traps:

- `crates/voxy-ui/src/media.rs` defines `MediaPool`, the **video decoder
  pool**, not the panel. It, and its "media pool" doc comments, stay as they
  are. The rename is only the panel and the user-facing wording.
- The panel tab today is `Tab::Media`, titled "Media".

**Consequence.** Sections 1–6 of the brief are buildable, but several of them
need foundations that do not exist. I propose a milestone **P-0** (section 9)
that builds the minimum of those foundations, and I need your decision on
whether it is in scope (question Q-A in section 10). Everything below is
written for the repository as it is.

## 1. Representing "no active composition"

Two separate ideas, kept separate in the code:

- **Main composition** (document state, Q9): `Document::root_comp` becomes
  `Option<CompId>`. It is what export uses by default. It may be `None`.
  Setting, clearing and re-pointing it is a command (`SetMainComp`).
- **Active composition** (view state): `EditorState::active_comp:
  Option<CompId>`, the composition shown by the focused timeline tab and the
  viewport. It is derived from the tab strip (`open_tabs: Vec<CompId>`,
  `active_tab: Option<usize>`); `None` exactly when no tab is open.

`voxy-core` rules:

- `Document::new` creates **zero** compositions. `CompSettings::default()`
  stays as the pre-filled values of the dialog (1920 x 1080, 30 fps, 10 s);
  it is no longer used to build a document.
- `Document::root()` (which panics) is deleted. 90 call sites in 18 files use
  `root()` or `root_comp`; the compiler finds all of them. The replacements are
  `comp(id) -> Option<&Composition>` and `main_comp() -> Option<&Composition>`.
- `validate()`: `root_comp: None` is valid; `Some(id)` must exist.
- Removing a composition that is the main composition clears `root_comp` in
  the same command, so one undo restores both.
- Evaluation, render planning and the render graph already take a `CompId`
  or a `&Composition`; they never see "none". The `None` is handled one level
  up, by whoever chooses which composition to draw.

Components that must handle `None` (each gets a test, section 7):

| Component | Behavior with no active composition |
|---|---|
| Timeline panel | Empty state (section 3). No ruler, no layer list. |
| Viewport | "No composition open" and compact buttons (section 3). Nothing is rendered; the last frame is released. |
| Properties / Graph Editor | "Nothing selected" style placeholder; no layer lookups. |
| Playback / transport | Play, step and scrub are disabled; the clock stays paused at 0. Opening a composition resets the time. |
| Export | Disabled with a status message: "Create or open a composition first." If a main composition is set but no tab is open, export still works from the main composition. |
| Autosave / save | Saves normally. An empty project round-trips (section 6). |
| Menus and shortcuts | Items that need a composition are disabled with the tooltip "Create or open a composition first." Shortcut handlers check the same predicate, so a shortcut can never run a disabled action. |
| Render graph / `voxy-render` | Unchanged; never called without a composition. |

To make "every menu and shortcut action without panicking" testable, the
menus and shortcuts move to one **action table** (`crates/voxy-ui/src/actions.rs`):
`enum Action`, with `id() -> &'static str`, `label()`, `shortcut()`,
`enabled(&EditorState) -> Result<(), DisabledReason>` and `perform(&mut
EditorState)`. `app.rs` builds menus and shortcut handling from it. The
no-panic test iterates `Action::ALL` on an empty project.

## 2. The Project panel and the rename

- User-facing: panel title and tab "Project"; items are "project items".
  Window menu entry "Project" (the Window menu is new, see P-0). Strings:
  "No media yet." becomes "Double-click or drag files here to import";
  "Reveal Layer Source in Project"; "Not used in any composition".
- Code: `Tab::Media` becomes `Tab::Project`; `media_ui` becomes
  `project_ui` in a new module `project_panel.rs` (the panel is already too
  big for `panels.rs`); action IDs use the `project_panel.*` prefix.
- **Migration plan.** Because nothing is persisted yet, the rename itself
  breaks nothing today. The migration layer is built *with* the persistence it
  protects and tested against synthetic old data:
  - dock layout: `Tab` derives `Deserialize` with `#[serde(alias = "Media")]`
    on `Project`; a layout containing "Media" loads as the Project panel and is
    written back as "Project";
  - shortcuts: an `ACTION_ALIASES` table (`"media_panel.*" -> "project_panel.*"`)
    applied when loading a shortcut map; unknown IDs are dropped with a log
    line, never an error;
  - `ui_state` (section 6): the old key `media_panel` is read as
    `project_panel`; if both exist, `project_panel` wins.

### Rows, columns, sort, filter

Row for a composition: icon, name, "1920 x 1080, 30 fps", duration, usage
count. Columns (toggleable, stored in `ui_state`): Name, Type, Size, Frame
Rate, Duration, Used, Date Added. Filter by type: Compositions, Footage,
Folders, Missing. Sort keys: any column, plus "by type" which groups
compositions together and keeps folders first. Sorting and filtering never
reorder the document, only the view.

The info header (one item selected) shows a thumbnail at the composition's
current frame if it is open, otherwise its first frame, and the settings
summary. Thumbnails render through the existing render path at low resolution,
cached by composition revision; they never touch document state.

Newly created compositions are selected and scrolled into view; the "New
Composition" button also opens inline rename. Pre-compose puts results in a
folder named "Pre-comps", created on first use by the same command that
creates the precomp.

## 3. The empty state

Shown in the timeline panel and the viewport when `active_comp` is `None`, and
removed the same frame a composition opens.

**Tiles.** Two tiles, drawn with `egui::Painter` strokes (no image files, so
nothing is traced and everything scales): an outlined frame with a circle and a
triangle for New Composition, and the same frame plus a small film-strip
outline for New Composition From Footage. Each tile has a label, hover and
keyboard-focus states (focus ring, not color alone), a tooltip, and AccessKit
labels. Tab moves between tiles; Enter and Space activate.

**Layout rules** (a pure function `tile_layout(available: Vec2, show_tip:
bool) -> TileLayout`, unit-tested):

| Available width | Layout |
|---|---|
| >= 520 pt | Side by side. Each tile is at most 280 x 220 pt, at least 200 x 160 pt, 24 pt gap, centered. |
| 280 – 519 pt | Stacked vertically, each tile full width up to 280 pt, 16 pt gap. |
| < 280 pt | Compact rows: icon beside label, no tile chrome. |

Height: the tip is hidden below 360 pt of available height; the tiles shrink
toward their minimum, then switch to compact rows. Tiles never overlap at any
size; a test sweeps widths 100 to 1600 pt in 10 pt steps and asserts disjoint
rectangles inside the available rectangle.

**Viewport.** Smaller: "No composition open" in weak text, and compact
buttons (not full tiles) if at least 360 x 200 pt is free. See Q-D.

**New Composition.** Opens Composition Settings in new mode with project
defaults. OK runs one command (create) that also opens the tab and selects the
item in the Project panel. Cancel changes nothing.

**New Composition From Footage.**

1. Source of footage: if the Project panel selection contains footage items,
   use them and do not open a picker (tooltip: "Uses the footage selected in
   the Project panel. If none is selected, opens a file picker."). Selected
   compositions and folders are ignored here. Otherwise open the file picker
   (multi-select). Files dropped on the tile are used the same way. Files
   dropped elsewhere on the empty state do nothing; dropped on the Project
   panel they only import.
2. Cancel the picker: nothing changes, no history entry.
3. Settings from footage: one video: its size, frame rate and duration.
   One still: its size, the project frame rate, and the **default still
   duration** (5 s; a project preference, so not hard-coded in the command).
4. Several items: if all share size and frame rate, no prompt. Otherwise show
   the prompt: "Create a single composition" or "Create one composition per
   item", plus a settings choice (first item, or pick from a list). **Default:
   single composition using the first item's settings** (Q-B).
5. Unreadable files are skipped with a status message naming them. If none
   are usable, nothing is created.
6. The whole thing is one `History::begin("Create Composition From Footage")
   ... commit()` transaction: add assets (reusing existing ones for the same
   path), create the composition or compositions, place layers, open the tab,
   select. The existing transaction nests, so one Undo reverts everything; the
   tab and selection are view state and are restored by the undo handler
   closing tabs whose composition no longer exists.

**Tip of the Day** (below the tiles, hidden when short on height): an
original lightbulb outline icon drawn by us, bold "Tip of the Day", one or two
lines of text, optional "Next tip" link. Preference "Show tips", default on.

## 4. Tips

- Data: `crates/voxy-ui/data/tips.en.toml`, embedded with `include_str!`.
  One file per language so translators edit one file:

  ```toml
  [[tip]]
  id = "reveal-source"
  text = "Right-click a layer and choose Reveal Layer Source in Project to find its footage."
  actions = ["layer.reveal_source_in_project"]
  ```

  `id` is stable; `text` is the only translated field; `actions` lists the
  action IDs the tip mentions.
- At least 30 tips, all original, none attributed to a person, none naming
  other software.
- Tests: the file parses; at least 30 tips; IDs are unique; every `actions`
  entry exists in `Action::ALL`; every action ID quoted in a tip's text is
  listed in `actions`; text length <= 140 characters (two lines at the
  default tile width); no empty text; no text mentions another product by name.
  Each tip's text must also appear on at most one id (no duplicates).
- Selection: see Q-C. The code isolates it behind `pick_tip(history,
  today) -> usize`, so changing the rule later is one function.
- Persistence: "Show tips" and the last tip shown live in app preferences,
  not in the project (P-0 adds a small preferences file).

## 5. Menus and panel extras

- File > New > Composition and File > New > Composition From Footage, with
  shortcuts defined in the action table (proposed: Ctrl/⌘+Alt+N for
  Composition; From Footage unbound). Both are always enabled.
- Project panel empty state: faint hint "Double-click or drag files here to
  import" and an Import button. Footer: "N items, M compositions" and a
  Create New Composition button. Context menu in empty space: New Composition,
  New Folder, Import, Select All.

## 6. Persistence and format version

Schema changes, so the version bumps from 0.3.0 to **0.4.0**:

| Change | Why |
|---|---|
| `project.root_comp` becomes optional (omitted when none) | Zero compositions. A 0.3.0 reader would reject such a file as invalid, so the bump is required. |
| `folders` array; optional `folder` on assets and compositions | Project tree membership (P-0). |
| Optional `added_at` on assets and compositions | "Date Added" column. |
| Optional `ui_state` object: open tabs, active tab, panel columns, sort, filter, expanded folders | Per-project view state. |

Migration 0.3.0 → 0.4.0: none needed; every 0.3.0 file is valid 0.4.0 content
(all new fields are optional). The result is a document with one composition
and `root_comp` set, exactly as before. Nothing is created when a file has
none; a file that has none cannot come from 0.3.0 or earlier, which require
the root, so "legacy file with no composition" is not a reachable state; the
empty project test covers the 0.4.0 case instead. `docs/voxy-format.md` gets
the new version row and field tables.

`ui_state` is a view hint: it is never read by evaluation or rendering, never
affects the content hash, and a missing or invalid `ui_state` is ignored (with
a log line) rather than failing the load. Unknown tab IDs are dropped.
`added_at` is wall-clock time and is likewise display metadata only.

## 7. Tests

As the brief lists, mapped to files:

- `voxy-core`: new project has zero comps and validates; removing the last
  composition creates none and clears `root_comp` in one undoable step;
  importing an asset into an empty document creates no composition;
  0.3.0 fixture (kept in `tests/`) loads as one composition; empty 0.4.0
  round-trip; undo/redo property test extended with the new commands.
- `voxy-ui` logic, through commands and the action table: From Footage with
  one video, one still, mixed items (single and per-item), unreadable file
  skipped, cancel is a no-op, one undo reverts the whole operation; the
  no-panic test over `Action::ALL` plus every shortcut on an empty project;
  panel and shortcut alias migration; `ui_state` migration; tile layout sweep;
  tip tests.
- Snapshot test of the empty state at several widths **only if** `egui_kittest`
  passes `cargo deny` and builds on CI; otherwise the pure `tile_layout`
  tests are the substitute and I will say so.

## 8. Things I am flagging, as the brief asks

- **Determinism:** rendering is unaffected. The new stored fields
  (`added_at`, `ui_state`, `folders`) are excluded from the content hash and
  from evaluation. Saved files are no longer byte-identical for identical
  edits because of `added_at`; golden *image* tests are unaffected. If you
  need byte-reproducible saves, I will drop `added_at` and sort by insertion
  order instead (Q-E).
- **Undo:** composition, folder, asset and main-composition changes are
  commands. **Tab open/close, active tab, Project panel selection, sort, filter
  and columns are view state and are not undoable**, which is a deliberate
  exception to "every state change goes through commands". Undo restores
  document state and then closes tabs whose composition vanished.
- **Color space:** untouched. The working space stays in `meta`. Composition
  Settings does not add a per-composition space.
- **OFX sandbox:** untouched; no OFX code exists on this path.
- **Panics today:** `Document::root()` panics by design. Removing it is the
  largest single change in P-B.

## 9. Milestones

Each ends with working software, `cargo clippy --workspace --all-targets`, and
`cargo test --workspace` passing, plus a short summary.

- **P-0 (new, needs approval): foundations the brief assumes.** Folders and
  item membership; composition commands (create, rename, duplicate, delete,
  set main, move to folder); `AddAsset` as a standalone command; usage-count
  query; action table with the existing menu items moved onto it; Window
  menu; app preferences file; Composition Settings dialog (new mode only);
  composition tabs and `EditorState::open_tabs`. Format 0.4.0 lands here.
- **P-A:** Project panel rename, alias and migration layer, compositions
  listed with columns, filter and sort.
- **P-B:** zero default compositions; `Option` main composition; every
  `root()` call site handled; no-panic test.
- **P-C:** empty state, tiles, From Footage flow, menu entries, panel hints.
- **P-D:** Tip of the Day (30+ tips, preference, tests), polish, docs.

Out of scope until their own milestones, and noted so the brief's
"Pre-compose", "Reveal" and drag-to-viewport items are not forgotten: precomp
layers and Pre-compose (Milestones 4–5), Reveal actions and the shortcut
editor. Where a tip would mention one of these, it is left out until the
action exists; that is what the tip test enforces.

## 10. Questions that block coding

- **Q-A.** The baseline from section 0 is missing. Build P-0 as part of this
  work (recommended), or should I wait for `design-media-comps.md` and the
  earlier work to land from somewhere else?
- **Q-B.** Several footage items with different settings: default to a single
  composition (recommended, first item's settings) or one per item?
- **Q-C.** Tip selection: deterministic by date (testable, no state) or random
  but never the same twice in a row (needs the last tip stored)?
- **Q-D.** Viewport empty state: compact buttons only (recommended, since the
  timeline already shows the full tiles), or the full tiles in both?
- **Q-E.** Keep `added_at` (wall-clock "Date Added", non-byte-reproducible
  saves), or order by insertion and skip the timestamp?
