# Design note: media pool, compositions, precomps, reveal, drag to timeline

**Status: approved.** The owner's decisions are recorded in §13. Milestone A
(data model and commands in `voxy-core`) is implemented; see "Implementation
notes (Milestone A)" at the end. Deferred work is tracked in
[`deferred.md`](deferred.md).

This note extends the architecture in `design-proposal.md`. Nothing is
rewritten:
- Every document change is a `Command` run through `History`: snapshot undo,
  transactions, and merge keys for drags.
- Time stays in flicks, and keyframable values use `Property`.
- `voxy-core` stays free of GPU, FFmpeg and UI code.

Wording and icons are voXY's own. The workflow follows After Effects; no
Adobe assets or text are copied.

## 0. What exists today, and what this work depends on

Facts from the code at `5ae02e7` that shape this design:

| Area | Today | Consequence |
|---|---|---|
| Compositions | `Document.comps` is already a map, but the UI only shows `doc.root()`. | Most of the model is there. The UI moves from "the root" to "the active tab's composition". |
| Assets | `Asset { id, name, kind: Image \| Video, path, pixel_size, duration, frame_rate }`. The name is already separate from the path. | Extend this, don't replace it. |
| IDs | One `next_id` counter in the document. Undo restores an older snapshot, and with it an older `next_id`, **so an ID can be reused after undo**. | §1.1 fixes this ("never reused"). |
| Keyframes | Stored in **composition time**: `value_at(t)` takes comp time. | Precompose and unprecompose shift keyframe times by the layer offset (§4). |
| Layer timing | `in_point`, `out_point`, `start_offset`; source time is `t − in + start_offset`. | Precomp layers reuse this and add speed, loop and remap. |
| Selection | `EditorState.selection: Option<LayerId>`, one layer. | Becomes a per-tab `Vec<LayerId>`. |
| Viewport cache | `FrameKey { revision, time, size, asset_root, video }`: any edit re-renders. | Already safe against stale frames. Precomp caches add a content hash (§9). |
| Render plan | A flat list of `DrawOp`s for one composition. | Gains a nested-composition source (§9). |

These parts of the brief depend on systems that **do not exist yet**. §13 asks
how to handle each:
- **Export:** no export dialog or encoders. This blocks the export overrides
  and the "even dimensions for the chosen encoder" check.
- **Audio layers and mixing:** video audio is not played and there are no
  audio layers, so there is no precomp audio to mix.
- **Shortcut editor:** shortcuts are constants in `app.rs`.
- **Autosave and crash recovery:** not implemented.
- **Markers and the work area:** not modeled.
- **Viewport hit testing:** the viewport cannot pick a layer under the
  pointer. This is needed for viewport double-click, viewport reveal and
  "viewport drop at position".
- **Text layers and effects:** "double-click keeps text editing" and "keep
  effects on replace" have nothing to act on yet. The rules are written so
  they apply unchanged when those features land.

## 1. Data model (`voxy-core`)

### 1.1 IDs
- New ID type `FolderId`, from the same counter as the others.
- **IDs are never reused, even after undo.** `History` keeps a high-water
  mark of `next_id` and raises the working copy's counter to it before each
  `apply`. Undo still restores the snapshot exactly, so the round-trip
  property tests are unaffected. **Flag (undo):** a change to `History`, but
  undo and redo results do not change.

### 1.2 Media items (`Asset`, extended)
```rust
pub struct Asset {
    pub id: AssetId,
    pub name: String,                 // display name; editable
    pub kind: AssetKind,              // Image | Video | Audio | Svg  (Model later)
    pub path: String,                 // unchanged rules
    pub content_hash: Option<ContentHash>,  // NEW, for relink matching
    pub pixel_size: Option<[u32; 2]>,
    pub duration: Option<Time>,
    pub frame_rate: Option<FrameRate>,
    pub audio: Option<AudioInfo>,     // NEW: channels, sample_rate
    pub folder: Option<FolderId>,     // NEW: None = root
}
```
- **Content hash.** It is computed outside core (in voxy-media or the UI,
  because it does I/O) and stored as an opaque value: `{ algo: "fnv1a64-sampled",
  size, digest }`.
  - The digest covers the file size plus the first, middle and last 1 MiB,
    so hashing a multi-GB file is instant.
  - FNV-1a is written in a few lines, so no new dependency is needed.
  - The hash is used only to suggest relink candidates, never as an
    identity.
- **No "date added" field is needed.** IDs only increase, so sorting by date
  added means sorting by ID.
- `Audio` and `Svg` are added to `AssetKind` only when their layer types
  land. The serde tags are reserved now.
- **"Missing" (offline) is runtime state**, not document state: a file check
  the media panel caches per path. Saving and loading never change it.

### 1.3 Folders
```rust
pub struct Folder { pub id: FolderId, pub name: String, pub parent: Option<FolderId> }
// Document gains: pub folders: Arc<BTreeMap<FolderId, Folder>>
```
- Assets, compositions and folders each carry `folder` or `parent`.
- Order inside a folder is never stored. The panel sorts by the chosen key,
  so there is no stored order to keep consistent.
- Validation rejects missing folders and folder cycles.

### 1.4 Compositions (extended)
```rust
pub struct Composition {
    // existing: id, name, width, height, frame_rate, duration, background, layers, foreign
    pub pixel_aspect: Ratio,          // NEW, default 1/1 (num/den u32)
    pub work_area: Option<(Time, Time)>, // NEW, None = whole comp
    pub folder: Option<FolderId>,     // NEW
}
```
- **The work area belongs to the composition** (document, undoable, saved),
  as in AE. A tab only remembers the view: playhead, zoom, scroll and
  selection. This settles the brief listing the work area under both the
  composition (§1) and per-tab state (§7).
