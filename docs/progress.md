# Progress: effects system and OpenFX hosting

Working branch: `claude/laughing-meitner-l06q1i`, started from `origin/main`
at `743b053` (merge of #3).

## Done

- **Step 0: repo sync and state report.** Local `main` fast-forwarded to
  `origin/main`. The working branch is at the same commit. State found:

  | Area | State |
  |---|---|
  | Shared property type | Exists: `voxy-core` `Property` / `PropertyDesc` (key, label, `ValueKind`, default, hard and soft range, `Unit`, animatable, spatial). `Value` covers Bool, Int, Scalar, Vec2, Vec3, Color, Enum, String. There is no layer-reference value. `Unit` covers None, Pixels, Degrees, Factor, Normalized, Seconds. |
  | Keyframes and easings | Exist: hold, linear, bézier and the five parametric easings (spring, bounce, oscillation, steps, stepped ramp), plus the graph editor and easing presets in `preferences.toml`. |
  | Effects | None. `Layer` has no effect stack, and there is no registry. `EffectId` is only mentioned in the design proposal. |
  | Blend modes | `BlendMode` has only `Normal`. The compositor uses fixed-function premultiplied "over". |
  | Masks and mattes | None in the model or the renderer. They are specced in `design-proposal.md` §3.1 and §4.2. |
  | Adjustment layers | No `LayerKind::Adjustment` yet. |
  | Render graph | `voxy-render::plan` is a chain of draws per composition. Nested compositions render offscreen with a content-hash key. There are no per-layer intermediate targets, no ROI, and no effect nodes. |
  | Working space | Exists: `WorkingSpace::{Linear, SrgbCompat}` per project. It is honoured by the plan and the compositor. |
  | `voxy-ofx-sys`, `voxy-ofx-host`, `voxy-ofx-sandbox` | Placeholder crates only, with no code and no dependencies. `third_party/openfx/` (the vendored headers) does not exist, and xtask has no `bindgen` command yet. |
  | Project format | 0.6.0. A file is written as 0.6.0 only when it uses parametric easings. |
  | CI | Linux x64, Windows x64, macOS arm64, macOS x64. GPU golden tests run on Linux (lavapipe). There are cargo-deny and license-list checks. |

- **Step 1: design note** `docs/design-effects-ofx.md`, approved with
  D1 to D8.
- **P-0: plugin research.** The shortlist in `docs/ofx-plugins.md` is
  approved (Q1 to Q4). The credits data file is `third_party/credits.toml`,
  and the README and LICENSING.md credit blocks (between the
  `credits:begin` and `credits:end` markers) are kept in sync by hand until
  `cargo xtask credits --check` arrives in O-A. `docs/ofx-host.md` is
  started.

## Next, in order

1. E-A: the effect data model, the registry, commands and undo,
   placeholders for missing effects, format 0.7.0 only when used, and
   headless tests. It goes in small steps:
   1. **Done:** `Value::Layer`, `Value::Curve` (validated: 2 to 64 points,
      strictly increasing x, both in 0..=1, checked again on load) and
      `Value::Asset`. All three hold between keys. `AssetKind::Lut`
      moves to E-D group 2 together with the LUT effect, because it
      touches import and the Project panel.
   2. **Done:** `EffectId`, `EffectInstance` (`voxy_core::effect`),
      `Layer::effects`, and editable effect parameters in
      `Layer::properties()`.
      - Placeholders: an effect that cannot be read keeps its JSON
        verbatim, and its parameter IDs stay reserved.
      - Validation covers effect IDs and parameter keys.
   2b. **Done:** format 0.7.0, written only when a layer has effects,
       pulled forward so that no build ever saves effects under an older
       version number. It is documented in `docs/voxy-format.md`. Blend
       modes and adjustment layers join the 0.7.0 rule when they are added
       (D7).
   3. **Done:** the registry (`voxy_core::registry`: type and version
      lookup, validation of definitions, instantiation at defaults) and the
      group 1 built-in schemas. Group 1 is gaussian, box and directional
      blur, brightness/contrast, exposure (linear light), invert, tint, and
      hue/saturation/lightness. Enum parameters clamp to their choices.
   4. **Done:** the 17 `BlendMode` variants and `LayerKind::Adjustment`,
      model only: both are saved and loaded, and both write format 0.7.0
      (D7). The renderer still composites every mode as Normal until E-B,
      and draws nothing for adjustment layers until E-D. No UI sets either
      yet.
   5. **Done:** the commands (`voxy_core::command`), each one undo step:
      - `AddEffect`, `RemoveEffects`, `MoveEffect` and `DuplicateEffects`;
      - `SetEffectsEnabled`, `RenameEffect` and `SetEffectMatte` (the matte
        must be another layer in the same composition);
      - `PasteEffects`, onto several layers at once. Copies get new IDs,
        and references that leave the composition are cleared and
        reported;
      - `SetBlendMode` and `AddAdjustmentLayer`.

      Deleting layers (directly, or by removing their source) clears
      mattes and layer references to them. Copied layers (Duplicate
      Composition, Unprecompose) renumber their effects, including IDs
      inside a placeholder's preserved JSON, and remap references to the
      copies. Precompose clears references that would cross compositions.
   6. **Done:** loading resolves effects against the registry
      (`registry::resolve_effects`, `persist::from_json_resolved` with load
      warnings; plain `from_json` uses the built-ins). It fills in missing
      parameters, resets damaged values with a warning, keeps unknown
      types, newer versions or unknown parameters as placeholders, and
      restores placeholders once their definition is installed.
      - Fixed: a placeholder made from a loaded effect widened f32 colours
        to f64 digits. It is now built through text, so values stay
        exactly as in the file.
      - Migration of older built-in versions arrives with the first
        version 2 of a built-in; all are version 1 now.
   7. **Done:** the undo proptest (`tests/history_props.rs`) now mixes random
      effect-stack edits (add, remove, move, duplicate, enable, rename,
      matte, keyframed parameter edits, paste, blend mode, adjustment
      layers) with every existing layer, pool and precomp edit. Full undo
      restores the document exactly, save and load are lossless, and IDs
      are never reused. Run with 5000 cases locally.

   **E-A complete.** `cargo clippy -D warnings` is clean and all 565
   workspace tests pass on Linux.
2. E-B: the render-graph integration and group 1.
   1. The per-layer chain, in three slices:
      - 1a **done:** `EvaluatedLayer::effects` holds the enabled,
        non-placeholder effects with parameters at the frame time, plus an
        exact frame number for seeds. Placeholders are counted in
        `missing_effects`.
      - 1b **done:** `voxy_render::kernels` holds the CPU-side rules of
        each kernel (RoD and ROI, pure `f64`). The plan wraps layers with
        supported effects in `DrawSource::Chain` (source at full opacity,
        layer opacity after the effects). Chain content keys hash the
        source, every effect's type, version and evaluated parameters as
        exact bits, the scale and the working space, but not placement,
        and the frame only for seeded kernels. Effects without a kernel,
        and placeholders, pass through and are counted in the render
        report. The compositor still passes chains through.
        - Refinement of design §1.3: region rules live with the kernels in
          `voxy-render` (still GPU-free), because images and video know
          their size only when decoded.
      - 1c **done:** the compositor runs chains (`compositor/chain.rs`).
        The source is rendered into a layer-local buffer covering the
        chain's RoD at the preview scale (reduced, with a warning, if it
        would not fit the GPU). Effects run as full-buffer passes,
        ping-ponging between two textures. Results are cached by content
        key (512 MiB budget, LRU) and drawn through the layer's transform.
        - First kernel: invert, with goldens `effect_invert_linear` and
          `effect_invert_srgb_compat`.
        - Tests: inverting twice, a fully blended effect and a disabled
          effect all match the plain layer exactly. Static chains render
          once, moving a layer reuses them, and parameter and resolution
          changes re-render.
        - Fixed while testing: frames are zeroed for unseeded kernels, so
          static chains hit the cache.
        - The golden harness helpers moved to `tests/common/mod.rs`; the
          existing goldens are unchanged.
        - `docs/effects.md` started.
   2. **Done:** the blend modes. Normal keeps fixed-function "over", so
      existing goldens are unchanged. Any other mode copies the target
      into a backdrop texture and draws through `shaders/blend.wgsl`.
      - CPU reference in `voxy_render::blend`, with hand-computed values.
      - GPU test: 17 modes × 5 colour pairs × 2 working spaces, within
        2/255 (1/255 observed).
      - Golden `blend_modes`.
      - The draw's blend mode is part of the plan key.
   3. **Done:** the blurs. Gaussian and box share `convolve.wgsl`
      (separable, CPU `f64` weights via `libm`, up to 1023 taps per side);
      directional is `directional.wgsl`. Kernels can now take several
      passes, and each pass carries its own weights. The RoD grows by the
      reach unless Repeat Edge Pixels is on.
      - Tests: weights normalized and finite over the range; region rules;
        preview-scale parameters; the ROI test (a large blur spreads past
        the layer edge, and repeated edges stay inside); the half-resolution
        look matches full resolution (PSNR above 32 dB); radius 1000
        renders; two compositors give identical frames.
      - Four goldens: `effect_blur_gaussian`, `effect_blur_box`,
        `effect_blur_directional` and `effect_blur_repeat_edges`.
   4. **Done:** the colour basics.
      - Brightness/contrast, exposure (linear light, the same in both
        working spaces), tint and hue/saturation/lightness, on top of
        invert from 1c.
      - Defaults run no pass, so they are an exact pass-through (tested).
      - Outputs are capped at 60000 to stay inside RGBA16F; extreme
        settings followed by a blur stay finite (tested).
      - CPU grid test: every kernel's parameters are finite for every
        combination of min, default and max, in both spaces and at two
        scales.
      - Five goldens.
   5. **Done:** determinism and cache-key tests.
      - Every group 1 kernel stacked, with a blend mode on top, renders
        identically in fresh compositors, in both working spaces, and
        identically across frames.
      - Changing any parameter of any kernel, the version or the scale
        changes the chain key.
      - Effects apply to image and precomp sources; a collapsed precomp
        with effects renders through its chain.
      - `docs/effects.md` covers group 1 and the blend modes.

   **E-B complete.** `cargo clippy -D warnings` (Rust 1.99) is clean, and
   all 594 workspace tests pass on Linux.
