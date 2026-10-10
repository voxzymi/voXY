# Free third-party OpenFX plugins: research and shortlist

**Status: shortlist approved by the owner (answers in §4).** Nothing is
built yet: fixtures are first built in O-A and O-B. The research read
license files, READMEs, build files and plugin source code on the upstream
sites, and git tree metadata, which gives submodule pins without checking
out any source. All of it was done on 2026-10-10.

The rules are those of `docs/design-effects-ofx.md` §7:

- Permissive licenses can be built in CI.
- Copyleft licenses need an explicit owner decision before any use.
- Non-commercial, unclear and commercial licenses are rejected.
- Plugin source is never copied into voXY's tree.
- Every plugin is pinned to a commit and checksummed.
- Binaries built in CI are test fixtures only.

---

## 1. Sources examined

| Source | Upstream | Pin (newest at research time) | License, from the license file | Notes |
|---|---|---|---|---|
| OpenFX `Examples/` (C API) | <https://github.com/AcademySoftwareFoundation/openfx> | tag `OFX_Release_1.5.1`, commit `ab779510b2655b4d11a7e01e5c521f9aa8c88976` (the same pin as the headers) | **BSD-3-Clause**. `LICENSE.md`: "Copyright (c) 2025, OpenFX and contributors to the OpenFX project". Every file says `SPDX-License-Identifier: BSD-3-Clause`. | The examples use the raw C API and a shared header, `ofxUtilities.H`. Each one builds from a single `.cpp`. |
| OpenFX `Support/Plugins/` (C++ Support library) | same | same | **BSD-3-Clause**. `Support/LICENSE`: "Copyright OpenFX and contributors to the OpenFX project". | These use the C++ Support library from the same repository (BSD-3). The upstream CMake links `opengl::opengl` for every Support plugin. |
| Natron `openfx-misc` | <https://github.com/NatronGitHub/openfx-misc> | `master` at `0abd46b5a8cbc98fa24579042129460d0aa87b8f` (2025-04-29). Submodules: `openfx` (Natron fork) at `2303ff811bee3ffe085287602f684fe5fe5357e0`, `SupportExt` at `2485505dbe55885ee92ca9f164e7732a85856a13`. | **GPL-2.0-or-later**. `LICENSE` is GPL v2, and the README and file headers say "either version 2 of the License, or (at your option) any later version". "(C) 2018-2021 The Natron Developers, (C) 2013-2018 INRIA". | More than 100 plugins in one `Misc.ofx.bundle`. A `nomulti` build target makes one bundle per plugin. Builds against Natron's fork of the OpenFX headers with Natron extensions turned on. |
| Natron `openfx-misc` CImg plugins | same, `CImg/` | same, plus `CImg.h` fetched at build time at `b33dcc8f9f1acf1f276ded92c04f8231f6c23fcd` (`CIMGVERSION` in `CImg/Makefile`) | Plugin code: **GPL-2.0-or-later**. `CImg.h`: dual **CeCILL-C** or **CeCILL v2** (CImg README). | The only blur in Natron's set that is not motion- or direction-based. |
| `SupportExt` | <https://github.com/NatronGitHub/openfx-supportext> | `2485505…` (as above) | **GPL-2.0** (`LICENSE`) | A build dependency of `openfx-misc`. |
| Natron `openfx-io` | <https://github.com/NatronGitHub/openfx-io> | `master` at `1bce1f1a2ad05c793682dd6845e62e5b352a3ea6` | **GPL-2.0** (`LICENSE`) | Readers and writers that need OpenImageIO, FFmpeg and OpenColorIO. |
| Natron `openfx-arena` | <https://github.com/NatronGitHub/openfx-arena> | `master` at `49e4d5fc197a637196c1a7decab7f97751b867f6` | **GPL-2.0-or-later** (`LICENSE`, `COPYING`) | Needs ImageMagick, Pango, librsvg, poppler and more. The README warns that the plugins use OFX features some hosts lack. |
| Natron `openfx-gmic` | <https://github.com/NatronGitHub/openfx-gmic> | `master` at `292ac55c7c84f6eff56c1764a5a5037b96895d12` | Dual **CeCILL-C** or **CeCILL v2** (`COPYING`) | Needs the G'MIC library. |
| `gyroflow-ofx` | <https://github.com/gyroflow/gyroflow-ofx> | `HEAD` `c2fed29c2aa7892178e89b58b160797298d4657c` | **GPL-3.0** (`LICENSE`) | Video stabilisation from gyro data. Needs GPU, lens profiles and its own data files. |
| `BaldavengerOFX` | <https://github.com/baldavenger/BaldavengerOFX> | `HEAD` `3360e980f29772a8644cb28538af81e95d01034f` | **GPL-3.0** (`LICENSE`) | Plugins written for DaVinci Resolve, mostly on its CUDA, OpenCL and Metal paths. |
| `houdini-ofx-plugins` | <https://github.com/zhangxiao6776/houdini-ofx-plugins> | `HEAD` `dddaae29cbe1993f7ed50a029d6d7335bbdea85e` (2026-04-07) | `LICENSE` says BSD-3-Clause, **but its copyright line reads "OpenFX and contributors to the OpenFX Project"**, not the author. | One author's collection for Houdini. Its depth and normal estimation needs ONNX Runtime and model files. |
| `ofx-rs` example | <https://github.com/itadinanta/ofx-rs> | `HEAD` `83da40fd552ece5238ac056bc9216219eeec5e97` | Apache-2.0 on crates.io; the repository has no `LICENSE` file | A Rust translation of OpenFX `Examples/Basic`. The README says "01/2019 status: Not ready for production use". |