- **`root_comp` becomes `main_comp: Option<CompId>`:**
  - It is the composition opened when no UI state is saved.
  - It may be `None`: a project can have zero compositions, as in AE.
  - Removing the main composition promotes the composition with the lowest
    ID, or `None` if there is none left.
  - `Document::root()` callers in the UI switch to the active tab's
    composition.

### 1.5 Precomp layers
```rust
LayerKind::Precomp {
    comp: CompId,
    speed: f64,                 // 1.0 = normal; >0. Negative (reverse) not in v1
    looping: bool,
    time_remap: Option<Property>, // key "precomp.time_remap", scalar seconds of child time
    collapse: bool,             // collapse transformations (2D)
}
```
- `in_point`, `out_point` and `start_offset` keep their meaning.
  `start_offset` is the child time shown at `in_point`.
- `time_remap` is an ordinary keyframable `Property`. Its keyframes are in
  parent composition time, like every other property.
- `LayerKind::asset()` gains a sibling, `LayerKind::source() -> Option<SourceRef>`,
  where `SourceRef = Asset(AssetId) | Comp(CompId)`. Usage, reveal, replace
  and removal all go through it.

### 1.6 Placeholder layers
```rust
LayerKind::Placeholder { was: PlaceholderInfo }
// PlaceholderInfo { kind: "video"|"image"|"comp"|…, name, pixel_size, duration }
```
- This is what remains when a layer's source is removed with "replace layers
  with a placeholder".
- It renders as a neutral diagonal-stripe pattern at the original size, with
  the source name in the layer.
- `ReplaceLayerSource` turns it back into a real layer, keeping its timing,
  transform and keyframes.
- This is separate from **offline media**: an asset still in the pool whose
  file is missing. Offline media shows a missing badge and a Relink action,
  and reveal still selects it (§6.1).

### 1.7 Project settings
```rust
pub struct ProjectSettings {         // inside ProjectMeta
    pub new_comp: CompDefaults,      // width, height, pixel_aspect, frame_rate, duration, background
    pub default_still_duration: Option<Time>, // None = until the composition end (AE default)
}
```
- `working_space` stays where it is, unchanged.
- **Flag (color):** none of this work touches the color-space setting.
  Precomp offscreen buffers use the project's working space, the same as the
  main target.

### 1.8 Cycle detection
- `Document::comp_contains(outer, inner) -> bool` does a DFS over precomp
  layers, with a visited set.
- `Document::would_cycle(target_comp, inserted_comp)` returns true when the
  two are equal or `comp_contains(inserted, target)`.
- It is checked in every command that can create a precomp reference (add,
  drop, replace source, precompose) and again in `validate()`.
- Error message: `"“Intro” can't be placed in “Main”: “Intro” already
  contains “Main”"`. Indirect cycles also name the chain.

### 1.9 Usage tracking
- `Document::usages(src: SourceRef) -> Usage`, where
  `Usage { per_comp: Vec<(CompId, Vec<LayerId>)> }`:
  - compositions are sorted by name (case-insensitive), then by ID;
  - layers are in stack order;
  - plus `total()` and `comp_count()`.
- **Computed by scanning, not stored.** Stored back-references would be a
  second source of truth that every command and every undo snapshot would
  have to keep consistent.
  - A scan is O(layers) and needs no copying.
  - The UI memoizes the result per `History::revision()`.

## 2. Commands

Every command is undoable through the existing snapshot `History`:
- "Undo restores everything" (item, folder, layers) holds by construction.
- Commands that do several things are one `Command` whose `apply` does all of
  them, so each is one undo step. Transactions are not needed.

| Command | Notes |
|---|---|
| `RenameMedia`, `RenameComposition`, `RenameFolder` | Trims whitespace. Rejects empty names and control characters. Duplicate names: see §13 Q1. |
| `RemoveItems { items, mode }` | One command for media, compositions and folders, so a mixed selection is one undo step. `mode`: `PlaceholderLayers` or `DeleteLayers`. A folder removes its contents recursively. Never touches files on disk. A removed composition with open tabs: §11. |
| `CreateComposition { settings, folder }` | |
| `DuplicateComposition { comp }` | Deep copy with new IDs for the composition, layers and properties. Parent links are remapped inside the copy. Precomp references to other compositions stay shared, as in AE. Name: "<name> 2", "<name> 3", and so on. |
| `CreateFolder`, `MoveToFolder { items, folder }` | Moving a folder into its own subtree is rejected. |
| `PrecomposeLayers` | §4.1 |
| `UnprecomposeLayer` | §4.2 |
| `ReplaceMedia { asset, path, metadata, hash }` | Relink: every layer keeps pointing at the same `AssetId`. Returns warnings (size or frame-rate mismatch) for the UI. Supersedes `SetAssetPath`, which stays as a thin wrapper. |
| `ReplaceLayerSource { layers, source }` | Keeps transform, keyframes, timing, flags and (later) effects. Rejects incompatible kinds: an audio-only source can't replace a visual layer. Checks cycles. |
| `SetCompositionSettings { comp, settings, layer_mode, snap_keys }` | Merge key `("comp-settings", comp)`, so dialog edits with live preview merge into one step. §3. |
| `SetProjectSettings` | |
| `DropItems { target, items, placement }` | §5. One step per drop, never merged with another. |
| `SetPrecompTiming { layer, speed, looping, remap on/off }` | The remap keyframes themselves use the existing keyframe commands. |
| `SetCollapse { layer, on }` | |

