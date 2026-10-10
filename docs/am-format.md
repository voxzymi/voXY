# Alight Motion format notes (living document)

> **ON HOLD BY DECISION.** Alight Motion compatibility is not being worked on:
> the project owner dropped it (no import, no export, no easing-graph
> mapping, no second sample batch, no fidelity tests against reference
> renders). **Do not pick it up by accident.** Nothing here is deleted so the
> work can resume if the decision changes: the `voxy-am-compat` crate (lossless
> container, inspection, inventory and diff tooling), this document and the
> sample requests below all stay as they are. The sample requests are
> **withdrawn**: nothing is being waited for. Only the owner can lift the hold.

**Status: no samples received yet. Nothing below is known about the format.**

This document records what voXY learns about Alight Motion's project
(`.alightmotion`), preset and element XML formats. The rules are:

- **Every finding comes from sample files** the project owner exports from
  their own copy of the app. Nothing is guessed from other sources.
- **Each finding is marked** as one of:
  - **confirmed**: a single setting was changed in the app, the file was
    re-exported, the XML was diffed, and the change matched;
  - **inferred**: observed in samples but not yet isolated by a diff.
- **Each finding lists the samples that show it.**
- **Ambiguities become sample requests** to the owner (see the section at the
  end).
- **No format version is listed as supported until the owner confirms** that
  files voXY writes open in the app without errors.

**Status: the tooling is ready (Compat 2a, format-agnostic part). Format
work waits for the first sample batch below and for the owner's review of
the app's terms of service.**

## Tooling

Everything below is format-agnostic. It assumes nothing about Alight
Motion's vocabulary and exists so that every finding comes from the files
themselves.

| Step | Command | What it gives |
|---|---|---|
| Inspect | `cargo xtask am-inspect tests/am-samples/* --out inspect.md` | Per file: zip or not, every entry with its size, compression method and content type, and whether voXY writes it back byte-for-byte. Then an inventory of every element path, attribute, value shape (integer, decimal, hex, …) and example values, with the samples each appears in. |
| Diff | `cargo xtask am-diff before.alightmotion after.alightmotion` | Every attribute, text and element change between two exports of the same project. This is the differential analysis that turns an *inferred* finding into a *confirmed* one. |
| Round trip | `cargo test -p voxy-am-compat --test samples` | Fails if any sample does not re-export byte-identically. |

Library pieces (`crates/voxy-am-compat`):
- `xml`: the lossless tree.
- `container`: zip or bare files; unedited packages write back
  byte-for-byte, and edited ones copy untouched entries verbatim. Archives
  are bounded (entry count, decompressed size) because samples are
  untrusted input.
- `inspect`: what a file contains.
- `inventory`: elements, attributes and value shapes.
- `diff`: structural changes between two exports.

Fuzz targets live in `fuzz/`. Matching `proptest` checks run in CI.

## Samples received

| ID | File | Exported from (app version, OS) | Content | Matching render |
|---|---|---|---|---|
| — | — | — | — | — |

**Requested first batch.** Export each from the same app version, put
them in `tests/am-samples/`, and note the app version and OS:

| # | File | What to make in the app |
|---|---|---|
| 1 | `01-empty` | A new, empty project with default settings. |
| 2 | `02-empty-other-size` | The same, but a different canvas size, frame rate and background color. Diffed against #1, this pins down the project settings. |
| 3 | `03-one-solid` | One solid or rectangle layer, nothing animated. |
| 4 | `04-solid-moved` | #3 with the layer moved, scaled, rotated and set to 50% opacity (no keyframes). |
| 5 | `05-position-2keys` | #3 with two position keyframes (0 s and 2 s), default easing. |
| 6 | `06-position-2keys-linear` | #5 with the easing changed to linear. |
| 7 | `07-position-2keys-custom` | #5 with a custom curve from the graph editor. |
| 8 | `08-rotation-opacity-keys` | Keyframes on rotation and opacity. |
| 9 | `09-hold` | A hold (no interpolation) keyframe, if the app has one. |
| 10 | `10-image-layer` | One imported image layer. |
| 11 | presets | One exported preset and one element XML, if available. |

For each, a rendered **video export** with the same name (e.g.
`05-position-2keys.mp4`) for the fidelity tests. Each pair isolates one area, which is what makes the diffs
conclusive:
- #1/#2 differ only in project settings;
- #3/#4 only in static transform values;
- #5/#6 and #5/#7 only in easing.

If one change per export is practical, splitting #4 into move-only,
scale-only, rotate-only and opacity-only exports is even better.

## Parametric graphs: spring, bounce, oscillation, steps, stepped ramp

**Nothing here is confirmed.** voXY has no sample of these graph types. The
only source so far is the project owner's reading of a promotional image of
five graph shapes, which is **not** the app's documentation and is recorded
here only as a list of questions, not as findings. voXY's own definitions of
the five easings (`docs/design-graphs.md`) were built independently of the
app's format.

| Question | Why it matters | Status |
|---|---|---|
| Are these graph types stored as a type identifier plus parameters, or as sampled or bézier-approximated curves? | Decides whether import can map to voXY's analytic types or must keep the data verbatim and approximate. | Unknown |
| Identifier strings and parameter names for each type | Needed to map import and to write export. | Unknown |
| Parameter encoding and normalization (units, ranges, whether 0..1) | voXY's parameters (frequency in cycles, damping as decay rate, …) may not match the app's. | Unknown |
| Does the app's "overshoot" (spring) mean peak height, launch energy, or something else? | voXY currently uses launch energy (`design-graphs.md` §2.1). | Unknown |
| What do the two handles (main and secondary) control, per type? | The handle mapping in `design-graphs.md` §4 is a proposal only. | Unknown |
| How does each type behave at its ends (does it hit the target exactly)? | voXY guarantees `f(0)=0`, `f(1)=1`. | Unknown |
| Step jump positions and the "hold on last value" behavior | voXY uses CSS `steps()` semantics. | Unknown |