3. E-C (complete, CI pending):
   1. **Done:** Effect Controls in the Properties panel.
      - One collapsible section per effect. The header has an enable
        toggle, the name (warning colour for placeholders), ▲/▼ to reorder,
        reset and remove.
      - Right-click the name for rename, copy, paste below, duplicate,
        reset and remove. "Add Effect" menu by category, and Paste.
      - Parameters reuse the keyframe controls (stopwatch, diamond, arrows,
        easing menu). New widgets for ints, enums (labelled choices), layer
        references (the other layers of the composition) and colours
        without alpha.
      - Placeholders show why they are unavailable, with read-only values.
      - Core: `ResetEffect` (all parameters back to static defaults, one
        undo step).
      - Opening and recovering projects resolve effects against the
        session's registry (`EditorState::effects`), and load warnings go to
        the status bar.
      - Effect parameters already appear in the timeline lanes and the
        graph editor, because they are layer properties.
      - Audit: every panel draws with all effects, a keyframed parameter, a
        disabled effect and a placeholder.
   2. **Done:** the Effects & Presets panel (`effects_panel.rs`).
      - It is a new dock tab, grouped with the Project panel by default.
      - Search matches every word in an effect's name, category or
        description. Categories are collapsible, with small painted glyphs
        as icons (no icon font).
      - Double-click applies an effect to the selected layer, in one undo
        step. Effects can be dragged onto a timeline layer, or onto the
        viewport, which applies them to the selected layer (the viewport
        has no layer picking yet).
      - Saved dock layouts from before the panel existed are kept: the
        panel joins the Project panel's group instead of the whole layout
        being refused (tested).
   3. **Done:** ten effect actions (twelve with step 5) in the action table, with no default
      shortcuts, so they can be rebound in the shortcut editor: Apply Last
      Effect, Copy, Paste, Duplicate, Remove, Remove All, Turn On or Off,
      Move Up, Move Down, Reset.
      - Clicking an effect's name in Effect Controls selects it, and the
        single-effect actions act on it (with a reason when they cannot).
      - New Effect menu. The Window menu already lists the Effects &
        Presets panel.
      - Tests: the full action sequence is one undo step per edit, and the
        audit runs every action with effects and a selected placeholder.
   4. **Done:** effect presets (`effect_presets.rs`).
      - Stored in `effect_presets.toml` in the per-user config directory,
        each preset holding its effects as project-file JSON (TOML cannot
        hold `null`).
      - Tolerant loader: a bad preset or file is dropped alone. Writes are
        atomic.
      - Save from the Effect Controls right-click menu (one effect, or the
        whole stack).
      - The Effects & Presets panel lists presets (filtered by the search)
        to double-click, apply or delete.
      - Applying resolves against the session registry, so the effects get
        real descriptors. It leaves out, and reports, effects that are not
        installed, and pastes in one undo step.
      - Core: `registry::resolve_effect` for effects outside a document.
   5. **Done:** solo and the before/after split.
      - `RenderSettings::effects` (`EffectsView`: all, bypass, or solo one
        effect). Export keeps the default (all); only the viewport changes
        it.
      - "Solo" in each effect header shows only that effect on its layer;
        other layers draw as usual. "Before/After" in Effect Controls
        splits the viewport: no effects left of a draggable divider.
      - The viewport has a second render target for the "before" frame,
        skipped while unchanged like the main one.
      - Both are plain editor state (`EffectsPreview`), never commands or
        saved, and reset when another project opens. Also two actions:
        Solo Effect and Before/After Split.
      - Tests: plan views (bypass, solo, a stale solo changes nothing) and
        no history entries.
   6. **Done:** on-canvas handles for point parameters (`gizmo.rs`).
      - Every enabled effect on the selected layer draws a handle for each
        parameter marked `gizmo: Point`, mapped from layer pixels through
        the layer's world transform (parents included). The effect clicked
        in Effect Controls is highlighted.
      - Dragging issues `SetProperty` at the current time (a key when the
        parameter is animated); its merge key makes one drag one undo step.
        A layer scaled to zero ignores the drag.
      - Core: the registry refuses a point gizmo on a parameter that is not
        a `Vec2`.
      - Group 1 has no point parameters, so the tests register their own
        definition. The first built-in users come in E-D.

   **E-C complete.** `cargo clippy -D warnings` (Rust 1.99) is clean, and
   all 610 workspace tests pass on Linux. CI green on all four platforms
   (run 38070338244), and `main` fast-forwarded to `9e3665d`.
