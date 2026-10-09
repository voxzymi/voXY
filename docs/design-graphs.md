# Design: advanced parametric graphs

**Status: Milestones H-A (data model, evaluators, format, tests), H-B
(inspector, handles, numeric fields, live preview, thumbnails) and H-C
(step jumps in the value/speed graph, user presets, copy/paste, apply to many,
actions) implemented.** H-D (Alight Motion import and export) is described
here but not built.

This file is the specification the code in `crates/voxy-core/src/shapes.rs`
implements. Where this file and the code disagree, that is a bug in one of
them; the tests in `shapes.rs` and `tests/shapes.rs` pin the formulas below.

## 0. Starting point and a gap in the plan

The addendum extends "the custom keyframe graphs follow-up (G-A to G-D)" and
refers to "the per-segment curve representation from G-A". **That follow-up
was never built**: there is no `docs/design-graphs.md` before this one, no
G-A to G-D commits, and no user presets or value/speed toggle. What exists is
Milestone 2's model:

- a segment's timing is `Keyframe::interp` of its left key: `Hold`,
  `Linear` or `Bezier`; a Bézier segment takes `P1` from the left key's
  `ease_out` and `P2` from the right key's `ease_in`;
- the graph editor (Milestone 2c) edits those handles and previews the speed
  of Bézier segments.

**Decision taken (reversible):** rather than build G-A first, the five new
easings are new variants of `Interp`, each carrying its parameters:

```rust
pub enum Interp {
    Hold, Linear, Bezier,                       // unchanged
    Spring(SpringParams), Bounce(BounceParams), Oscillation(OscillationParams),
    Steps(StepsParams), SteppedRamp(SteppedRampParams),
}
```

This is the "easing kind per segment" the addendum describes, stored where
the segment's timing already lives, so every existing code path (commands,
undo, persistence, evaluation, the graph editor) already handles it. If G-A is
built later it can wrap or replace this enum; the evaluators in `shapes.rs`
do not depend on where the choice is stored. **Items from G-A to G-D that are
still missing and that H-C assumes** (value/speed toggle, user presets,
copy/paste of curves) are listed in §6.

## 1. Common contract

Every easing is a function `f(u)` of normalized segment time
`u = (t − t₀)/(t₁ − t₀)`, computed from integer flicks. The segment's value
is `mix(v₀, v₁, f(u))` for numbers, vectors, colors (linear light) and
integers (rounded); booleans, enums and strings hold `v₀`. For spatial
properties `f(u)` is the fraction of the motion path's arc length (§5).

- `f(0) = 0` and `f(1) = 1` **exactly**, for every parameter value. Any
  `u ≤ 0` or NaN gives 0, any `u ≥ 1` gives 1.
- Time is never reordered: `f` is a function of `u`, and `u` is monotonic in
  `t`. Values may leave `[0, 1]` (spring, oscillation) or jump (steps), time
  cannot.
- **Total and finite.** Parameters are sanitized on every evaluation
  (clamped into range; NaN or infinity becomes the default), so no input
  produces NaN or infinity. Files and edits are *validated* instead (§3):
  out-of-range values are rejected, never silently changed.
- **Analytic, not approximated.** There is no bézier fit anywhere in voXY's
  own pipeline. An approximation is only ever used if an export path
  requires it (H-D), and the export report will say so.

### Numeric determinism

Only `+ − × ÷`, `sqrt`, `floor` and the pure-Rust `libm` crate's `sin`, `cos`
and `exp` are used. `libm` is a software implementation written in plain
IEEE-754 operations, so it returns the same bits on every platform; `std`'s
`sin`/`cos`/`exp` call the system math library and are deliberately not used.
There is no fast-math, no FMA contraction (Rust does not fuse by default) and
no SIMD reassociation. `tests/fixtures/shapes_golden.txt` pins the exact bits
of ten parameterizations at 513 points each (value and speed). It was
generated on Linux x86-64.

**Result (CI run 37980901457, commit `32f1c94`, debug builds):** the pinned
table, `legacy_interp.txt` and the other `voxy-core` tests **passed on Windows
x64, macOS arm64 and macOS x64**, and on Linux x64, so the evaluators are
bit-identical on those four targets.

**Optimized builds (CI runs 37986352929 on `983bf87` and 37987980892 on
`c22967b`):** the `determinism-release` job runs the `voxy-core` suite,
including the pinned tables, with `--release`, and it **passed on Linux x64,
Windows x64, macOS arm64 and macOS x64**. So the tables are bit-identical in
optimized builds on those four targets too. The claim covers only those targets
and the Rust version CI used (1.99).