Commands return extra information (warnings, "could not preserve" reports)
through an optional `CommandReport` that `History::execute` passes back. This
is an additive API change. **Flag (undo):** the history behaves the same.

## 3. Resolution and frame rate

### 3.1 Settings dialog
- Fields: name, width, height, pixel aspect ratio, frame rate (presets plus a
  custom rational), duration (timecode and frames), background, aspect
  presets (16:9, 9:16, 1:1, 4:5, 21:9, custom), and a lock-aspect toggle.
- The same dialog style edits the project defaults.
- Duration in frames converts with `FrameRate::frame_to_time`.

### 3.2 Changing resolution
Options, offered only when the size changes and the composition has layers:
1. **Keep layers** (proposed default, AE-like):
   - Canvas only. The content keeps its pixel size.
   - An anchor grid (3×3, default center) decides where the old canvas sits
     in the new one.
   - Positions are offset by the anchor delta: every `transform.position`
     value and keyframe of **unparented** layers moves by the same vector.
     Child layers follow their parents automatically.
2. **Scale layers proportionally:**
   - Uniform factor `s = min(new_w/old_w, new_h/old_h)`. Content is fitted
     and then centered if the aspect ratio changes.
   - Unparented layers get position ×s plus centering, and scale ×s.
   - Spatial tangents ×s, and solid sizes are unchanged (they scale through
     the layer scale).

Precomp layers that use this composition are not changed. Their content is
re-rendered at the new size, as in AE. See §13 Q2.

### 3.3 Changing frame rate
- Keyframes and layer timing **do not move**: they are flicks. Only the frame
  grid changes.
- The dialog counts keyframes and layer edges that fall off the new grid
  (`time != frame_to_time(time_to_frame(time))`). It shows "N keyframes and M
  layer edges will fall between frames" with the checkbox "Snap them to the
  nearest frame" (default off).
  - When on, the same command moves them.
  - Collisions (two keys snapping to the same frame) keep the later one, and
    the count is reported.
- **Footage at a different rate** uses the existing decode rule: show the
  frame on screen at the source time, which is "hold the previous frame".
  Blend and optical flow come later as per-layer options. §13 Q3.
- **Precomps at a different rate:** see §8.2.

### 3.4 Validation
- **Width and height:** 16 to 16384 in core, a fixed documented constant.
  - The UI also checks `device.limits().max_texture_dimension_2d`, and warns
    (without blocking) when the composition, or the composition × preview
    scale, exceeds it. The viewport then renders at a reduced scale and the
    warning says so.
  - Odd sizes are allowed in compositions, as in AE. The even-size rule
    belongs to the encoder, so it is checked by export (see §0).
- **Frame rate:** `FrameRate::new` rules. Custom rates are capped at 240 fps.
- **Duration:** at least 1 frame, at most 3 hours.
- **Pixel aspect:** 0.25 to 4.

### 3.5 Cache correctness
- Composition settings are part of every precomp content hash (§9.2), and the
  viewport key already includes `revision`.
- Test: render a frame, change the resolution (or frame rate), render again
  without clearing any cache. The result must equal a render on a fresh
  renderer, and must not equal the stale frame.

## 4. Precompose and unprecompose

### 4.1 `PrecomposeLayers { comp, layers, mode, name }`
- **`MoveAttributes`** (AE's "move all attributes"; required for more than
  one layer):
  - The new composition gets the parent's size, rate and duration.
  - The selected layers move into it unchanged: same times, same keyframes,
    since both compositions share a time origin.
  - The precomp layer is inserted at the topmost selected index, with
    `in = 0`, `out = parent duration`, `start_offset = 0`, `speed = 1` and an
    identity transform centered on the canvas.
  - **Parenting:**
    - Links between moved layers are kept.
    - A moved layer parented to a layer that stays behind loses the link.
      Its world transform at the current time is baked into its own static
      values, and this is reported.
    - Layers that stay behind and were parented to a moved layer are
      re-parented to the precomp layer and reported.
- **`LeaveAttributes`** (one layer only, not a precomp):
  - The layer's transform and timing stay on the new precomp layer in the
    parent.
  - The child holds a copy of the source with an identity transform,
    covering the source's own size and duration.
  - Keyframe times on the layer stay valid because the precomp layer keeps
    the same in-point and time origin. `start_offset` maps through.
- `open_new: bool` (default on) is a UI flag. The command returns the new
  composition's ID and the UI opens it.

### 4.2 `UnprecomposeLayer { layer }`
- The precomp layer is replaced in place by copies of the child's layers,
  with new IDs.
- **Baked exactly when** the mapping is linear: no remap, no loop, any
  speed. A speed other than 1 bakes exactly only when every child keyframe
  and edge lands on a flick after scaling, which is checked.
  - Child times map to parent times with `t_parent = in + (t_child − start_offset)/speed`.
  - Each copied layer's in/out are clipped to the precomp layer's in/out.
  - The precomp layer's transform becomes a new null-like parent: voXY has
    no null layer yet, so a transparent `Solid` of size 0, named
    "<precomp> transform". Children with no parent are parented to it, so
    the precomp's transform and keyframes are preserved exactly.
  - Parenting does not inherit opacity, in AE or in voXY. So a precomp
    opacity other than 100%, or an animated one, is reported as "not
    preserved".
