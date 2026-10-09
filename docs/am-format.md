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

Work on this starts at Compat 2a, after Milestone 2. Before that, the owner is
reviewing the app's terms of service.

## Samples received

| ID | File | Exported from (app version, OS) | Content | Matching render |
|---|---|---|---|---|
| — | — | — | — | — |

Requested first batch:

1. An empty project.
2. One project per feature, for example:
   - a single solid,
   - one shape,
   - one keyframed position,
   - each easing type,
   - one text layer.
3. A few complex projects.
4. Preset and element XML exports, and any clipboard or share-link payloads.
5. A rendered video export of each sample, to use as visual ground truth.

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