4. E-D (in progress), group by group (design §5), each in small steps:
   1. **Done:** Radial Blur (spin) and Zoom Blur, the first built-ins with
      an on-canvas point (their centre).
      - Kernels get a `PassContext` (texel scale, working space and the
        buffer's origin in layer pixels), so position-based parameters
        reach the shader in buffer texels.
      - Region rules for both are tested on the CPU against sampled
        geometry: every sample a pixel reads is inside the ROI, and every
        pixel an input point reaches is inside the RoD.
      - Core: `AddEffect` starts point parameters at the centre of the
        layer's content when its size is known
        (`Document::content_size`).
      - Two goldens; `docs/effects.md` has the formulas. 613 workspace
        tests pass, clippy clean.
   2. **Done:** Levels (per channel or RGB, crossed points invert, no
      clipping above white) and Posterize. Three goldens, a CPU test of
      their parameter edge cases, and Levels joins the
      defaults-pass-through test.
   3. **Done:** Curves (RGB, red, green, blue and alpha) with a curve
      editor in Effect Controls (`curve_editor.rs`).
      - Core: `Curve::eval`, a monotone cubic (Fritsch–Carlson) in `f64`,
        tested to pass through its points, never overshoot and hold flat
        past the ends.
      - The renderer bakes four 256-entry tables (the RGB curve folded into
        red, green and blue), which fill the uniform's table exactly.
      - Editor helpers (move within neighbours, insert, remove, limits) are
        unit-tested; each edit is one `SetProperty`, so one drag is one undo
        step. A golden, and Curves joins the defaults-pass-through test.
        617 workspace tests pass, clippy clean.
   4. **Done:** Color Balance (shadow, midtone and highlight shifts with
      luma weights, optional preserve-luminosity). Two goldens.
      - Effect Controls now draws parameter groups as collapsible sections,
        nested (needed for OpenFX pages and groups too).
      - Core: the registry checks groups (unique IDs, each parent listed
        before its children, so no cycles). 617 tests pass, clippy clean.
   5. **Done:** Gradient Map (shadow, midtone and highlight stops, a
      midtone position and an amount). One golden.
   6. **Done:** `AssetKind::Lut` with import, and Apply LUT (`.cube`).
      - Core: `voxy_core::lut` (tolerant `.cube` parser, never panics, and
        `CubeLut::apply`, the CPU reference). A LUT asset is never a layer
        source (refused with the reason), and removing one clears the
        parameters that used it, in the same undo step.
      - **Format:** a project with a LUT asset is written as 0.7.0, like
        effects (docs/voxy-format.md).
      - Render: LUTs upload as 32-bit float 3D textures, read with
        `textureLoad` and interpolated by hand (bind group 2 for the LUT
        kernel only). A chain is re-rendered when a LUT's path changes, and
        a missing LUT passes through, is reported, and is retried.
      - UI: a LUT picker for asset parameters with "Import…" (one undo
        step), LUTs in the Import dialog and Project panel (own icon,
        Missing filter, relink).
      - Tests: the parser, the GPU against the CPU reference in both working
        spaces, the missing-file path, core rules, the import step.

   **E-D group 2 complete.** 624 workspace tests pass, clippy clean.

   Group 3, in steps:
   1. **Done:** Vignette and Pixelate. Kernels now also get the layer's own
      size (`PassContext::layer_size`), for effects relative to its edges.
      Three goldens and a region test for Pixelate's block grid.
   2. **Done:** Grain, the first seeded effect (design §3.4). The planner
      gives seeded kernels their layer's ID (`EffectNode::seed`, in the
      chain key) and keeps the frame only while Animate is on. Tests: same
      frame bit-identical (also across compositors), next frame different,
      Animate off still, one seed per layer; two goldens.
   3. **Done:** Chromatic Aberration (red and blue scaled `1 ± k` around a
      draggable centre). Region rules tested against sampled geometry; one
      golden. CI green on all four platforms for group 2 (run 38123139130,
      `bf3b52c`).
   4. **Done:** Drop Shadow and Inner Shadow, on new chain machinery:
      - Kernels can have several stages (shaders), with pipelines keyed by
        kernel and stage.
      - A kernel can keep its own input (bound as `orig`) while its passes
        ping-pong between the other textures; only such chains get a third
        texture. Landed first as its own commit with every golden
        unchanged.
      - Three goldens and a region and stage test.
   5. **Done:** Glow, in linear light (design §3.5): bright pass, blur,
      and an additive composite over the kept input. One golden.
   6. **Done:** Stroke (outside, behind the layer, or the outline alone),
      by jump flooding. Two goldens and an exact width check on the GPU.

   **E-D group 3 complete.** 630 workspace tests pass, clippy clean.
   Then groups 3 to 6 (group 6 brings adjustment-layer rendering and
   the transitions).