Until these are answered from samples: **import keeps the original data
verbatim** (the existing lossless rule), renders an approximation, and
reports it; **export writes what it can represent and reports what it cannot**.
If an approximation (for example a sampled or bézier fit) turns out to be the
only way to export a type, the export report will say so explicitly.

**Requested sample batch for these graphs (second batch, "H" files).** One
export pass, in this order, so every file differs from its neighbor in exactly
one thing. **(Withdrawn: Alight Motion compatibility is on hold by decision, see
the note at the top. Kept for reference only; nothing is waiting on these.)**

### The one scene every file uses

Same app version and OS as the first batch. Make this once and save it as
`H00`; every other file is `H00` with the easing changed and *nothing else*.

- Composition 1280×720, 30 fps, 3 s, background black.
- One solid layer, 600×200 px (not a square: a square looks identical at 0°
  and 90°), a mid-gray color, centered, anchor in the center.
- **Rotation** keyframed: **0° at 0.5 s** and **60° at 2.5 s**, two keyframes
  only. (Rotation does not clamp, so overshoot stays visible; opacity would
  clip it.) Nothing else animated.
- The graph under test is applied to **that segment** (the 0.5 s to 2.5 s one).
  Say in the notes which keyframe(s) carry it if the app stores it on one or
  both.

For **every** file send:

1. the project export (`.alightmotion`), unchanged;
2. the **rendered video** of it with the same name (`.mp4`, 1280×720, 30 fps,
   the app's highest quality, no audio): the fidelity reference;
3. one line of notes: the parameter values *as the app displays them*, and
   which control you changed (a screenshot of the graph editor is welcome and
   is the only way I can check the handle colors and roles).

### Tier 1: required (the minimum that identifies the encoding)

| File | What to make |
|---|---|
| `H00-baseline-linear` | The scene with the keyframes' normal interpolation, no special graph (linear if the app has it, else the app's default; say which). |
| `H01-spring-default` | Spring graph, every parameter at its default. |
| `H02-bounce-default` | Bounce graph, defaults. |
| `H03-oscillation-default` | Oscillation graph, defaults. |
| `H04-steps-default` | Steps graph, defaults. |
| `H05-stepped-ramp-default` | Stepped-ramp graph, defaults. |
| `H1x-spring-<param>` | `H01` with **exactly one** spring parameter changed, one file per parameter the app exposes. Change it to a clearly different value (about double or half, not a tiny nudge) and note the exact number. |
| `H2x-bounce-<param>` | Same for bounce, one file per parameter. |
| `H3x-oscillation-<param>` | Same for oscillation, one file per parameter. |
| `H4x-steps-<param>` | Same for steps, one file per parameter; a parameter that is a choice (jump position, for example) gets **one file per non-default option**. |
| `H5x-stepped-ramp-<param>` | Same for stepped ramp, including each non-default option. |

Name `<param>` after what the app calls it (`H12-spring-damping`). If a type
has *N* parameters, that is *N* files, so with four to five parameters a type
Tier 1 is roughly 25 to 30 files.

### Tier 2: strongly useful (gives ranges and scaling)

For each Tier 1 parameter file, a second one named `…-max` or `…-min` with the
parameter dragged to the **app's extreme** (the largest or smallest value it
allows), same notes. Two values per parameter, plus the default, are what let
me tell a linear scale from a log one and find the units.

### Tier 3: handles (checks the provisional handle mapping)

For each of the five types, two files, with the default graph and **only one
handle moved**:

| File | What to make |
|---|---|
| `H6x-<type>-main-handle` | Only the **main** (yellow) handle dragged to a clearly different spot. Note which direction you dragged and the numbers the app shows before and after. |
| `H7x-<type>-secondary-handle` | Only the **secondary** (white) handle dragged. Same notes. |

If a type has only one handle, say so instead of making the second file.

### Also, if the app makes it easy

- An exported **preset or element XML** of a layer carrying one of these
  graphs, and a **share-link/clipboard payload** of the same (the lossless
  path in `docs/am-format.md` needs to see how a graph travels outside a whole
  project).
- A note of the app **version string and OS** once, at the top of the notes.

Put everything in `tests/am-samples/` (or send it any way you like and I will
place it). Whether they are committed is your call; the folder's README
explains that the tests skip when it is empty.

## Container

*Unknown.* For each sample, check: is it a zip? What does it contain? What
encoding does the XML use? Is there embedded media?

## Elements and attributes

| Element / attribute | Values seen | Samples | Appears to control | Status |
|---|---|---|---|---|
| — | — | — | — | — |

## Enumerations

| Enum (where it appears) | Value | Samples | Meaning | Status |
|---|---|---|---|---|
| — | — | — | — | — |

## Version fields

*Unknown.*

## Effects seen

These feed the generated effect coverage report.

| Effect identifier | Samples | Parameters | voXY mapping | Status |
|---|---|---|---|---|
| — | — | — | — | — |

## Open questions and sample requests

*None yet.*