The golden-*image* tests (`parametric_easings_render_as_pinned`) are a
different matter: they need a GPU, which only the Linux job has (software
Vulkan, required there), so on Windows and macOS they are skipped and prove
nothing about rendering on those systems.
`tests/fixtures/legacy_interp.txt` does the same for hold, linear, Bézier and
spatial interpolation, and was generated **before** the new code was added, so
passing it proves legacy projects evaluate bit-for-bit as before.

## 2. The five easings

Notation: `u ∈ (0, 1)`, `c` cycles, `a` decay rate, and `ω = 2π·frequency`.

### 2.1 Spring

Closed-form damped oscillator released toward the target.

```
a = damping,  ω = 2π·frequency,  B = (a/ω)(1 − overshoot)
D(u) = e^(−a·u) · (cos ωu + B·sin ωu)
R    = D(1)
f(u) = 1 − D(u) + R·h(u),         h(u) = u²(3 − 2u)
```

| Parameter | Range | Default | Meaning |
|---|---|---|---|
| `frequency` | 0.5 – 12 | 2.5 | Oscillation cycles across the segment |
| `damping` | 1 – 24 | 7 | The envelope shrinks by `e^(−damping)` over the segment |
| `overshoot` | 0 – 2 | 0 | Extra launch energy; `0` releases the spring from rest |

- `1 − D(u)` solves `x″ + 2a x′ + (a² + ω²)x = a² + ω²` from `x(0) = 0`
  with initial speed `a·overshoot`. At `overshoot = 0` it starts at rest
  (`f′(0) = 0`) and the first peak is `1 + e^(−aπ/ω)` at `u = π/ω`.
- **The window.** `R·h(u)` cancels the part of the oscillation that has not
  died out by `u = 1`, so `f(1) = 1` exactly and `f` is continuous. Its size
  is at most `e^(−a)·√(1 + B²)`: about 0.1 % of the change at the defaults
  (where the first peak overshoots by about 25 %), and up to about 37 % at
  `damping = 1`. Low damping is allowed, but the end is then visibly
  "pinched" onto the target by the window.
- **Interpretation to confirm.** The addendum names three parameters:
  frequency, damping and "overshoot amount". For a physical spring released
  from rest, frequency and damping already fix the overshoot, so a third
  independent knob needs a third degree of freedom. voXY uses the launch
  speed for it. If Alight Motion's "overshoot" means something else
  (for example the peak height), say so and this will be re-parameterized
  before it is exposed in the UI.

### 2.2 Bounce