The web searches turned up no other freely redistributable OpenFX
image-effect plugin set, and in particular no MIT or Apache plugin
collection. Every general-purpose effect set found is under the GPL, or
depends on CeCILL code.

---

## 2. Candidates

**Abbreviations.** The Contexts column uses F (filter), G (generator),
T (transition), Gen (general) and P (paint, unused by voXY). "Builds" gives
the four CI platforms in order: Windows x64, macOS arm64, macOS x64,
Linux x64.

**Build status.** OpenFX's own CI builds its Examples and Support plugins on
Linux, Windows and macOS-latest (arm64). macOS x64 is expected to work but
is not in their CI. Natron ships binaries for all three operating systems,
but how it builds them (MSYS2 on Windows, universal binaries on macOS)
differs from our CI. **So no plugin's status is "verified" until O-B builds
it.**

### 2.1 OpenFX samples (BSD-3-Clause)

| # | Plugin (identifier) | What it does | Contexts | Suites and params it needs | Builds | GPU or extra libs | Verdict |
|---|---|---|---|---|---|---|---|
| 1 | Examples/Invert | Inverts RGB | F | property and image effect suites only; no params; **8-bit only** | expected on all 4 | none | **Fixture.** The simplest possible plugin. It exercises the host's 8-bit conversion. |
| 2 | Examples/Basic (gain) | Scales RGBA | F, Gen | Double (×6), Boolean, Group, Page; 8/16/32-bit | expected on all 4 | none | **Fixture.** Basic filter with parameter groups and pages. |
| 3 | Examples/Rectangle | Draws a rectangle | F, G, Gen | Double2D (×2) as points, RGBA; all depths | expected on all 4 | none | **Fixture.** Generator plus 2D point parameters. |
| 4 | Examples/ChoiceParams | Choice demo | F, Gen | Choice (×2), StrChoice (1.5), Page | expected on all 4 | none | **Fixture.** Choice parameters. |
| 5 | Examples/DepthConverter | Converts bit depth | F, G, Gen | Choice; GetClipPreferences (output depth) | expected on all 4 | none | **Fixture.** Clip preferences and depth handling. |
| 6 | Examples/Test (`testProperties`) | Logs every host property and suite it can fetch | F | every host suite | expected on all 4 | none | **Fixture (diagnostic).** Checks what the host advertises. Not a render golden. |
| 7 | Support/Invert | Inverts RGBA | F | multi-thread suite (C++ Support library) | expected on all 4 | links OpenGL (system library; not used by Invert) | **Fixture.** The same effect as #1, but through the C++ Support layer, which most real plugins use. |
| 8 | Support/Generator (noise) | Noise generator | G, Gen | Double, Page | expected on all 4 | links OpenGL | **Fixture.** Generator context. |
| 9 | Support/Transition (crossFade) | Cross-fade | T, Gen | Double (the Transition param); two clips | expected on all 4 | links OpenGL | **Fixture (O-C).** Transition context. |
| 10 | Support/Tester | Host feature tester | F | PushButton (×2), Double2D, Choice, Boolean, Double; overlay interact | expected on all 4 | links OpenGL; interact (O-D) | **Fixture.** Push buttons and `InstanceChanged`. |
| 11 | Support/MultiBundle | Two plugins in one binary | F | `OfxGetNumberOfPlugins` > 1 | expected on all 4 | links OpenGL | **Fixture.** Discovery and registry with several plugins per bundle. |
| 12 | Examples/Custom | Custom param demo | F | Custom, **Bytes (new in 1.5)**, PushButton (×2); overlay interact (OpenGL) | expected on all 4 | OpenGL | **Fixture in O-D**, when interacts arrive. Raises the Bytes question (§4, Q3). |
| — | Examples/Overlay, Examples/DrawSuite | Overlay interacts | F | interact and draw suites | — | OpenGL | **Later (O-D)**, with the interact suite. |
| — | Support/GPUGain, Examples/OpenGL | GPU render suites | F | CUDA, Metal, OpenCL, OpenGL | — | GPU | **Reject**: GPU suites are out of scope (D5). |
| — | Examples/ColourSpace | Colour-management demo | F | colour-management suite (1.5) | — | cimg, spdlog | **Reject for now.** voXY advertises no colour management (design §6.5). |
| — | Support/Retimer | Retimer context | Retimer | — | — | — | **Reject.** voXY does not offer the retimer context. |
| — | Support/Field | Field rendering | F | field properties | — | — | **Reject.** voXY is progressive only (design §6.5). |

