# Built-in effects

Each effect's formula, parameters, colour space and region rule. This file
is the specification of the WGSL kernels in
`crates/voxy-render/src/shaders/effects/` and the region rules in
`crates/voxy-render/src/kernels.rs`. The parameter schemas live in
`crates/voxy-core/src/registry/builtin.rs`. The design is in
[`design-effects-ofx.md`](design-effects-ofx.md).

## Conventions

- **Buffers** are premultiplied RGBA16F in the project's working space:
  linear light in Linear projects, sRGB-encoded values in SrgbCompat
  projects. Effects marked *working* compute on those values as they are,
  like blending does. Effects marked *linear* decode to linear light first
  and re-encode afterwards, so they mean the same in both spaces.
- **Straight colour** is `C = rgb / a` (zero where `a = 0`). Effects that
  need it unpremultiply, apply the formula, and premultiply again.
- **Pixel parameters** are in layer pixels and are multiplied by the preview
  scale, so the look does not change at reduced preview resolution.
- **Regions:** RoD is how far the output can reach; ROI is which input it
  reads. "Same" means the region is unchanged.
- **Version:** every entry gives the definition version the formula belongs
  to. A changed formula means a new version (design §1.3).

## Color

### Invert (`voxy.color.invert`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Channel (`channel`) | enum: RGB, Red, Green, Blue, Alpha | — | RGB |
| Blend With Original (`mix`) | scalar, normalized | 0–1 | 0 |

- **Space:** working.
- **Formula:** `C` = straight colour clamped to `[0, 1]`. For RGB, Red,
  Green or Blue, each selected channel becomes `1 − C`. For Alpha,
  `a′ = 1 − a`, with the straight colour kept, so fully transparent areas
  become opaque black. Then `out = mix(premultiply(C′, a′), in, mix)` in
  premultiplied values.
- **Regions:** RoD and ROI are the same as the input.
- **Note:** values above 1 (HDR) are clamped to 1 before inverting.

## Blend modes

A layer's blend mode decides how it combines with everything below it in
its composition. Normal is fixed-function premultiplied "over". Every other
mode runs `shaders/blend.wgsl`, which reads a copy of the target taken just
before the layer is drawn. The CPU reference is
`crates/voxy-render/src/blend.rs`. The GPU tests check every mode over five
colour pairs in both working spaces, within 2/255 of the reference (1/255
observed on lavapipe), and `blend_modes.png` pins the look.

- **Space:** working (like Normal), as the project setting says.
- **Compositing:** `B(Cb, Cs)` is applied to the straight colours, then
  `co = cs·(1 − αb) + cb·(1 − αs) + αs·αb·B(Cb, Cs)` and
  `αo = αs + αb·(1 − αs)`, with `cs` and `cb` premultiplied. A transparent
  source changes nothing, and over a transparent backdrop every mode is
  plain "over".
- **Clamping:** Screen, Overlay, Soft Light, Hard Light, Color Dodge, Color
  Burn, Hue, Saturation, Color and Luminosity clamp `Cb` and `Cs` to
  `[0, 1]`. The other modes also work on HDR values.

| Mode | `B(Cb, Cs)` |
|---|---|
| Normal | `Cs` |
| Multiply | `Cb·Cs` |
| Screen | `Cb + Cs − Cb·Cs` |
| Overlay | `HardLight(Cs, Cb)`, the arguments swapped |
| Soft Light | `Cs ≤ ½`: `Cb − (1 − 2Cs)·Cb·(1 − Cb)`; otherwise `Cb + (2Cs − 1)·(D(Cb) − Cb)`, where `D(x)` is `((16x − 12)x + 4)x` for `x ≤ ¼` and `√x` otherwise |
| Hard Light | `Cs ≤ ½`: `Cb·2Cs`; otherwise `Screen(Cb, 2Cs − 1)` |
| Add | `Cb + Cs` (not clamped) |
| Subtract | `max(Cb − Cs, 0)` |
| Difference | `|Cb − Cs|` |
| Color Dodge | `0` if `Cb = 0`; `1` if `Cs ≥ 1`; else `min(1, Cb / (1 − Cs))` |
| Color Burn | `1` if `Cb = 1`; `0` if `Cs ≤ 0`; else `1 − min(1, (1 − Cb) / Cs)` |
| Lighten | `max(Cb, Cs)` |
| Darken | `min(Cb, Cs)` |
| Hue | `SetLum(SetSat(Cs, Sat(Cb)), Lum(Cb))` |
| Saturation | `SetLum(SetSat(Cb, Sat(Cs)), Lum(Cb))` |
| Color | `SetLum(Cs, Lum(Cb))` |
| Luminosity | `SetLum(Cb, Lum(Cs))` |

`Lum(C) = 0.30·R + 0.59·G + 0.11·B` (W3C weights, owner decision D8).
`SetLum`, `ClipColor`, `Sat` and `SetSat` are the W3C functions. Add and
Subtract are not in the W3C specification; their formulas here are voXY's.