- **Reported as approximate:**
  - Remap: baked by sampling at the parent frame rate.
  - Loop: the child is repeated only up to 16 times, with a warning.
  - A frame rate that differs from the parent.
  - Collapse on: blend order may differ.
  - The child's background color, which is dropped.
- The child composition stays in the pool, as in AE.
- One command, so one undo step.

## 5. Drop resolution (pool or OS → timeline/viewport)

`DropItems { target: DropTarget, items: Vec<DropItem>, placement }`, where:
- `DropItem = Asset(AssetId) | Comp(CompId) | File(ImportSpec)`. OS files are
  imported inside the same command, so import plus place is one undo step.
- `DropTarget = Timeline { comp, time, index } | ReplaceLayer { layer } | Viewport { comp, time, pos } | NewComp`.

Resolution is a pure function, `resolve_drop(doc, target, items, placement) -> Plan`.
The UI ghost preview and the command share it, so the preview always matches
the result, and the tests target it directly.

| Rule | Proposed default |
|---|---|
| Multiple items | **Stack**: all start at the drop time, first item on top. **Ctrl/⌘ held: sequence**, each starting at the previous one's out-point. §13 Q4 |
| Item order | The media panel's current display order of the selection, not click order, so the result is deterministic. |
| Layer index | Empty space below the last layer appends. Between rows inserts at that boundary. Above the first row inserts at the top. |
| Video length | The full source duration from the drop time. **Not trimmed at the composition end**: the layer extends past it and the timeline shows the overflow clipped. Trimming is lossy and AE does not trim either. Unknown duration means until the composition end. |
| Still length | `default_still_duration`, or until the composition end when that is `None` (default). |
| Composition length | Its own duration. |
| Dropped too close to the end | Less than one frame left: the layer still gets its full length (above), so it is never zero-length. |
| Alt/Option onto a layer | `ReplaceLayerSource`. Keeps in, out, start_offset, transform and keyframes. If the new source is shorter, video holds its last frame (existing rule) and a precomp shows nothing past its end. The layer is never retimed. A multi-item drop with Alt uses only the first item and says so. |
| Viewport drop | Layer anchor at its own center, position at the drop point in composition pixels, in-point at the current time, on top of the stack. |
| Cycles | `resolve_drop` returns `Blocked(reason)`. The cursor becomes "not allowed" and the reason shows in the status bar. The command re-checks and refuses. |
| New composition from drop | On the New Composition button, or on a timeline with no open composition: size, rate and duration come from the first item. If the items disagree, a small prompt lists the candidates (default: the first). Stills use the project defaults for rate and duration. |
| Snapping | To the playhead, layer in/out edges, the work-area edges, and markers when they exist. The threshold is 8 px. Holding **Shift** inverts snapping, as in AE. |
| Escape | Cancels the drag. Nothing is executed. |
| Auto-scroll | Within 24 px of the timeline edges, at a speed proportional to depth. |

## 6. Reveal

Reveal actions change **view state only**: selection, scroll, folder
expansion and tabs. They never enter `History`.
- They are registered as named actions (`reveal.source_in_project`,
  `reveal.in_composition`, `reveal.in_file_manager`) in a new action table.
- The menus and the future shortcut editor read the same table.
- Default bindings: none. AE has none for these either.

### 6.1 Layer → project ("Reveal Layer Source in Project")

| Layer | Target |
|---|---|
| Image, video, audio | Its asset, even when offline: the item shows the missing badge. |
| Precomp | Its composition. |
| Solid | Disabled: "Solids are not project items." |
| Placeholder | Disabled: "Source was removed from the project." |

- Multi-select reveals the distinct sources, in panel display order, and
  scrolls to the first.
- Ancestor folders are expanded.
- An active search or filter that hides a target is cleared, with the status
  message "Search cleared to show the item".

### 6.2 Project → composition ("Reveal in Composition")
- Built from `usages()`, unioned over the selected items.
- Submenu entries look like `Main (3 layers)`, sorted by composition name,
  then ID.
- One composition skips the submenu. None disables the entry with "Not used
  in any composition".
- **Multi-selection spanning several compositions:** the submenu lists every
  composition with the count of layers that use *any* selected item. Picking
  one selects all of those layers there.
  - With no submenu (the keyboard command), the target is the composition
    with the most matching layers, ties going to the lowest name and then
    ID.
  - The rule is deterministic and documented.
- Choosing a composition switches to its tab, reopening it if closed. The
  rest of §7.1 applies. It then selects the layers and scrolls the timeline
  to the first one in stack order, and moves the playhead to that layer's
  in-point if the in-point is out of view.

