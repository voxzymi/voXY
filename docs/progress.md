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
      - 1b: the plan chain on the CPU (kernel table in `voxy-render`, RoD
        and ROI, content keys), with unit tests.
      - 1c: GPU execution (layer-local targets, chain cache) with the first
        kernel (invert) and a golden.
   2. The blend modes (ping-pong shader), with CPU reference tests.
   3. The blurs (gaussian, box, directional), with goldens and the ROI test.
   4. The colour basics (brightness/contrast, exposure, invert, tint,
      hue/saturation), with goldens.
   5. Determinism and cache tests, and `docs/effects.md` for group 1.
3. E-C, then E-D, then O-A to O-D.

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

## Verified only on Linux

- Nothing yet.
