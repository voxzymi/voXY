# Alight Motion format notes (living document)

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

**Requested sample batch for these graphs.** Same app version and OS as the
first batch, each with the matching rendered video for the fidelity tests.
Use a single scalar property (opacity or rotation) from 0 to 100 over 2 s so
the curve is easy to read, and change nothing else.

| # | File | What to make in the app |
|---|---|---|
| 12 | `12-ease-spring-default` | Two keyframes with the spring graph at its default parameters. |
| 13 | `13-ease-spring-<param>` | #12 with exactly one spring parameter changed, one file per parameter (name what you changed in the file name). |
| 14 | `14-ease-bounce-default` | The same with the bounce graph; then one file per changed parameter. |
| 15 | `15-ease-oscillation-default` | The oscillation graph; then one file per changed parameter. |
| 16 | `16-ease-steps-default` | The steps graph; then one file per changed parameter, including each jump position. |
| 17 | `17-ease-stepped-ramp-default` | The stepped-ramp graph; then one file per changed parameter. |
| 18 | `18-ease-handles` | For each type, one export with only the main handle dragged and one with only the secondary handle dragged, and a note of which numbers the app shows. |

Also useful: a screenshot of each graph's editor with its handles, so the
yellow/white handle roles can be confirmed against the real UI.

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