### 6.3 Reveal in file manager
- Lives in `voxy-ui/src/reveal.rs`.
- A pure function builds the launch plan, and a thin executor runs it:
  ```rust
  enum Os { Windows, MacOs, Linux }
  struct Launch { program: OsString, args: Vec<OsString>, raw_windows_arg: bool }
  fn reveal_plan(os: Os, path: &Path) -> Vec<Launch>   // tried in order until one succeeds
  ```
  - **Windows:** `explorer.exe` with one argument, `/select,"<path>"`.
    Explorer does not follow the standard argv quoting, so this one argument
    goes through `CommandExt::raw_arg`.
    - Windows paths cannot contain `"`, so wrapping the path in quotes is
      unambiguous.
    - `\\?\` verbatim prefixes are stripped (`\\?\UNC\x` becomes `\\x`).
    - UNC paths are passed unchanged.
    - No shell is involved.
  - **macOS:** `open -R <abs path>`. The path is absolute, so it can never
    start with `-` and be read as an option.
  - **Linux:** first `dbus-send --session --dest=org.freedesktop.FileManager1
    --type=method_call /org/freedesktop/FileManager1
    org.freedesktop.FileManager1.ShowItems array:string:<file URI> string:`.
    - The URI percent-encodes every byte except unreserved characters and
      `/`, so commas, which `dbus-send` treats as array separators, become
      `%2C`. Spaces, unicode and quotes are encoded too.
    - Fallback: `xdg-open <parent dir>`.
    - This uses no new D-Bus crate.
- Tests cover argument construction for every OS branch, whatever the host
  OS: spaces, unicode, commas, `%`, `#`, a leading `-`, UNC and verbatim
  paths. Nothing is launched in CI.
- Disabled for compositions, folders, solids and placeholders. An offline
  file shows the disabled entry "File not found", next to "Relink…".

## 7. Tabs and navigation (UI state)

### 7.1 Model (`voxy-ui`, not `voxy-core`)
```rust
pub struct Workspace { pub tabs: Vec<CompTab>, pub active: Option<usize> }
pub struct CompTab {
    pub comp: CompId,
    pub path: Vec<NavStep>,       // breadcrumb: how we got here; last == comp
    pub time: Time,
    pub view: TimelineView,       // zoom, scroll, expanded rows (existing type)
    pub graph: GraphView,
    pub selection: Vec<LayerId>,
    pub selected_keys: HashSet<(PropId, Time)>,
    pub link_playhead: bool,
}
pub struct NavStep { pub comp: CompId, pub via_layer: Option<LayerId> }
```
- **One tab per composition, never duplicates.** "Open" switches to the
  existing tab or appends a new one.
- **Double-click** on a precomp layer (timeline or viewport) or a pool
  composition (§13 Q6). A preference, **"Precomp double-click"**, chooses:
  - **"Open precomps in current tab"** (default): double-click
    **navigates the current tab** into the precomp, and the breadcrumb
    grows. If the precomp already has its own tab, it switches to that tab
    instead. **Shift+double-click** and "Open in New Tab" open or switch to
    the composition's own tab.
  - **"Always open in own tab"**: double-click opens or switches to the
    composition's own tab; Shift+double-click navigates the current tab.
  - Either way there is one tab per composition.
  - From the media panel, double-click always opens or switches to the
    composition's own tab, with a fresh breadcrumb.
- Double-clicking any other layer type keeps its current behavior.
- **Closing a tab** never touches the document.
- **Breadcrumbs:**
  - Clicking an entry navigates up within the tab.
  - More than 4 levels keeps the first and the last 2, with a `…` menu
    listing the hidden middle.
  - Labels are read from the document every frame, so a rename shows
    immediately.
- **"Go to Parent Composition":** Composition menu, shortcut **Ctrl/⌘+Shift+Up**
  (Alt+Up conflicts with the OS on some Linux desktops). It pops one step.
  With an empty path (opened from the pool), the parent is chosen from
  `usages()` with the §6.2 tie rule, or the entry is disabled when nothing
  uses the composition.
- **Navigation never executes a command.** A property test runs random
  open, up and switch sequences and asserts that the document and revision
  are unchanged.

### 7.2 Playhead mapping

`fn child_time(layer: &Layer, child: &Composition, t: Time) -> Option<Time>`
is shared with the renderer. It is defined in core:
1. If `t ∉ [in, out)`, the layer is inactive. For navigation, clamp `t` into
   `[in, out−1]` first.
2. **With remap:** `c = round_to_flick(remap.value_at(t) seconds)`.
3. **Without remap:** `c = start_offset + round((t − in) × speed)`, in `i128`.
   Speed `1.0` uses the exact path with no float.
4. **Loop:** `c = c.rem_euclid(child.duration)`.
5. Entering a precomp sets the child playhead to `clamp(c, 0, duration − 1 frame)`,
   snapped down to the child's frame grid.

**Linked playheads** (per-tab toggle; §13 Q5):
- Scrubbing the parent drives the child through `child_time`.
- The reverse direction is only defined when the mapping is invertible: no
  remap, no loop, speed ≠ 0. Otherwise, scrubbing the child does not move
  the parent, and a hint says so.

### 7.3 Parent-time indicator
- In a child tab whose last `NavStep` has `via_layer`, the ruler shades the
  child times outside `[child_time(in), child_time(out−1)]`, with ticks at
  those points.
- With remap or loop, the shaded region comes from sampling
  `child_time` on parent frames: the min/max of the mapped range. A loop
  covering the whole range shades nothing.

### 7.4 Undo across tabs
- There is one global `History`, as today.
- After undo or redo, the UI diffs the two snapshots by `Arc::ptr_eq` over
  `comps`, which is cheap and exact.
- If the active tab's composition did not change, but another one did, the
  UI switches to (or reopens) the changed composition's tab, choosing the
  lowest ID when several changed. It then selects the layers whose `Arc`s
  changed.
- `History` itself is unchanged.

### 7.5 Parent backdrop (last)
- A per-tab toggle.
- The viewport first renders the parent composition at the mapped parent
  time, at 30% opacity, under the child render. The child's bounds are drawn
  as a rectangle placed by the precomp layer's transform.
