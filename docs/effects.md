# Built-in effects

Each effect's formula, parameters, colour space and region rule. This file
is the specification of the WGSL kernels in
`crates/voxy-render/src/shaders/effects/` and the region rules in
`crates/voxy-render/src/kernels.rs`. The parameter schemas live in
`crates/voxy-core/src/registry/builtin.rs`. The design is in
[`design-effects-ofx.md`](design-effects-ofx.md).

## Layer inputs

Some effects read another layer of the same composition: Displacement
Map's map layer and every transition's Transition To layer (later also
OpenFX clips). The rule (owner decision Q-E1, as in After Effects):

- An effect gets the referenced layer's **plain source** (its solid,
  image, video frame or nested composition), **placed by its transform**
  (parents included) in the effect layer's space.
- **Not included:** the referenced layer's own effects, its opacity, and
  its blend mode. A hidden map layer (opacity 0) still works. To use a
  layer with its effects, precompose it.
- Only the effect layer's own region is computed: the input shows where
  that region is, and nowhere else.
- A layer outside its time range, a deleted layer, or none reads as
  transparent. Deleting a referenced layer clears the reference in the same
  undo step.
- The input's source, size and placement are part of the effect chain's
  cache key; its effects and opacity are not, since they never reach it.

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

## Blur

### Gaussian Blur (`voxy.blur.gaussian`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Blurriness (`radius`) | scalar, layer px | 0–1000 (slider 0–100) | 10 |
| Blur Dimensions (`dimensions`) | enum: Horizontal and Vertical, Horizontal, Vertical | — | both |
| Repeat Edge Pixels (`repeat_edges`) | bool | — | off |

- **Space:** working.
- **Formula:** a separable gaussian, one pass per selected axis.
  - `σ = r·s / 3` texels, where `r` is the radius and `s` the preview
    scale.
  - Taps `k = −K..K` with `K = ceil(3σ)`, capped at 1023.
  - Weights `exp(−k² / 2σ²)`, normalized to sum to 1.
  - The weights are computed on the CPU in `f64` with `libm` and uploaded
    as `f32`, so they are identical on every platform.
  - Computed on premultiplied values, so transparent pixels never bleed
    colour.
  - `σ ≤ 0.25` texels (a radius below about 0.75 px) does nothing.
- **Edges:** off, pixels past the input are transparent and the blur
  spreads outward. On, the edge pixels repeat and the output stays inside
  the input.
- **Regions:** off, RoD = input padded by `r + 1` px on each blurred axis,
  and ROI = output padded the same way. On, RoD = input.

### Box Blur (`voxy.blur.box`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Blur Radius (`radius`) | scalar, layer px | 0–1000 (slider 0–50) | 5 |
| Passes (`passes`) | int | 1–5 | 1 |
| Blur Dimensions (`dimensions`) | enum, as above | — | both |
| Repeat Edge Pixels (`repeat_edges`) | bool | — | off |

- **Space:** working.
- **Formula:** the same separable pass as the gaussian, with equal weights
  over `2K + 1` texels, where `K = round(r·s)`. It is repeated `passes`
  times, and three passes look nearly gaussian. Because `K` is a whole
  number of texels, the look is approximate at reduced preview resolution.
- **Regions:** RoD = input padded by `r·passes + 1` px on each blurred axis
  (none with repeated edges). ROI is the same padding.

### Directional Blur (`voxy.blur.directional`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Direction (`direction`) | angle, degrees | any (slider ±180) | 0 |
| Blur Length (`length`) | scalar, layer px | 0–2000 (slider 0–200) | 10 |

- **Space:** working.
- **Formula:** the average of `N` bilinear samples along the line through
  the pixel at the given angle, where `N` is `ceil(L·s)` rounded up to odd
  and `L` is the length. The samples are spaced evenly from `−L/2` to
  `+L/2`. 0° is horizontal, and positive angles turn clockwise on screen.
- **Regions:** RoD = input padded by `(|cos θ|, |sin θ|)·(L/2 + 1)` px, and
  ROI is the same.

### Radial Blur (`voxy.blur.radial`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Angle (`angle`) | angle, degrees | 0–360 (slider 0–90) | 10 |
| Center (`center`) | point, layer px, on-canvas handle | any | centre of the layer's content |

