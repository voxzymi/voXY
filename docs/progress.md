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
   1. `Value::Layer`, `Value::Curve` and `Value::Asset`, plus
      `AssetKind::Lut` (with validation).
   2. `EffectId`, `EffectInstance`, `Layer::effects`, and effect
      parameters in `Layer::properties()`.
   3. The registry and the built-in definitions for group 1 (schemas only).
   4. `BlendMode` variants and `LayerKind::Adjustment` (model only).
   5. Commands: add, remove, move, duplicate, enable, rename, matte,
      paste, set blend mode, new adjustment layer, and clearing layer
      references on delete.
   6. Persistence: format 0.7.0 when used, placeholders kept verbatim, and
      migrations.
   7. Tests: undo proptest, round trips, placeholders.
2. E-B, E-C, then E-D, then O-A to O-D.

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