5. O-A to O-D.

## Approved decisions

- Taken over from earlier work: an out-of-process sandbox with one helper
  process per plugin bundle (design proposal Q3), and bindgen output checked
  into `voxy-ofx-sys` with headers vendored from a pinned tag.
- Step 0: effects get an optional effect-matte reference only. Layer masks
  and track mattes stay a later milestone, with a reserved slot in the render
  order.
- Step 0: add `Value::Layer` for layer-reference parameters.
- Design note approved, with all of D1 to D8 as recommended (see its §11):
  - D1: add `Value::Curve` (validated, hold only), plus `AssetKind::Lut`
    with `Value::Asset`; gradient map uses three colour stops.
  - D2: hand plugins premultiplied images and convert the output.
  - D3: `interprocess` local sockets.
  - D4: `memmap2` over per-session files, cleaned up on exit and, after a
    crash, on the next launch.
  - D5: no GPU suites in O-A to O-D.
  - D6: batch order as listed, in small groups.
  - D7: blend modes and adjustment layers also write 0.7.0.
  - D8: W3C luma weights.
  - Also: OpenFX 1.5.1 headers, the macOS universal folder, and Custom
    parameters as the only plugin state.
- P-0 answers:
  - Q1: the 11 BSD-3 OpenFX samples at `ab77951` are CI-only fixtures,
    never shipped.
  - Q2: blur, transform and keyer test plugins are written by us and
    licensed with voXY. No GPL plugins anywhere (CI, artifacts or downloads);
    Natron's plugins would need a separate decision.
  - Q3: Bytes parameters are stored as hidden base64, like Custom, capped
    at 1 MiB per value and validated on load.
  - Q4: no "Get free plugins" for now; revisit after O-C.

## Open questions waiting on the owner

- None.

## Toolchain

- CI uses the current stable Rust (1.99 since 2026-09-28), and so does
  local work now. Clippy 1.99 added `assert_is_empty`, and the new tests
  follow it.

## Verified only on Linux

- GPU golden and chain tests (`voxy-render`): lavapipe locally and on Linux
  CI. Other platforms skip them without an adapter, as before.

## Fixed along the way (outside the effects scope)

- `recovery::Session` IDs could collide on macOS (microsecond clock), which
  made `two_sessions_never_share_a_file` fail on macOS arm64 CI. IDs now
  include a per-process counter.
