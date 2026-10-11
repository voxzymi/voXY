# voXY licensing

This document records voXY's licensing obligations and the policy every
dependency must meet. The full, generated list of Rust dependencies and their
licenses is in [`docs/third-party-licenses.md`](docs/third-party-licenses.md).
It is regenerated with `cargo xtask licenses`, and CI fails if it is stale.
The policy itself is enforced by `cargo deny` using [`deny.toml`](deny.toml).

## voXY's own license

**Not chosen yet; this is the project owner's decision.** Until it is
chosen, voXY's crates carry no `license` field, and `cargo deny` ignores them
as private crates. The dependency policy below keeps every common choice open,
whether a permissive license, a copyleft license, or proprietary
distribution.

## Policy for Rust crates

Rust crates are statically linked into the `voxy` executable, so they must be
under permissive licenses. The allow-list in `deny.toml` is:

MIT, Apache-2.0 (optionally WITH LLVM-exception), BSD-2-Clause, BSD-3-Clause,
ISC, Zlib, BSL-1.0, 0BSD, Unlicense, Unicode-3.0 and WTFPL, plus OFL-1.1 and
Ubuntu-font-1.0 for the fonts egui bundles for the UI.

WTFPL covers only the Rust bindings to FFmpeg (`ffmpeg-next`,
`ffmpeg-sys-next`). It is maximally permissive. FFmpeg itself is a separate
LGPL library; see the FFmpeg section below.

If a crate is offered under several licenses (`A OR B`), voXY uses it under
one of the permissive options. Concretely:

- `Apache-2.0 OR GPL-2.0-only` is used under Apache-2.0.
- `MIT OR Apache-2.0 OR LGPL-2.1-or-later` is used under MIT.

Copyleft-only crates are not allowed. Some crates are banned outright:

| Crate | License | Why it is banned |
|---|---|---|
| `x264`, `x264-sys`, `x265` | GPL | voXY ships an LGPL FFmpeg build |
| `gifski` | AGPL | Copyleft |
| `imagequant` | GPL-3.0 or commercial | Copyleft |

The generated list covers every target platform, Windows, macOS and Linux,
because Cargo's lockfile does.

### Fonts

egui's default UI fonts are bundled in the `epaint_default_fonts` crate:

- Ubuntu Light, under the Ubuntu Font Licence 1.0;
- Noto Emoji and emoji-icon-font, under OFL-1.1 and MIT/Apache-2.0.

They are used only for the editor's own interface. Text layers will use system
fonts through `parley`/`fontique`, starting in Milestone 4. No Alight Motion
fonts are used or distributed.

## OpenFX

voXY implements an OpenFX **host** against the official OpenFX C headers. The
headers are licensed under **BSD-3-Clause**.

- From Milestone 7 they will be vendored, pinned to a release tag, in
  `third_party/openfx/` together with their license file.
- Bindings are generated from them with `bindgen`.
- BSD-3-Clause requires keeping the copyright notice and license text in
  source and binary distributions. The installer's third-party notices and
  the About dialog will include it.
- BSD-3-Clause also forbids using the contributors' names to endorse voXY.
- It is compatible with every Rust license in the allow-list above.

Third-party OFX **plugins** are separate programs that the user installs. voXY
loads them at run time in a separate sandbox process and does not distribute
them, so their licenses do not affect voXY's.

- Plugins used as **CI test fixtures** are built from source at a pinned
  commit in CI. They are never shipped, uploaded as artifacts or linked into
  voXY. Copyleft plugins need the project owner's explicit approval before
  any use. See [`docs/ofx-plugins.md`](docs/ofx-plugins.md).
- Offering plugins to users as a download is a separate decision by the
  owner, and it is not made yet.

### Credits and notices

