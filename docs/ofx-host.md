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

## Suites (O-A, `voxy_ofx_host::suites`)

- `fetchSuite` returns null for any suite or version not implemented,
  including the GPU and OpenGL render suites (D5), and for a host handle
  that is not the host's.
- Every handle a plugin passes is checked against the live-handle registry
  first. A stale, null or made-up handle gets `kOfxStatErrBadHandle`; it
  is never dereferenced. A panic inside a suite call is caught and
  reported as `kOfxStatErrUnknown`; it never unwinds into the plugin.
- **Property suite (V1).** Where the spec does not say:
  - Getting or setting with the wrong type gives `kOfxStatErrValue`.
  - Setting a property only the host may set gives `kOfxStatErrValue`.
  - Setting past the end of a variable-dimension property grows it, and
    `propSet*N` makes it exactly `count` long. Fixed dimensions refuse
    indices outside them (`kOfxStatErrBadIndex`).
  - A null name, value or out pointer gives `kOfxStatErrValue`.
  - A returned string points at the stored value and stays valid until
    that property changes.
- **Message suite (V1, V2; one struct serves both).** The printf-style
  arguments are formatted by voXY's own C99 formatter (the Windows runtime
  exports no `vsnprintf`). `%n` is ignored, never written through.
  `long double` (`%L…`), `%a` and unknown conversions end the text with
  `…`, because their argument size is unknown. A null handle is allowed
  for transient messages. A question gets `kOfxStatFailed`: the sandbox
  has no user to ask, and the spec makes questions an error outside an
  interactive session. Persistent messages accept error, warning and
  message types only.
- **Progress suite (V1, V2).** An update returns `kOfxStatReplyNo` once
  the host asks the effect to abort. Fractions are clamped to 0–1.
- **Time line suite (V1).** The plugin sees the time and bounds the host
  sets. `gotoTime` gets `kOfxStatFailed`: a plugin in the sandbox cannot
  move the editor's playhead.
- **Multi-thread suite (V1).** `multiThread` runs `min(nThreads, CPUs)`
  threads (all CPUs for 0), each index once, and refuses a call from a
  spawned thread with `kOfxStatErrExists`. Mutexes are recursive;
  `mutexCreate` with a lock count of n is held n times by the creating
  thread; unlocking a mutex the thread does not hold gives
  `kOfxStatFailed`.
- **Memory suite (V1).** Blocks are 64-byte aligned. Freeing a pointer the
  suite did not hand out, or freeing twice, gives `kOfxStatErrBadHandle`.

## Still to be written (O-A to O-D)

Supported pixel depths and conversions, render scale, field order, pixel
aspect, colour management (none advertised), thread-safety levels, the
supported properties and suites, contexts, timeouts and abort.