- **Space:** working.
- **Formula:** with `h` half the angle and `d = p − c` (pixel minus
  centre, in texels), the average of `N` bilinear samples at
  `c + R(a)·d` for `a` evenly from `−h` to `+h`. `N` is one per texel of
  the arc `2·h·|d|`, plus one, rounded up to odd and capped at 255. Samples
  outside the buffer are transparent.
- **Regions:** with `r` the largest distance from `c` to a corner of the
  rectangle, RoD = input padded by `r·min(h, 2) + 1` px, kept inside the
  circle of radius `r` around `c` (padded by 1). ROI uses the same rule on
  the output rectangle. Angle 0 changes nothing.

### Zoom Blur (`voxy.blur.zoom`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Amount (`amount`) | scalar, factor | 0–0.9 (slider 0–0.5) | 0.2 |
| Center (`center`) | point, layer px, on-canvas handle | any | centre of the layer's content |

- **Space:** working.
- **Formula:** with `d = p − c` in texels and amount `a`, the average of
  `N` bilinear samples at `c + d·k` for `k` evenly from 1 down to `1 − a`.
  `N` is one per texel of `a·|d|`, plus one, rounded up to odd and capped
  at 255. Samples outside the buffer are transparent. Bright areas streak
  outwards, away from the centre.
- **Regions:** RoD = the input and the input scaled by `1 / (1 − a)`
  around `c`, padded by 1 px (hence the 0.9 cap: at most 10×). ROI = the
  output and the output scaled by `1 − a` around `c`, padded by 1 px.
  Amount 0 changes nothing.

Point parameters (Radial and Zoom Blur centres) start at the centre of the
layer's content when the effect is added and its size is known; otherwise
at the layer origin.

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

### Brightness & Contrast (`voxy.color.brightness_contrast`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Brightness (`brightness`) | scalar, normalized | −1–1 | 0 |
| Contrast (`contrast`) | scalar, normalized | −1–1 | 0 |

- **Space:** working.
- **Formula:** `C′ = max((C − ½)·k + ½ + brightness, 0)` on the straight
  colour, with alpha unchanged.
  - The contrast factor is `k = 1 + c` for `c < 0`, which reaches a flat
    grey at −1, and `k = 1 / (1 − 0.99·c)` for `c ≥ 0`, which reaches 100
    at 1. It is continuous, with `k = 1` at 0.
  - Values above 1 are kept (HDR). Results are capped at 60000 so they
    never overflow half floats.
- **Defaults** (0, 0) run no pass: an exact pass-through.
- **Regions:** same.

### Exposure (`voxy.color.exposure`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Exposure (`exposure`) | scalar, stops | −20–20 (slider ±5) | 0 |
| Offset (`offset`) | scalar | −1–1 (slider ±0.1) | 0 |
| Gamma Correction (`gamma`) | scalar | 0.01–10 (slider 0.2–5) | 1 |

- **Space:** linear. In SrgbCompat projects the effect decodes to linear
  first and re-encodes afterwards, so it looks the same in both working
  spaces (tested).
- **Formula:** `C′ = max(C·2^exposure + offset, 0)^(1/gamma)` in linear
  light, capped at 60000.
- **Defaults** (0, 0, 1) run no pass.
- **Regions:** same.

### Tint (`voxy.color.tint`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Map Black To (`map_black`) | colour (alpha unused) | — | black |
| Map White To (`map_white`) | colour (alpha unused) | — | white |
| Amount to Tint (`amount`) | scalar, normalized | 0–1 | 1 |

- **Space:** working. The colours are encoded for the working space.
- **Formula:** `L = clamp(Rec709Luma(C), 0, 1)` with weights
  `0.2126, 0.7152, 0.0722`, then `T = mix(black, white, L)` and
  `C′ = mix(C, T, amount)`.
- **Amount 0** runs no pass.
- **Regions:** same.

### Hue / Saturation / Lightness (`voxy.color.hue_saturation`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Hue (`hue`) | angle, degrees | any (slider ±180) | 0 |
| Saturation (`saturation`) | scalar, normalized | −1–1 | 0 |
| Lightness (`lightness`) | scalar, normalized | −1–1 | 0 |
| Colorize (`colorize`) | bool | — | off |
| Colorize Hue (`colorize_hue`) | angle, degrees | any (slider 0–360) | 0 |
| Colorize Saturation (`colorize_saturation`) | scalar, normalized | 0–1 | 0.25 |