A ball dropped onto the target (the second key's value) from the first key's
value, bouncing to rest. Gravity is constant, so the whole curve follows from
the first rebound.

```
h = first_height,  r = restitution,  n = count
H_k = h·r^(2(k−1))                  rebound height of arc k = 1..n
d_k = 2·t₀·√h·r^(k−1)               duration of arc k
t₀  = 1 / (1 + 2√h · Σ_{k=1..n} r^(k−1))     fall time, so total time = 1
c₀ = t₀,  c_k = c_{k−1} + d_k,  c_n := 1      contact times
fall   (u < c₀):          f = (u / t₀)²
arc k  (c_{k−1} ≤ u < c_k), q = (u − c_{k−1})/d_k:   f = 1 − 4·H_k·q·(1 − q)
```

| Parameter | Range | Default | Meaning |
|---|---|---|---|
| `count` | 1 – 12 (integer) | 3 | Rebound arcs after the first landing |
| `first_height` | 0.02 – 1 | 0.25 | First rebound height as a fraction of the drop |
| `restitution` | 0.1 – 0.9 | 0.5 | Speed kept per bounce; arc heights shrink by `r²`, durations by `r` |

The defaults are the familiar "ease-out bounce" (contacts at 1/2.75, 2/2.75,
2.5/2.75, 1). `f` stays inside `[0, 1]`; at each contact `f = 1` and the speed
reverses (a corner, not a jump). "Decay per bounce" in the addendum is exposed
as `restitution` (higher = bounces last longer); the UI may label it
"Bounciness".

### 2.3 Oscillation

A wave between the two levels, riding on a ramp so it cannot miss the target.

```
w(u) = ½ − ½·cos(2π(cycles·u + phase))           raw wave, between 0 and 1
g(u) = w(u) − (1 − u)·w(0) − u·w(1)               w minus its end-to-end ramp
f(u) = u + amplitude · e^(−decay·u) · g(u)
```

| Parameter | Range | Default | Meaning |
|---|---|---|---|
| `cycles` | 0.25 – 20 | 1.5 | Wave cycles across the segment |
| `amplitude` | 0 – 2 | 1 | `1` swings between the two levels |
| `phase` | 0 – 1 | 0 | Start phase, in cycles |
| `decay` | 0 – 12 | 0 | Envelope rate; `0` keeps the full amplitude |

**It always ends exactly on `f(1) = 1`** (and starts on 0): `g(0) = g(1) = 0`
by construction, for every parameter value. With `amplitude = 1`, `decay = 0`,
`phase = 0` and a half-integer `cycles` (0.5, 1.5, …) `f` is exactly the pure
wave `w` between the two levels. `amplitude = 0` is linear. Because the end
condition is enforced by the ramp term, a wave whose cycle count is not a
half-integer is the pure wave plus a linear correction; that is deliberate.

### 2.4 Steps

```
n = count,  k(u) = number of j ∈ 1..n with j/n ≤ u      (0 ≤ k ≤ n − 1)
end:   f = k/n            start: f = (k + 1)/n
both:  f = (k + 1)/(n+1)  none:  f = k/(n − 1)             for 0 < u < 1
```

`f(0) = 0` and `f(1) = 1` always. `k(u)` is defined by *comparison against the
correctly rounded `j/n`*, not by `floor(n·u)`, so jump instants are identical
on every platform.

| Parameter | Range | Default | Meaning |
|---|---|---|---|
| `count` | 1 – 64 (integer) | 4 | Intervals (`none` uses at least 2) |
| `jump` | start, end, both, none | end | As CSS `steps()`: `jump-start`, `jump-end`, `jump-both`, `jump-none` |
| `hold` | 0 – 0.9 | 0 | Fraction of the segment at its end that holds the final value |

`hold` squeezes the staircase into `u ∈ [0, 1 − hold]` (use `u/(1 − hold)`)
and holds `f = 1` afterward. `none` is an addition beyond the addendum's
"start, end, or both", included because it is part of CSS `steps()` and costs
nothing.

### 2.5 Stepped ramp

```
f(u) = ramp·u + (1 − ramp)·staircase(u; count, jump)
```

| Parameter | Range | Default | Meaning |
|---|---|---|---|
| `count` | 1 – 64 | 4 | Intervals |
| `ramp` | 0 – 1 | 0.5 | `0` = pure steps, `1` = exactly linear |
| `jump` | start, end, both, none | end | As above |

Each plateau climbs with slope `ramp`; each jump has height
`(1 − ramp)·(unit jump)`. There is no `hold` (the addendum lists none).

## 3. Data model, validation and format

- Types: `SpringParams`, `BounceParams`, `OscillationParams`, `StepsParams`,
  `SteppedRampParams`, `JumpMode` (in `shapes`, re-exported from the crate
  root). All are `Copy`; `Interp` is `Copy + PartialEq` (it is no longer `Eq`
  or `Hash`: it holds floats).
- **Validation.** Ranges above are enforced when an `Interp` is
  deserialized, when a `KeyframeTrack` is built (`TrackError::InvalidEase`),
  and when a command sets one (`SetInterpolation`, `Property::insert_keyframe`).
  Out-of-range is an error with a message naming the parameter. The UI uses
  `Interp::sanitized()` (clamp) before issuing an edit. The evaluator
  sanitizes too, as a last line of defence.
- **JSON** (`.voxy` format 0.6.0): `interp` is a string for the legacy modes
  (unchanged) and a single-key object for the new ones, with missing
  parameters taking their defaults:

  ```json
  "interp": { "spring": { "frequency": 2.5, "damping": 7.0, "overshoot": 0.0 } }
  "interp": { "steps": { "count": 4, "jump": "end", "hold": 0.0 } }
  ```

  Keys: `spring`, `bounce`, `oscillation`, `steps`, `stepped_ramp`.
- **Versioning.** A project is written as **0.6.0 only if a keyframe uses one
  of these easings**; every other project is still written as 0.5.0 and
  opens in older builds unchanged. An older voXY that meets a 0.6.0 file
  refuses it with the existing "newer than this build supports" message and
  leaves the file untouched; it never guesses or half-loads. (Showing a
  warning and opening read-only is not possible for builds that already
  shipped; it would need that behavior in the old build.) Ease handles on the
  keys are ignored for parametric segments but kept in the file, so switching
  back to Bézier restores them.
- Commands: no new command was needed. `SetInterpolation` already carries an
  `Interp`, so applying, changing and re-parameterizing an easing is an
  ordinary undoable command; consecutive parameter edits from a handle drag
  will use the same merge mechanism as `SetEase` (H-B).

## 4. Handles (H-B; **provisional**, unconfirmed)

The addendum's reading of the promotional image: a yellow *main* handle and a
white *secondary* handle per graph. voXY has no access to the app, so this
mapping is **provisional: it has not been checked against Alight Motion**, and
the handle tooltips say so while `HANDLE_MAPPING_PROVISIONAL` is `true`. All
of it lives in one function, `handle_layout` in
`crates/voxy-ui/src/ease_params.rs`; changing the mapping means editing that
function and this table, nothing else.

| Type | Main handle (yellow) | Secondary handle (white) |
|---|---|---|
| Spring | x → damping (log scale), y → overshoot | x → frequency |
| Bounce | x → restitution, y → first height | x → count (snaps to integers) |
| Oscillation | x → decay, y → amplitude | x → cycles |
| Steps | x → count (snaps) | x → hold |
| Stepped ramp | x → count (snaps) | y → ramp |

Handles live in the graph's unit square: a mapped axis follows its parameter
over the parameter's whole range (damping on a log scale, counts snap to whole
numbers); an unmapped axis rests at a fixed place (main handles on the top
edge, secondary handles just below the baseline, the ramp handle on the right
edge). Every parameter also has a numeric field and a tooltip, and the curve
is redrawn live while a handle or field is dragged. A handle drag is one undo
step (one history transaction re-derived from the drag's starting easing on
every frame); field edits merge through `SetEasing`'s merge key. Parameters
are clamped to the ranges above, so `f(0) = 0` and `f(1) = 1` cannot be
broken (tested for every handle position).