- **Viewport only:** export and the document never see it.

## 8. Precomp rendering

### 8.1 Plan
- `RenderPlan` gains `DrawSource::Comp { plan: Arc<RenderPlan>, key: ContentKey }`.
- `evaluate` recurses into the precomp:
  - `EvaluatedContent::Comp { comp, child_time, eval: Box<EvaluatedComp> }`
    when collapse is off;
  - the child's layers are inlined into the parent list, with concatenated
    matrices, when collapse is on.
- The compositor renders a `Comp` draw by:
  1. rendering its sub-plan into a pooled offscreen `Rgba16Float` target at
     **child size × preview scale**, cleared to **transparent**, in the
     project working space. As in AE, a composition's background color shows
     only when the composition is viewed or exported directly, never when
     it is nested, so unprecompose has no background to lose;
  2. drawing that texture like an image layer, with the precomp layer's
     transform and opacity.
- **Collapse transformations (2D):**
  - No offscreen pass, and the child background is not drawn.
  - The layer opacity multiplies into each child layer's opacity.
  - Child blend modes apply against the parent's stack.
  - Content outside the child's bounds is visible.
  - No cache entry exists for the precomp, because its draws hash
    individually as part of the parent.
- **Depth:** at most 16 nested levels. `evaluate` carries a stack of
  `CompId`s. A re-entry (a cycle that slipped past validation) or depth 17
  draws nothing for that layer and adds a `RenderReport` warning.
  Validation should make this unreachable.

### 8.2 Sampling rules
Decided (§13 Q3). Two rules, one for footage and one for nested
compositions:

- **Footage at a different frame rate: hold.** A video layer shows the
  source frame on screen at its source time, `t − in_point + start_offset`:
  the last frame that started at or before that time. A 24 fps clip in a
  60 fps composition therefore repeats each frame 2–3 times. This is the
  existing decode rule (Milestone 3c), shared by preview and export. Blend
  and optical flow are later per-layer options.
- **Nested compositions: exact mapped time.** The child is evaluated at
  the exact child time in flicks that `Layer::child_time` gives for the
  parent's time (continuous sampling). This is AE's default, with "preserve
  frame rate" off.
  - A child at 24 fps inside a 60 fps parent therefore animates at 60 fps.
    Footage inside the child still follows the hold rule above.
  - A per-composition flag, "Hold child frame rate when nested", is
    proposed for later. It floors the mapped time to the child's frame grid.
- **Resolution:** the child renders at its own size (× preview scale), and
  the parent samples it bilinearly through the layer transform, the same
  filter as images today.