- **Space:** working. The straight colour is clamped to `[0, 1]` and
  converted to HSL.
- **Formula:**
  - Without Colorize: `H′ = H + hue`, and `S′ = clamp(S·(1 + saturation), 0, 1)`.
  - With Colorize: `H′` is Colorize Hue, `S′` is Colorize Saturation, and
    `L` is kept.
  - Lightness then applies: `L + (1 − L)·l` for `l > 0`, `L·(1 + l)` for
    `l ≤ 0`.
  - Angles are reduced to turns in `f64` on the CPU, so large angles stay
    exact.
- **Defaults** run no pass.
- **Regions:** same.

### Levels (`voxy.color.levels`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Channel (`channel`) | enum: RGB, Red, Green, Blue, Alpha | — | RGB |
| Input Black (`input_black`) | scalar, normalized | 0–1 | 0 |
| Input White (`input_white`) | scalar, normalized | 0–1 | 1 |
| Gamma (`gamma`) | scalar, factor | 0.1–10 (slider 0.2–5) | 1 |
| Output Black (`output_black`) | scalar, normalized | 0–1 | 0 |
| Output White (`output_white`) | scalar, normalized | 0–1 | 1 |

- **Space:** working, on straight colour.
- **Formula:** for each selected channel `v`,
  `x = max((v − ib) / (iw − ib), 0)` and
  `v' = ob + (ow − ob)·x^(1/γ)`. Values above the input white keep going
  (no clipping), so HDR values survive. Crossed points (`iw < ib`) invert
  the channel. When `|iw − ib| < 10⁻⁴` the span is taken as `10⁻⁴`, a hard
  step at that point. The Alpha choice changes alpha only (clamped to
  `[0, 1]`) and keeps the straight colour.
- **Regions:** per pixel. The defaults change nothing.

### Curves (`voxy.color.curves`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| RGB (`master`) | curve | 2–64 points in `[0, 1]²` | identity |
| Red, Green, Blue (`red`, `green`, `blue`) | curve | as above | identity |
| Alpha (`alpha`) | curve | as above | identity |

- **Space:** working, on straight colour clamped to `[0, 1]` (Curves clips
  HDR values).
- **Curve:** a monotone cubic through the points (Fritsch–Carlson
  tangents: the mean of the neighbouring slopes, zero at a local extreme,
  then scaled so `α² + β² ≤ 9`), flat before the first point and after the
  last, clamped to `[0, 1]`. It never overshoots its points. Evaluated in
  `f64` (`voxy_core::Curve::eval`). Curves hold between keyframes (D1).
- **Formula:** `r' = RGB(Red(r))`, likewise for green and blue, and
  `a' = Alpha(a)`. The CPU samples each composite at 256 evenly spaced
  inputs, and the shader interpolates linearly between samples.
- **Regions:** per pixel. All-identity curves change nothing.
- **Editor:** in Effect Controls. Drag a point to move it (kept between its
  neighbours), drag on empty space to add one, double-click to remove one
  (at least two stay).

### Color Balance (`voxy.color.color_balance`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Shadows, Midtones, Highlights → Cyan / Red, Magenta / Green, Yellow / Blue (`shadow_red` … `high_blue`, nine in all) | scalar, normalized | −1–1 (slider ±0.5) | 0 |
| Preserve Luminosity (`preserve_luminosity`) | bool | — | on |

- **Space:** working, on straight colour.
- **Formula:** with `L` the Rec. 709 luma clamped to `[0, 1]`, the three
  ranges are weighted `(1 − L)²`, `2L(1 − L)` and `L²` (they sum to 1), and
  `rgb' = rgb + Σ weightᵢ·shiftᵢ`. With Preserve Luminosity, the luma
  difference is added back: `rgb'' = rgb' + (luma(rgb) − luma(rgb'))`. The
  result is clamped at 0 below; HDR values above 1 are kept.
- **Regions:** per pixel. All shifts at 0 change nothing.
- The three ranges are collapsible groups in Effect Controls.

