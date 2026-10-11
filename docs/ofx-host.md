# voXY OpenFX host: decisions

**Status: started. It records the owner-approved host decisions now, and
the full list (each checked against the OpenFX 1.5.1 specification text)
grows from O-A onwards.** The design is in
[`design-effects-ofx.md`](design-effects-ofx.md) §6.

Many commercial plugins expect a specific host and may not behave correctly
in voXY. voXY does not bundle any commercial plugin.

## Premultiplication (owner decision D2)

- voXY's working buffers are premultiplied. **Input clips are always handed
  to plugins premultiplied** (`kOfxImagePreMultiplied`). A plugin cannot
  choose its input state under the spec, so the host does not guess.
- Clips the host knows to be opaque are declared `kOfxImageOpaque`.
- The host honours the **output** premultiplication the plugin declares in
  GetClipPreferences. If it reports `kOfxImageUnPreMultiplied`, the host
  premultiplies the result on return (`rgb × a`). For `kOfxImageOpaque`,
  alpha is set to 1.

## Persisted plugin state (owner decisions, design §6.8 and P-0 Q3)

OpenFX has no general state-blob API. The only plugin state voXY saves is
the values of **Custom and Bytes parameters**:

- Custom: `Value::String`.
- Bytes (OpenFX 1.5): base64 in a hidden `Value::String` property.
- Both are hold-only, and capped at **1 MiB per value** (decoded size for
  Bytes). A larger value set by a plugin is refused with `kOfxStatErrMemory`
  and a warning.
- On load, a value that is over the cap, not valid base64 or of the wrong
  type resets only that parameter to its default, with a warning. Nothing
  panics.

## Discovery (O-A, `voxy_ofx_host::discovery`)

Checked against OpenFX 1.5.1, "Packaging OFX Plug-ins".

- **Search order:** `OFX_PLUGIN_PATH` (split on `;` on Windows and macOS,
  `:` elsewhere), then the standard folder, then the user's folders from
  Preferences.
  - Windows: `%CommonProgramFiles%\OFX\Plugins` (the folder
    `CSIDL_PROGRAM_FILES_COMMON` names), then the deprecated
    `C:\Program Files\Common Files\OFX\Plugins`.
  - macOS: `/Library/OFX/Plugins`. Linux: `/usr/OFX/Plugins`.
- **Host choices:**
  - Empty list entries are ignored.
  - A folder listed twice is searched once, at its first place.
    Comparison ignores case on Windows and macOS, and a trailing separator.
  - The scan goes at most 16 folders deep, visits each real folder once
    (links cannot loop), and sorts names so the order is stable.
  - A bundle inside a bundle belongs to the outer one and is not a plugin.
  - A bundle with no binary for this platform is listed without one, so
    the UI can say why it is missing.
- **Skipped:** any folder or bundle whose name starts with `@`.
- **Binary:** `NAME.ofx.bundle/Contents/<arch>/NAME.ofx`, with `<arch>`
  `Win64`, `Linux-x86-64` or `MacOS` (universal; OpenFX 1.5 has no arm64
  folder). Intel Macs fall back to the deprecated `MacOS-x86-64`. For arm64
  Windows and Linux the host uses the spec's informational names,
  `Win-arm64` and `Linux-aarch64`; voXY does not build for them yet.
- Discovery never loads a binary. Loading and Describe happen only in the
  sandbox (design §6.3).

## Still to be written (O-A to O-D)

Supported pixel depths and conversions, render scale, field order, pixel
aspect, colour management (none advertised), thread-safety levels, the
supported properties and suites, contexts, timeouts and abort.