- **Region of interest:** v1 renders the full child canvas. The plan records
  the region of definition (the child's bounds) so ROI clipping can be added
  without a format or API change.

### 8.3 Cache
- `ContentKey = hash(sub-plan)`. It is a stable 64-bit FNV over a canonical
  byte encoding of the resolved sub-plan:
  - draw sources, `f64::to_bits` of every matrix and color;
  - video frame identity `(asset, source time)`;
  - width, height, scale and working space.
- Hashing the *resolved* plan rather than "content + settings + mapped time"
  is stricter and cheaper:
  - two times that resolve identically share an entry, so a static title
    renders once;
  - any settings change alters the plan and therefore the key, so a stale
    frame is impossible by construction.
- Storage: a GPU texture LRU (budget 512 MiB) inside `Compositor`.
- **Flag (determinism):** an offscreen pass plus resampling changes pixels
  compared with drawing the layers directly. It is deterministic on the same
  machine (Q4), and new golden images are added. Existing goldens have no
  precomps and do not change.

### 8.4 Audio
- `child_time` is the single mapping for both video and audio.
- Audio from precomps is mixed when audio layers exist (see §0). The plan:
  the mixer walks the same tree, using the mapped time and speed as the
  resampling ratio.

## 9. Persistence

### 9.1 Format 0.4.0
- **Added:** `folders`; asset fields `content_hash`, `audio` and `folder`;
  composition fields `pixel_aspect`, `work_area` and `folder`;
  `meta.settings`; layer kinds `precomp` and `placeholder`;
  `root_comp` → `main_comp` (nullable).
- **Migration 0.3.0 → 0.4.0:**
  - `main_comp = root_comp`;
  - every item goes in the root folder;
  - `pixel_aspect = 1`, no work area, and default project settings.
  - The result is the legacy project as a single composition with its media
    at the root.
- **Newer files** are refused with a clear message (existing policy). voXY
  never writes over them.
- `docs/voxy-format.md` gains the 0.4.0 row and field tables.

### 9.2 UI state
- **Separate section.** `ui_state` is a sibling of `project` in the
  envelope, not inside it:
  ```json
  { "format": "voxy-project", "format_version": "0.4.0", "generator": "…",
    "project": { … },
    "ui_state": { "version": 1, "tabs": [ … ], "active": 0 } }
  ```
  - Saved: tabs, the active tab, per-tab composition, path, time, zoom,
    scroll, selection and link toggle.
  - Media panel state: expanded folders, sort, search.
- **Loading UI state:**
  - Load errors are swallowed per field: unknown versions, or tabs pointing
    at missing compositions or layers, are dropped silently.
  - It is never passed to `voxy-core`. `persist::from_json` returns
    `(Document, Option<serde_json::Value>)`, and `voxy-ui` parses the value.
  - Rendering never reads it.
  - Saving UI state alone does not mark the document dirty. The existing
    `is_dirty` covers document edits only.
- **Autosave and crash recovery:** they don't exist yet (§0). When added,
  they write the same envelope, including `ui_state`, so this needs no extra
  design.

## 10. Alight Motion

- Nothing is assumed. `docs/am-format.md` gets a section for media names,
  nesting and groups, and project size and rate. Each finding is marked
  *confirmed* or *inferred*, as the existing rules require.
- Mapping targets, if the samples show them: nested element → `precomp`,
  group → `precomp` with collapse on, unless the samples show group
  semantics that need a real group layer (to be decided then).
- Unknown data stays in `foreign`. Export reports anything unrepresentable.
- **Sample requests for the owner:**
  1. one project, exported twice, before and after renaming one imported
     media item;
  2. one project at a non-default resolution and frame rate (for example
     1080×1350 at 24 fps), with a second export at the default, to diff;
  3. a project with one group, and a second with one nested or element
     compound, each also exported before grouping, to diff.

## 11. Edge cases

| Case | Behavior |
|---|---|
| The composition behind an open tab is removed (or undone out of existence) | The tab closes, with the status message "“Intro” was removed; its tab was closed". Parent layers become placeholders (remove-mode default). Breadcrumbs in other tabs drop the missing steps. |
| Reveal into a closed tab | The tab is reopened. |
| Offline media inside a child composition | Reveal selects the offline item, which shows the missing badge. |
| Rename while the tab is open | The tab title, breadcrumbs, viewport header and timeline header all read the name live. |
| Precomp drop that would cycle | Blocked (§5). |
| Undo removes a composition that tabs reference | Same as the first row. Redo does not reopen the tab. |
| Main composition removed | Promotion rule (§1.4). |

## 12. Tests (by milestone)
- **A:**
  - Every command in §2: success, errors and undo identity.
  - Cycle detection, direct and indirect.
  - Usage queries.
  - `child_time` for offset, speed, loop and remap.
  - Rate change keeps flick positions, and snap collisions.
  - Resolution change, both modes.
  - Duplicate with fresh IDs.
  - IDs not reused after undo.
  - Migration 0.3.0 → 0.4.0, round trip, and newer-version refusal.
  - `proptest`: random command, drop and precompose sequences, then full
    undo, equal the original.
- **B:**
  - Reveal target resolution, including the multi-composition rule.
  - Launcher argument tests for every OS branch.
  - Panel sort and filter logic.
- **C:**
  - Settings validation and the warning summary ("N keys off-grid", "layers
    scaled").
  - Stale-cache tests for resolution and rate.
- **D:**
  - `resolve_drop`: stack versus sequence, index, durations, Alt replace,
    cycle blocked, new composition from a drop, OS import plus place as one
    step.
- **E:**
  - Precompose then undo.
  - Navigation never changes the document (proptest).
  - Tab, undo and switch logic.
  - Goldens: a precomp at a different size, rate and remap; and an edit made
    in the child equals the parent's render.
- **F:**
  - Unprecompose bake: exact and reported cases.
  - Collapse goldens.
  - `ui_state` round trip and missing-composition tabs dropped.
  - `ReplaceMedia` warnings.

UI tests target the command and state layer (the `resolve_drop`, reveal and
navigation functions), not egui drawing.

## 13. Decisions (approved by the owner)

| # | Question | Decision |
|---|---|---|
| Q1 | Duplicate names | **Allowed.** Menus show the item's ID when names collide. |
| Q2 | Resolution change | **Keep layers**, with the 3×3 anchor grid (default center). The dialog offers **"Scale layers to fit"** as the alternative. |
| Q3 | Different frame rates | **Footage holds the current frame; precomps are sampled at the exact mapped time.** Both rules are in §8.2. |
| Q4 | Multi-item drop | **Stack by default.** Ctrl/⌘ sequences, Shift toggles snapping, Alt/Option replaces a layer's source. |
| Q5 | Linked playheads | **Off by default**, per-tab toggle. |
| Q6 | Double-click a precomp layer | **A preference**: "Open precomps in current tab" (default) or "Always open in own tab". One tab per composition either way (§7.1). |
| Q7 | Removed sources vs. missing files | **Placeholder layer kind** for removed sources; missing files stay in the pool with a missing badge and Relink. |
| Q8 | Systems that don't exist yet | **Build now:** work area, viewport layer picking, named-action table. **Defer** (tracked in [`deferred.md`](deferred.md), code kept ready): export overrides and the even-size check, precomp audio mixing, autosave and crash recovery (scheduled after Milestone F, not dropped), markers. |
| Q9 | Main composition | **Optional;** zero compositions allowed. |
| — | ID high-water mark in `History` | **Approved**, with a test that IDs are never reused after undo then new edits. |
| — | Keyframes in composition time | **Kept;** pre-compose and unprecompose shift keyframe times, with round-trip tests. |

The original proposal table, kept for reference:

| # | Question | Options | Proposed |
|---|---|---|---|
| Q1 | Duplicate names | (a) allow duplicates, as AE does; (b) auto-suffix "Name 2" | **(a) allow.** IDs disambiguate, the menus show a small `#id` hint only when names collide, and new items and duplicates still get auto-numbered default names. |
| Q2 | Default on resolution change | (a) keep layers, anchor center; (b) scale proportionally | **(a)**, with the 3×3 anchor grid. |
| Q3 | Frame-rate sampling | Footage: hold, blend, or optical flow later. Nested compositions: continuous or hold the child's rate. | **Footage: hold** (the existing decode rule). **Nested: continuous.** Both are AE defaults. |
| Q4 | Multi-item drop | (a) stack, Ctrl/⌘ to sequence; (b) sequence, Ctrl/⌘ to stack | **(a) stack**, with Shift toggling snapping and Alt replacing. |
| Q5 | Linked playheads | default on or off | **Off.** It is a per-tab toggle. |
| Q6 | Double-click a precomp layer | (a) navigate in the current tab (breadcrumb grows), Shift for a separate tab; (b) open or switch to its own tab, Shift to navigate in place | **(a)** for layers. Pool double-click always uses the composition's own tab. |
| Q7 | Placeholders versus offline (§1.6) | Placeholder layer kind for removed sources, offline badge for missing files | **As described.** |
| Q8 | Systems that don't exist yet (§0) | For each: build a minimal version in this work, or defer with the hook in place | **Build:** work area, viewport hit testing, the action table (bindable IDs; editor UI later). **Defer, with the API ready:** export overrides and the even-size check (export milestone), precomp audio mixing (audio-layer milestone), autosave (its own milestone), markers (snapping hook ready). |
| Q9 | `root_comp` → nullable `main_comp` | Allow zero compositions (AE) or keep at least one | **Allow zero.** The timeline shows "No composition open — drop media here or create one". |

**Flags:**
- **Determinism:** precomp offscreen rendering (§8.3).
- **Undo:** the ID high-water mark (§1.1) and the `CommandReport` return
  value. Undo and redo results do not change.
- **Color space:** not affected.
- **OFX sandbox:** not affected.

## Implementation notes (Milestone A)

What landed in `voxy-core`, and where it refines the design above.

- **Model.** `FolderId`, `Folder`, `PoolItem`, `SourceRef`; asset
  `content_hash`, `audio`, `folder`; composition `pixel_aspect`,
  `work_area`, `folder`; `LayerKind::Precomp` and `LayerKind::Placeholder`;
  `ProjectSettings` in `meta.settings`; `Document::main_comp:
  Option<CompId>` and `Document::main()`. `AssetKind` gains no audio or SVG
  variants until those layer types exist.
- **Queries** (`pool.rs`): `usages`, `usages_of`, `Usage::primary` (the
  §6.2 tie rule), `expand_items`, `removal_impact` (what the removal dialog
  lists), `comp_contains`, `nesting_error` (names the chain, e.g. "“C”
  can't be placed in “A”: “C” already contains it (“C” › “B” › “A”)").
- **Time mapping.** `Layer::child_time` implements §7.2 and is what
  evaluation, unprecompose and (later) navigation and audio use.
  `evaluate` produces `EvaluatedContent::Precomp { comp, child_time, size }`;
  the render plan draws nothing for it until Milestone E. Placeholders draw
  a grey rectangle at their remembered size.
- **Commands** (`command/pool.rs`, `settings.rs`, `precomp.rs`):
  `RenameMedia`, `RenameComposition`, `RenameFolder`, `CreateFolder`,
  `MoveToFolder`, `RemoveItems` (one command for media, compositions and
  folders), `CreateComposition`, `DuplicateComposition`, `ReplaceMedia`,
  `ReplaceLayerSource`, `AddSourceLayer` (the single-item building block
  for drops in Milestone D), `SetPrecompTiming`, `SetCollapse`,
  `SetCompositionSettings`, `SetProjectSettings`, `SetWorkArea`,
  `PrecomposeLayers`, `UnprecomposeLayer`. `DropItems` comes with
  Milestone D.
- **`CommandReport`.** `Command::apply_reported` and
  `History::execute_reported` return created items and warnings;
  `execute` is unchanged for existing callers.
- **ID high-water mark.** `History` raises each edit's starting `next_id`
  to the highest it has seen. A command that only would raise the counter is
  still a no-op. Tested directly and by a property test over random edit
  sequences with undo and redo.
- **Settings validation** happens in commands (16–16384 px, ≤ 240 fps, one
  frame to 3 hours, pixel aspect 0.25–4), so older files with other values
  still load.
- **Pre-compose details.**
  - Selected layers that are not adjacent end up together at the position
    of the topmost one, as in AE.
  - A parent link crossing the new composition boundary is replaced by
    baking the parent's world transform at the given time into the child's
    position (all keys), rotation and scale. The report says so and marks
    it approximate if the parent is animated or unevenly scaled.
  - Default precomp name: `precomp_name()` gives "<comp> Precomp 1", or the
    next free number.
- **Unprecompose details.**
  - The transform layer is skipped when the precomp layer's transform is a
    static identity with no parent, which makes pre-compose → unprecompose
    return the original layers exactly (tested).
  - With remap or looping, animated properties get one linear key per
    parent frame (at most 100 000 frames).
- **Frame-rate snapping.** "Nearest frame" ties go to the earlier frame.
  Snapping a layer's in-point moves its `start_offset` with it, so the
  source stays where it was.
- **Format 0.4.0** (`docs/voxy-format.md`). Loading goes through a JSON
  value so the 0.3.0 → 0.4.0 migration can rename `root_comp`.
  `persist::{to,from}_json_with_ui` and `{save,load}_with_ui` carry the
  `ui_state` section unparsed; `voxy-core` never interprets it.
- **UI.** Until composition tabs land (Milestone E), the editor shows the
  main composition (`EditorState::comp_id`), and every panel handles a
  project with none. The viewport's frame key now includes the composition
  ID, so switching compositions can never show a stale frame.