### Gradient Map (`voxy.color.gradient_map`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Shadows (`shadow`) | colour, no alpha | — | black |
| Midtones (`midtone`) | colour, no alpha | — | sRGB grey 128 |
| Highlights (`highlight`) | colour, no alpha | — | white |
| Midtone Position (`midpoint`) | scalar, normalized | 0.01–0.99 | 0.5 |
| Amount (`amount`) | scalar, normalized | 0–1 | 1 |

- **Space:** working; the stops are encoded for the working space, and the
  gradient is interpolated there.
- **Formula:** with `L` the Rec. 709 luma clamped to `[0, 1]` and `m` the
  midtone position, the mapped colour is `mix(shadow, midtone, L/m)` for
  `L ≤ m`, else `mix(midtone, highlight, (L − m)/(1 − m))`. The output is
  `mix(rgb, mapped, amount)`; alpha is kept. The defaults give a grey
  version of the layer (a map, not the identity); amount 0 changes nothing.
- **Regions:** per pixel.
- A general gradient value comes with shapes later (D1).

### Apply LUT (`voxy.color.lut`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| LUT (`lut`) | asset (a project LUT) | — | none |
| LUT Input (`input`) | enum: sRGB-encoded, Linear light | — | sRGB-encoded |
| Amount (`amount`) | scalar, normalized | 0–1 | 1 |

- **File:** a 3D `.cube` table (`LUT_3D_SIZE` 2–129, optional
  `DOMAIN_MIN`/`DOMAIN_MAX` and `TITLE`; red changes fastest). 1D tables are
  refused for now. Parsed by `voxy_core::lut::parse_cube`; a damaged file
  is refused at import and passes the image through when rendered.
- **Space:** the straight colour is first encoded as the table expects:
  sRGB-encoded values (in a linear project, the working values are encoded
  with the sRGB curve), or linear light. The result is decoded back.
- **Formula:** inputs are mapped from the domain onto the table and
  clamped, then interpolated trilinearly between the eight surrounding
  entries (`CubeLut::apply` is the CPU reference; the shader does the same
  arithmetic on a 32-bit float 3D texture). The output is
  `mix(encoded, table(encoded), amount)`, decoded; alpha is kept.
- **Regions:** per pixel. No LUT or amount 0 changes nothing; a missing
  file is reported with the other missing media.
- **UI:** pick a LUT from the project, or "Import…" a `.cube` file and use
  it in one undo step. LUTs are listed in the Project panel with their own
  icon, and relink like media.

### Posterize (`voxy.color.posterize`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Levels (`levels`) | integer | 2–256 (slider 2–32) | 6 |

- **Space:** working, on straight colour; alpha is kept.
- **Formula:** with `n` levels, each channel clamped to `[0, 1]` becomes
  `floor(v·(n − 1) + 0.5) / (n − 1)`.
- **Regions:** per pixel.

## Stylize

### Vignette (`voxy.stylize.vignette`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Amount (`amount`) | scalar, normalized | 0–1 | 0.5 |
| Size (`size`) | scalar, normalized | 0–1.5 | 0.6 |
| Feather (`feather`) | scalar, normalized | 0–1.5 | 0.6 |
| Color (`color`) | colour, no alpha | — | black |

- **Space:** working, on straight colour; alpha is kept (only the layer's
  own pixels change).
- **Formula:** with `d` the distance from the layer's centre in an ellipse
  fitted to the layer (1 at the middle of each edge, √2 at the corners),
  `t = amount · smoothstep(size, size + max(feather, 0.001), d)` and
  `rgb' = mix(rgb, color, t)`.
- **Regions:** per pixel. Amount 0 changes nothing.

### Pixelate (`voxy.stylize.pixelate`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Block Size (`cell`) | scalar, layer px | 1–1000 (slider 1–100) | 10 |

- **Space:** working, premultiplied.
- **Formula:** blocks of `cell` layer pixels on a grid starting at the
  layer's origin; each pixel shows the average of its block. A block side
  takes one sample per texel, up to 16 samples (larger blocks are averaged
  from a 16 × 16 grid of texels). Blocks at the layer's edge also average
  the transparent pixels beyond it.
- **Regions:** RoD and ROI = the rectangle grown to whole blocks. A size
  of 1 changes nothing.