**What the BSD set covers:** a basic filter (#1, #2, #7), a generator (#3,
#8), a choice parameter with a group and page layout (#2, #4), a 2D point
(#3), push buttons (#10), Custom and string parameters (#12), clip
preferences (#5), transitions (#9) and several plugins per bundle (#11).

**What it does not cover:** a blur, a transform and a keyer. No permissive
plugin for these exists upstream.

### 2.2 Natron `openfx-misc` (GPL-2.0-or-later): rejected by the owner (Q2)

These plugins would close the gaps. Each one would be built on its own with
the `nomulti` target.

| # | Plugin (identifier) | What it does | Contexts | Params | Extra licenses | Verdict |
|---|---|---|---|---|---|---|
| 13 | CImgBlur (`net.sf.cimg.CImgBlur`) | Gaussian and quasi-gaussian blur | F, Gen | Double, Choice, Boolean | CImg: CeCILL-C or CeCILL v2 | **Rejected (owner, Q2): no GPL plugins.** Was proposed as a copyleft fixture: Blur, and ROI growth. |
| 14 | TransformOFX (`net.sf.openfx.TransformPlugin`) | Translate, rotate, scale, skew | F, Gen | Double, Double2D, Boolean, Choice; transform interact | SupportExt: GPL-2.0 | **Rejected (owner, Q2): no GPL plugins.** Was proposed as a copyleft fixture: Transform, RoD and ROI. |
| 15 | ChromaKeyerOFX (`net.sf.openfx.ChromaKeyerPlugin`) | Chroma key | F, Gen | RGB, Double (×4), Choice (×3), Boolean, Page | — | **Rejected (owner, Q2): no GPL plugins.** Was proposed as a copyleft fixture: Keyer. |
| 16 | ColorCorrectOFX (`net.sf.openfx.ColorCorrectPlugin`) | Saturation, contrast, gamma, gain, offset | F, Gen | Group, Page, RGBA, RGBAScale, Double2D, Choice, **Parametric** | — | **Rejected (owner, Q2): no GPL plugins.** Was proposed as a copyleft fixture: A real colour corrector, and it exercises the "unsupported param type shows read-only" path. |
| — | Every other `openfx-misc` plugin, plus `openfx-io`, `openfx-arena`, `openfx-gmic` | — | — | — | GPL or CeCILL, heavy dependencies | **Reject.** Not needed for coverage. They need large libraries (OIIO, FFmpeg, ImageMagick, G'MIC). |

### 2.3 Other projects

| Project | Verdict |
|---|---|
| `gyroflow-ofx` | **Reject.** GPL-3.0, GPU-centred, needs lens data. Not a simple fixture. |
| `BaldavengerOFX` | **Reject.** GPL-3.0, built mainly on DaVinci Resolve's GPU paths. |
| `houdini-ofx-plugins` | **Reject for now.** The license file's copyright line names "OpenFX and contributors", not the author, so it is unclear who grants the license. The interesting plugins also need ONNX Runtime and model files. It could be reconsidered if the author clarifies. |
| `ofx-rs` example | **Reject.** Unmaintained since 2019, no license file in the repository, and a duplicate of OpenFX `Examples/Basic`. |
| Commercial plugins | Never included (design §6.10). |

---

## 3. Proposed shortlist

The **BSD-3 core (#1 to #11)** is 11 plugins, all from one pinned OpenFX
commit and the simplest first. They go in this order:

1. O-A (Describe and mapping): #1, #2, #4, #6, #11.
2. O-B (CPU rendering): #1, #2, #7, #5, #3, #8.
3. O-C: #9 (transition), #10 (push button).
4. O-D: #12 and the interact examples.

**Optional copyleft additions (#13 to #16)** are the only way to cover a
blur, a transform and a keyer with third-party plugins. **Alternative:**
we write three small test plugins of our own (blur, transform, chroma key),
like the crash and hang plugins, and skip the GPL set entirely.

### Distribution implications of #13 to #16 (GPL-2.0-or-later)

| Use | Implication |
|---|---|
| Built and run in CI only, as separate plugin files loaded by `voxy-ofx-sandbox` | Nothing is linked into voXY, and voXY's license is unaffected. Running GPL software has no conditions attached. The GPL's obligations start with **distribution**. If the binaries stay in the Actions cache (which only this repository's workflows can read) and are **not uploaded as downloadable artifacts or release assets**, nothing is distributed. |
| Uploaded as CI artifacts, or offered to users through "Get free plugins" | That is distribution of GPL binaries. Under GPL-2.0 §3 voXY would have to ship the complete corresponding source (`openfx-misc`, `SupportExt`, the Natron `openfx` fork and `CImg.h` at the pinned commits) next to each binary, or give a written offer valid for three years. Merely pointing at upstream is not enough for a project that distributes the binaries itself. CImg's CeCILL-C terms are GPL-compatible but add their own notice requirements. **I recommend against offering them to users** unless the owner decides otherwise later. |
| voXY's own license and binaries | Unaffected in every case. The plugins run in a separate process over IPC and are never linked, as the GPL FAQ's guidance on separate programs describes. voXY must not ship them in its installer. |

### Optional user download ("Get free plugins")

Recommendation: **not in this round.** The BSD samples are API
demonstrations, not useful effects. The useful set is GPL, and offering it
means voXY becomes a GPL distributor (above). The decision can be revisited
after O-C.

---

## 4. Questions for the owner

| # | Question | Recommendation |
|---|---|---|
| Q1 | Approve the BSD-3 core (#1 to #11) as CI test fixtures, built from OpenFX 1.5.1 (`ab77951`)? | Yes. |
| Q2 | For blur, transform and keyer: (a) use `openfx-misc` #13 to #16 as **CI-only** fixtures, never uploaded or distributed; (b) write our own three small test plugins; or (c) both? | **(b)**, our own plugins: no copyleft near the project, and simpler to debug. (a) is acceptable under the CI-only rule if you want third-party coverage of the Support Extensions and Natron-style plugins. |
| Q3 | OpenFX 1.5 adds a **Bytes** parameter type (binary data), used by #12. Persist it as base64 in a hidden `String` property (plugin state, like Custom), or show it read-only and not save it? | Persist it like Custom. That widens the approved "Custom parameters only" rule to "Custom and Bytes". |
| Q4 | "Get free plugins" download: skip for now? | Skip; revisit after O-C. |

---

### Owner's answers

- **Q1: yes.** The 11 BSD-3 OpenFX samples (#1 to #11) are CI test
  fixtures, pinned to `ab77951` like the headers. They are built in CI
  only and never shipped to users.
- **Q2: (b).** We write three small test plugins of our own (blur,
  transform, chroma key), licensed with the project. **No GPL plugins
  anywhere:** not in CI, not as artifacts, not offered to users. Using
  Natron's plugins would be a separate decision by the owner. Entries #13
  to #16 are therefore **rejected**.
- **Q3: yes.** Bytes parameters are stored as hidden base64 text, like
  Custom parameters. Values are capped at 1 MiB and validated on load
  (design §6.8).
- **Q4: skip** the "Get free plugins" download. It is revisited after O-C.

---

## 5. Compatibility table

Filled in from O-A onwards. It has one row per plugin and platform, with a
status (works, works with notes, host bug fixed, plugin bug or known issue,
fails) and notes. Empty until a plugin is built.

| Plugin | Platform | Status | Notes |
|---|---|---|---|
| — | — | — | — |