**UI wording.** Labels follow what the parameters do, not just their names:

- Spring `overshoot` is labelled **"Overshoot (launch energy)"**, with a
  tooltip saying so. Beside it the editor shows a read-only **"Peak
  overshoot"** percentage: how far the first swing goes past the target,
  `SpringParams::peak_overshoot()`. It is a result of the three parameters,
  not a fourth one. If Alight Motion's overshoot knob turns out to mean peak
  height, the parameterization will be revisited.
- Bounce `restitution` is labelled **"Bounciness"** (higher = longer-lasting
  bounces).
- Steps and stepped ramp offer four jump positions. **"No end jumps (extra)"**
  (CSS `jump-none`) is an addition beyond the five graph types and is marked as
  such in the selector and its tooltip.

## 5. Discontinuities and display rules

Only Steps and Stepped ramp are discontinuous. `Interp::shape_jumps()` /
`KeyframeTrack::segment_jumps(i)` return the jumps as `(at, height)`, where
`at ∈ [0, 1]` is segment time; `at = 0` is a jump just after the left key and
`at = 1` one at the right key. `segment_slope(i, u)` returns the speed on the
continuous parts (0 on step plateaus, `ramp` on ramp plateaus).

- **Value graph.** Plateaus and ramps are drawn as the curve. Each jump is a
  thin dashed connector, not part of the curve, with a dot at both ends;
  hovering shows the height in the property's units.
- **Speed graph.** The speed is infinite at a jump, so no curve passes
  through it: the plateau speed is drawn as a line and every jump is a marker
  (a vertical tick with an "∞" cap) at its time, labelled with the height on
  hover. Spring, bounce and oscillation speed are ordinary curves; a bounce
  contact is a sign flip of the speed (a visible vertical step in the curve,
  finite).
- **Motion paths and spatial interpolation.** `f(u)` is the fraction of arc
  length, so a stepped easing makes the layer hop between points *on the same
  path*: the path itself is unchanged, the position holds on each plateau and
  jumps along the curve. A spring or oscillation that leaves `[0, 1]`
  continues past the path's ends along the end tangent at the same speed (the
  existing rule for overshooting Bézier eases). Bounce stays on the path.
- **Discrete value types.** Integers are interpolated then rounded; booleans,
  enums and strings hold the first key's value until the next key, whatever
  the easing.