### Grain (`voxy.stylize.grain`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Amount (`amount`) | scalar, normalized | 0–1 (slider 0–0.5) | 0.1 |
| Grain Size (`size`) | scalar, layer px | 0.5–50 (slider 0.5–10) | 1 |
| Monochrome (`monochrome`) | bool | — | on |
| Animate (`animate`) | bool | — | on |
| Random Seed (`seed`) | integer | 0–1 000 000 | 0 |

- **Space:** working, on straight colour; alpha is kept, transparent
  pixels stay transparent, and the result is clamped at 0.
- **Seed (design §3.4):** the base seed is an FNV-1a hash of the layer's
  ID, the seed parameter and the frame (`floor(t × fps)`, exact; 0 when
  Animate is off), folded to 32 bits on the CPU
  (`voxy_render::kernels::grain_seed`).
- **Formula:** grain cells of `size` layer pixels are counted from the
  layer's origin, so the pattern does not move with the preview resolution
  or the effect region. Each cell gets triangular noise
  `n = u₁ + u₂ − 1` in `[−1, 1]`, with `u₁`, `u₂` from a PCG integer hash of
  the cell, the channel and the base seed (bit-exact on every GPU).
  Monochrome uses one `n` for all channels. `rgb' = max(rgb + amount·n, 0)`.
- **Regions:** per pixel. Amount 0 changes nothing. Only an animated grain
  puts the frame in the cache key, and only seeded effects put the layer
  in it.

### Chromatic Aberration (`voxy.stylize.chromatic_aberration`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Amount (`amount`) | scalar, factor | 0–0.2 (slider 0–0.05) | 0.01 |
| Center (`center`) | point, layer px, on-canvas handle | any | centre of the layer's content |

- **Space:** working, premultiplied.
- **Formula:** with `k` the amount and `d = p − c` in texels, red comes
  from a bilinear sample at `c + d/(1 + k)` (red drawn `1 + k` larger),
  blue from `c + d/(1 − k)` (drawn `1 − k` smaller), green from the pixel
  itself. Alpha is the largest of the three samples' alphas. Samples
  outside the buffer are transparent.
- **Regions:** RoD = the input and the input scaled by `1 ± k` around `c`,
  padded by 1 px. ROI = the output and the output scaled by `1/(1 ± k)`,
  padded by 1 px. Amount 0 changes nothing.

### Drop Shadow (`voxy.stylize.drop_shadow`, v1) and Inner Shadow (`voxy.stylize.inner_shadow`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Shadow Color (`color`) | colour, no alpha | — | black |
| Opacity (`opacity`) | scalar, normalized | 0–1 | 0.5 |
| Direction (`direction`) | angle, degrees | any (slider ±180) | 45 |
| Distance (`distance`) | scalar, layer px | 0–1000 (slider 0–100) | 5 |
| Softness (`softness`) | scalar, layer px | 0–500 (slider 0–50) | 5 |
| Shadow Only (`shadow_only`, Drop Shadow only) | bool | — | off |

- **Space:** working, premultiplied.
- **Passes:** (1) the input's alpha moved by the offset
  `distance·(cos θ, sin θ)` (0° right, clockwise on screen; bilinear,
  transparent outside), (2) blurred like Gaussian Blur with radius
  Softness (skipped at 0), (3) combined with the effect's input, which is
  kept while the passes run.
- **Drop Shadow:** with `m` the moved, blurred alpha,
  `shadow = opacity·(color·m, m)` and the output is the input over the
  shadow, `in + shadow·(1 − in.a)`, or the shadow alone.
- **Inner Shadow:** `s = opacity·(1 − m)` and
  `rgb' = mix(rgb, color, s)` on the input's straight colour, keeping its
  alpha: the shadow shows inside the layer where the moved copy does not
  cover it.
- **Regions:** RoD = the input and the moved input padded by Softness + 1
  px (Inner Shadow needs the moved, blurred alpha beyond the layer too).
  ROI = the output and the output moved back, padded the same. Opacity 0
  changes nothing (unless Shadow Only is on).

### Glow (`voxy.stylize.glow`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Threshold (`threshold`) | scalar, linear light | 0–2 (slider 0–1) | 0.5 |
| Radius (`radius`) | scalar, layer px | 0–500 (slider 0–100) | 15 |
| Intensity (`intensity`) | scalar, factor | 0–10 (slider 0–4) | 1 |