<!-- credits:begin (generated from third_party/credits.toml; do not edit by hand) -->
**OpenFX (OFX) image-effect API**: <https://github.com/AcademySoftwareFoundation/openfx>,
tag `OFX_Release_1.5.1`, commit `ab779510b2655b4d11a7e01e5c521f9aa8c88976`.
License: BSD-3-Clause ([full text](https://github.com/AcademySoftwareFoundation/openfx/blob/ab779510b2655b4d11a7e01e5c521f9aa8c88976/LICENSE.md)),
"Copyright (c) 2025, OpenFX and contributors to the OpenFX project". Use:
voXY's host implements this API. The headers are vendored in
`third_party/openfx/` with this license file.

**OpenFX example plugins (Invert, Basic, Rectangle, ChoiceParams, DepthConverter, Test; Custom in O-D)**: <https://github.com/AcademySoftwareFoundation/openfx/tree/ab779510b2655b4d11a7e01e5c521f9aa8c88976/Examples>,
tag `OFX_Release_1.5.1`, commit `ab779510b2655b4d11a7e01e5c521f9aa8c88976`.
License: BSD-3-Clause ([full text](https://github.com/AcademySoftwareFoundation/openfx/blob/ab779510b2655b4d11a7e01e5c521f9aa8c88976/LICENSE.md)),
"Copyright (c) 2025, OpenFX and contributors to the OpenFX project". Use:
built from source in CI as test fixtures only. They are never
shipped, uploaded or linked into voXY, so no binary-distribution
notice obligation arises.

**OpenFX Support library sample plugins (Invert, Generator, Transition, Tester, MultiBundle)**: <https://github.com/AcademySoftwareFoundation/openfx/tree/ab779510b2655b4d11a7e01e5c521f9aa8c88976/Support/Plugins>,
tag `OFX_Release_1.5.1`, commit `ab779510b2655b4d11a7e01e5c521f9aa8c88976`.
License: BSD-3-Clause ([full text](https://github.com/AcademySoftwareFoundation/openfx/blob/ab779510b2655b4d11a7e01e5c521f9aa8c88976/Support/LICENSE)),
"Copyright OpenFX and contributors to the OpenFX project". Use:
built from source in CI as test fixtures only. They are never
shipped, uploaded or linked into voXY, so no binary-distribution
notice obligation arises.

voXY uses no copyleft (GPL, LGPL, AGPL) plugins anywhere. voXY offers no plugins for download.
<!-- credits:end -->

## FFmpeg (from Milestone 3)

voXY reaches FFmpeg through the `ffmpeg-next` bindings (WTFPL), which link
the system's or the bundled FFmpeg shared libraries at run time. Development
builds may use whatever FFmpeg the OS provides. Ubuntu's, for example, is a
GPL build; that is fine for local development, but it is not what voXY
distributes.

voXY uses FFmpeg for video and audio decoding and encoding, with these
constraints:

- **LGPL build only.** The FFmpeg that voXY ships is configured without
  `--enable-gpl` and without `--enable-nonfree`. That makes it
  LGPL-2.1-or-later.
- **Dynamic linking.** voXY links to FFmpeg's shared libraries (`.dll`, `.so`
  or `.dylib`) and does not link it statically. Users can replace those
  libraries with their own build.
- **Distribution duties.** Installers ship the LGPL license text, state which
  FFmpeg version and configure flags were used, and provide the matching
  FFmpeg source, or a written offer for it, as LGPL-2.1 §6 requires.
- **No GPL encoders bundled.** `libx264` and `libx265` are GPL, so they cannot
  be part of an LGPL build.
  - If a user supplies their own GPL-enabled FFmpeg, voXY will use those
    encoders automatically, because the library is loaded dynamically.
  - That build is the user's own, and voXY does not distribute it.

### Encoders voXY ships

| Format | First choice | Software fallback |
|---|---|---|
| H.264 | Platform hardware encoders: NVENC, Intel QSV, AMD AMF, Apple VideoToolbox, Windows Media Foundation | **OpenH264** (Cisco, BSD-2-Clause) |
| H.265/HEVC | Platform hardware encoders | **kvazaar** (BSD-3-Clause), where available |
| GIF | FFmpeg's `palettegen`/`paletteuse` filters (LGPL) | — |
| PNG sequence, stills | Pure Rust `image`/`png` crates | — |

### Patent licensing is the distributor's responsibility

H.264/AVC and H.265/HEVC are covered by patents, administered for example
through the Via LA and Access Advance pools. **Patent licensing is the
responsibility of whoever distributes voXY binaries, or binaries that bundle
these encoders or decoders.** voXY's copyright licenses and the licenses
above do not grant any patent rights.

- Cisco pays the royalties for OpenH264 only when its prebuilt binary is
  downloaded from Cisco at run time. Building OpenH264 from source carries no
  such coverage.
- Hardware encoders are licensed through the hardware or OS vendor. That
  coverage may not extend to the application that calls them.
- Distributors should get legal advice for their own jurisdictions and
  distribution channels.

## Alight Motion

voXY reads and writes Alight Motion project, preset and element files purely
for interoperability.

- No Alight Motion code, assets, icons, fonts, name or logo are copied or
  distributed.
- The format is learned only from files the project owner exports from their
  own copy of the app, and the findings are documented in
  [`docs/am-format.md`](docs/am-format.md).
- The owner is reviewing the app's terms of service before that work
  (Compat 2a) starts.
