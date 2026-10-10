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

## Next, in order

1. Answers on the missing pieces (see the open questions).
2. Design note `docs/design-effects-ofx.md`, then stop for approval.
3. P-0: plugin research and shortlist in `docs/ofx-plugins.md`, the credits
   TOML, and README/LICENSING scaffolding, then stop for approval.
4. E-A, E-B, E-C, then E-D, then O-A to O-D.

## Approved decisions

- Taken over from earlier work: an out-of-process sandbox with one helper
  process per plugin bundle (design proposal Q3), and bindgen output checked
  into `voxy-ofx-sys` with headers vendored from a pinned tag.

## Open questions waiting on the owner

- Masks and mattes do not exist yet, and neither does a layer-reference value
  type. How should effects relate to them? (Asked in the session.)

## Verified only on Linux

- Nothing yet.