- **Space:** linear light in both working-space modes (decoded before,
  re-encoded after), so it means the same thing in both.
- **Passes:** (1) the light above the threshold: with `lin` the linear
  straight colour and `L` its Rec. 709 luma,
  `g = lin·max(L − t, 0)/L·a` (keeps each colour's hue); (2) blurred like
  Gaussian Blur with the radius (skipped at 0); (3) added to the effect's
  kept input: `c = in_lin + intensity·g` (premultiplied linear). Outside
  the layer the light brings its own coverage,
  `a = in.a + min(max(g·intensity), 1)·(1 − in.a)`.
- **Regions:** RoD and ROI = padded by Radius + 1 px. Intensity 0 changes
  nothing.

### Stroke (`voxy.stylize.stroke`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Color (`color`) | colour, no alpha | — | white |
| Width (`width`) | scalar, layer px | 0–500 (slider 0–50) | 4 |
| Opacity (`opacity`) | scalar, normalized | 0–1 | 1 |
| Outline Only (`outline_only`) | bool | — | off |

- **Space:** working, premultiplied.
- **Passes:** (1) marks the pixels at least half opaque; (2) jump
  flooding: in steps from the power of two above `w + 1` down to 1, each
  texel keeps the nearest marked pixel known to itself or the eight texels
  one step away, stored as an offset (exact small integers in RGBA16F);
  (3) the line, coverage `clamp(w − d + 1, 0, 1)` with `d` the distance to
  the nearest marked pixel's centre (about half a pixel past the edge), so
  the line ends `w` pixels from the shape, with round corners.
- **Formula:** `line = opacity·cover·(color, 1)`, drawn behind the
  effect's kept input: `in + line·(1 − in.a)`; Outline Only keeps just
  `line·(1 − in.a)`. Jump flooding is approximate at a few rare pixels; the
  test checks the width on a straight edge exactly.
- **Regions:** RoD and ROI = padded by Width + 1 px. Width 0 changes
  nothing.

## Keying

### Chroma Key (`voxy.keying.chroma_key`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Key Color (`key_color`) | colour, no alpha | — | sRGB (0, 177, 64) |
| Tolerance (`tolerance`) | scalar, normalized | 0–1 | 0.15 |
| Edge Softness (`softness`) | scalar, normalized | 0–1 | 0.1 |
| Spill Suppression (`spill`) | scalar, normalized | 0–1 | 0.5 |

- **Space:** working, on straight colour.
- **Formula:** colours are compared in the Rec. 709 YCbCr chroma plane,
  `cc = ((B − Y)/1.8556, (R − Y)/1.5748)`, so brightness alone never keys a
  pixel out. With `d = |cc − cc_key|`, alpha is multiplied by
  `smoothstep(tol, tol + max(soft, 10⁻⁴), d)`. Spill: the component of
  `cc` along the key's chroma direction, where positive, is reduced by the
  spill amount; the colour is rebuilt from the original luma and the new
  chroma, clamped at 0.
- **Regions:** per pixel.

### Luma Key (`voxy.keying.luma_key`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Key Out (`mode`) | enum: Darker, Brighter | — | Darker |
| Threshold (`threshold`) | scalar, normalized | 0–1 | 0.1 |
| Edge Softness (`softness`) | scalar, normalized | 0–1 | 0.1 |

- **Space:** working; Rec. 709 luma `L` of the straight colour.
- **Formula:** Darker keeps `smoothstep(t, t + s, L)`, Brighter keeps
  `1 − smoothstep(t − s, t, L)` (with `s ≥ 10⁻⁴`); the premultiplied
  pixel is scaled by it.
- **Regions:** per pixel.

### Matte Choker (`voxy.keying.matte_choker`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Choke (`choke`) | scalar, layer px | −100–100 (slider ±20) | 1 |
| Softness (`softness`) | scalar, layer px | 0–100 (slider 0–20) | 1 |

- **Space:** working, premultiplied.
- **Passes:** a square erosion (positive choke) or dilation (negative), one
  pass per axis with `round(|choke|·s)` taps each side: shrinking scales
  each pixel by the smallest alpha around it (keeping its own colour);
  growing takes the largest of each premultiplied channel (new edge pixels
  take their neighbours' colour). Then a Gaussian blur with radius
  Softness (skipped at 0). The square shape gives square grown corners.
- **Regions:** RoD and ROI = padded by the growth (negative choke) plus
  Softness + 1 px.

## Distort

All distortions sample the input bilinearly at a mapped position
(transparent outside the buffer). Their regions always contain the input
region too, because one buffer holds every stage of a chain.

### Transform (`voxy.distort.transform`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Anchor Point (`anchor`) | point, layer px, on-canvas handle | any | centre of the layer's content |
| Position (`position`) | point, layer px, on-canvas handle | any | centre of the layer's content |
| Scale (`scale`) | 2D factor | −100–100 (slider 0–4) | (1, 1) |
| Rotation (`rotation`) | angle, degrees | any | 0 |
| Opacity (`opacity`) | scalar, normalized | 0–1 | 1 |

- **Formula:** the map `M(p) = position + R(rotation)·S(scale)·(p − anchor)`
  (computed in `f64`); each output pixel reads the input at `M⁻¹(p)`,
  times the opacity. A scale of 0 on either axis draws nothing.
- **Regions:** RoD = the input and the box of `M(input)`, padded by 1 px;
  ROI = the box of `M⁻¹(output)`, padded by 1 px. The defaults change
  nothing.

### Mirror (`voxy.distort.mirror`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Reflection Center (`center`) | point, layer px, on-canvas handle | any | centre of the layer's content |
| Reflection Angle (`angle`) | angle, degrees | any | 0 |

- **Formula:** with `n = (cos θ, sin θ)`, pixels with `(p − c)·n > 0` show
  the reflection `p − 2((p − c)·n)n`; the others stay. 0° mirrors the
  left side onto the right.
- **Regions:** the rectangle and its reflection, padded by 1 px.

### Wave (`voxy.distort.wave`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Amplitude (`amplitude`) | scalar, layer px | 0–500 (slider 0–100) | 10 |
| Wavelength (`wavelength`) | scalar, layer px | 1–5000 (slider 5–500) | 60 |
| Direction (`direction`) | angle, degrees | any | 0 |
| Phase (`phase`) | angle, degrees | any | 0 |

- **Formula:** with `d` the direction and `u` the distance along it from
  the layer's origin, a pixel reads the input at
  `p − A·sin(2π·u/λ + φ)·d⊥` (`d⊥` = `d` turned 90°). The phase is reduced
  modulo 360° in `f64` before reaching the GPU.
- **Regions:** padded by Amplitude + 1 px. Amplitude 0 changes nothing.

### Ripple (`voxy.distort.ripple`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Center (`center`) | point, layer px, on-canvas handle | any | centre of the layer's content |
| Amplitude (`amplitude`) | scalar, layer px | 0–500 (slider 0–50) | 4 |
| Wavelength (`wavelength`) | scalar, layer px | 1–5000 (slider 5–300) | 30 |
| Phase (`phase`) | angle, degrees | any | 0 |

- **Formula:** with `r = |p − c|`, a pixel reads the input at
  `p − A·sin(2π·r/λ − φ)·(p − c)/r`; increasing the phase moves the rings
  outwards.
- **Regions:** padded by Amplitude + 1 px. Amplitude 0 changes nothing.

### Bulge (`voxy.distort.bulge`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Center (`center`) | point, layer px, on-canvas handle | any | centre of the layer's content |
| Radius (`radius`) | scalar, layer px | 1–10 000 (slider 10–1000) | 100 |
| Amount (`amount`) | scalar, normalized | −1–1 | 0.5 |

- **Formula:** with `e = 2^amount` and `t = |p − c|/R < 1`, a pixel reads
  the input at `c + (p − c)·t^(e − 1)`, i.e. at distance `R·t^e`: positive
  amounts swell the centre, negative ones pinch it, and the circle's edge
  does not move.
- **Regions:** the rectangle, plus the circle's box when they meet.
  Amount 0 changes nothing.

### Tile (`voxy.distort.tile`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Tile Center (`center`) | point, layer px, on-canvas handle | any | centre of the layer's content |
| Tile Size (`tile_size`) | 2D factor of the layer's size | 0.01–10 (slider 0.05–2) | (0.5, 0.5) |
| Output Size (`output_size`) | 2D factor of the layer's size | 0.01–10 (slider 0.1–4) | (1, 1) |
| Mirror Edges (`mirror`) | bool | — | off |

- **Formula:** each tile shows the whole layer (its own content rectangle,
  so anything an earlier effect drew outside it is cut off), squeezed to
  the tile size. With `u = (p − center)/tile + ½`, a pixel reads the
  layer at `fract(u)`, flipped in every other tile with Mirror Edges.
  Samples stay half a texel inside the layer so tiles meet without seams.
  Outside the tiled area (the layer scaled by Output Size around its
  centre) everything is transparent.
- **Regions:** RoD = the input and the tiled area, padded by 1 px; ROI =
  the output and the layer, padded by 1 px. Region rules now receive the
  layer's own rectangle for this.

### Displacement Map (`voxy.distort.displacement_map`, v1)

| Parameter | Type | Range | Default |
|---|---|---|---|
| Displacement Map Layer (`map_layer`) | layer reference (see Layer inputs) | — | none |
| Use For Horizontal (`h_channel`) | enum: Red, Green, Blue, Alpha, Luminance | — | Red |
| Max Horizontal Displacement (`h_amount`) | scalar, layer px | −1000–1000 (slider ±100) | 10 |
| Use For Vertical (`v_channel`) | enum, as above | — | Green |
| Max Vertical Displacement (`v_amount`) | scalar, layer px | −1000–1000 (slider ±100) | 10 |

- **Formula:** with `m` the map's pixel at `p` (its straight channel value
  in `[0, 1]`, or Rec. 709 luma for Luminance, mixed towards the neutral
  0.5 by the map's coverage), the pixel reads the input at
  `p + 2·(m − 0.5)·amount` per axis: a bright map pulls pixels from the
  right (or below), so the image moves left (or up). No map, or a
  transparent one, changes nothing.
- **Regions:** padded by the amounts + 1 px.

## Transitions

Every transition (design §5) has:

| Parameter | Type | Range | Default |
|---|---|---|---|
| Progress (`progress`) | scalar, normalized | 0–1 | 0 |
| Auto (`auto`) | bool | — | on |
| Transition To (`layer_b`) | layer reference (see Layer inputs) | — | none |

- **Progress:** with Auto on, `t = (time − in)/(out − in)` over the layer's
  own duration (exact flicks, `EvaluatedEffect::layer_progress`); with it
  off, the Progress parameter (keyframable). Only then does time enter the
  chain's cache key.
- **B:** the Transition To layer, read as described under Layer inputs;
  none is transparency, so the transition takes its layer away.
- **Frame:** Slide and Zoom show nothing outside the layer's own rectangle.
- **Crossfade** (`voxy.transition.crossfade`, v1): `out = mix(A, B, t)`.
- **Wipe** (`voxy.transition.wipe`, v1; Angle, 0° left to right and
  clockwise; Feather 0–1000 layer px, default 20): the edge moves along the
  angle's direction `d` across the layer's extent along `d`, widened by the
  feather, and `out = mix(B, A, smoothstep(e − f/2, e + f/2, p·d))`.
- **Slide** (`voxy.transition.slide`, v1; Direction, 0° right and
  clockwise): A moves along `d` by `t` times the layer's extent along `d`,
  and B follows it in from behind, `out = A' + B'·(1 − A'.a)`.
- **Zoom** (`voxy.transition.zoom`, v1; Zoom 1–100, default 4): A is
  scaled by `1 + t·(zoom − 1)` around the layer's centre and mixed into B,
  `out = mix(B, A', 1 − t)`.
- **Regions:** RoD = the input; ROI = the output (and, for Slide and Zoom,
  the layer). Progress 0 changes nothing.

## Adjustment layers

An adjustment layer (design §1.5) applies its effect stack to the
composite of every layer below it, between its in and out points, over the
whole composition (its transform is ignored except opacity). The result
replaces what was below: `mix(below, result, opacity)` for Normal, and
`mix(below, B(below, result), opacity)` with another blend mode `B`
(premultiplied composite as in the blend-modes table). The composition's
background is not a layer: with adjustment layers the layers are
composited on transparency, the effects run on that, and the background
goes under the finished composite. An adjustment layer without enabled
effects does nothing.

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