- **At a key.** Evaluating exactly at a key returns that key's value
  bit-for-bit, so a "jump at start" staircase shows the left key's value at
  that single instant and the first step from the next instant on.

### 5.1 Step jumps in the value and speed graphs (implemented)

`graph_editor::jump_windows` finds each jump (`KeyframeTrack::segment_jumps`)
and brackets it with two flick-level samples a couple of flicks apart, so the
plateaus end exactly where they do. A test checks, for every jump mode and a
range of counts, that the value really steps by the jump's height across its
window and is flat on both sides.

- **Value graph:** the curve is drawn as separate runs, never joined across a
  jump; a thin dashed connector bridges each jump, with a hover tooltip giving
  the change per component (`Step jump: +45°`).
- **Speed graph:** samples within a quarter frame of a jump are left out (a
  central difference across a jump is a meaningless spike) and the vertical
  scale ignores them. Each jump gets an **∞** marker and a faint vertical line
  instead of a curve; plateaus show their real speed (0 on steps, `ramp × Δ`
  on a stepped ramp), and a caption explains the symbol. The speed strip in
  the ease inspector marks jumps the same way.

### 5.2 Copy, paste, presets and apply to many (implemented)

- **`SegmentEase`** (`voxy-core`): an easing as a value, the left key's
  `Interp` plus the two Bézier handles. It is what copy and paste carry and what
  a preset stores. Handles are only written when the easing is Bézier, so
  applying a spring never destroys a target segment's own curve.
- **`ApplyEasing`** (command): writes one `SegmentEase` onto many segments,
  across properties, **as one undo step**; the last key of a track starts no
  segment and is skipped; it fails without changes for a missing key or invalid
  parameters, or if there was nothing to write to.
- **Which segments:** in the graph editor, presets, thumbnails and Paste apply
  to *all selected segments* if the inspected segment is part of a selection of
  more than one, and to just that segment otherwise; the panel says so. "Apply
  to N" gives all selected segments the inspected segment's easing.
- **Copy** keeps the easing on an in-session clipboard and also puts its JSON on
  the system clipboard (for bug reports). Pasting reads the in-session
  clipboard only.
- **User presets** ("My easings" in the inspector) are saved by name in the
  per-user preferences file (`easing_presets`, TOML), not in the project:
  embedding presets in projects is H-D. Names are trimmed and unique (saving
  replaces); right-click deletes. An entry that does not parse (hand-edited, or
  from a newer build with an easing this build does not know) is dropped on its
  own and never costs the other presets or preferences.
- **Actions** (rebindable in the shortcut editor, no default shortcuts):
  `ease.copy`, `ease.paste`, `ease.apply_spring`, `ease.apply_bounce`,
  `ease.apply_oscillation`, `ease.apply_steps`, `ease.apply_stepped_ramp`, in
  the new **Keyframe** menu. They need a selected keyframe that starts a
  segment and say why when they cannot run.

## 6. Remaining milestones

- **H-B (done)** — segment inspector in the graph editor's ease panel: a type
  selector (`Interp::convert_to` keeps compatible parameters: Steps ↔ Stepped
  ramp share `count` and `jump`; Spring ↔ Oscillation map `frequency ↔
  cycles` and `damping ↔ decay`, clamped), numeric fields driven by one table
  (`ease_params::desc`, ranges taken from `voxy-core`), the two handles (§4,
  provisional), live preview, a speed strip that marks step jumps with ∞
  instead of drawing through them, and a strip of five thumbnails drawn from
  the easings themselves (original artwork). New undoable command
  `SetEasing`. Checked by running the app headlessly: a handle drag edits the
  easing live and leaves one "Edit Easing" undo step.
- **H-C (done)** — scoped to what the five types need. The Value/Speed
  toggle already existed (`GraphMode`, Milestone 2c); what was missing was
  discontinuities, now handled (§5.1). User presets and copy/paste did not
  exist and were built (§5.2). Full G-D (presets embedded in projects, Alight
  Motion curve import and export) stays in H-D.
- **H-D** — Alight Motion import and export, fidelity tests (SSIM/PSNR),
  polish. Blocked on samples; see `docs/am-format.md`.

Golden-image tests exist (`crates/voxy-render/tests/golden.rs`,
`parametric_easings_render_as_pinned`): a square sliding and turning under
each easing, five instants per easing, one reference PNG each in
`tests/golden/render/easing_*.png`, plus a check that frames render
identically twice. Not yet done: the Alight Motion import fuzz target (H-D).
