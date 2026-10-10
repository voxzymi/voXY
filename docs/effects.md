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
