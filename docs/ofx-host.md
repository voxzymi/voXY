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

## Host properties (O-A, `voxy_ofx_host::host`)

- API version 1.5, name `org.voxy.voXY`, label `voXY`, version from the
  crate.
- Contexts: filter and generator so far; transition and general arrive in
  O-C. Pixel depths: float, short, byte (float preferred). Components:
  RGBA, RGB, Alpha.
- Multi-resolution, tiles and temporal clip access: yes. Overlays: no
  (interacts arrive in O-D). Multiple clip depths or pixel aspects,
  settable frame rate or fielding: no. Draft quality: yes.
- Native origin: bottom-left, as OpenFX images are.
- No GPU render (`OfxImageEffectPropOpenGLRenderSupported` is `"false"`,
  D5) and no colour management (`OfxImageEffectColourManagementNone`).
- Parameters: string, choice, string-choice and boolean animation yes
  (they hold between keys); custom animation and custom interacts no; no
  page limit beyond a flat list.

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
- **Parameter suite (V1).** Descriptor parameters get the descriptor
  properties their type has in the spec, with its defaults. An instance
  copies them; on the instance only the properties the spec marks
  "instance (read/write)" stay writable (labels, hint, enabled, secret,
  data pointer, evaluate-on-change, ranges, increment, digits, choice
  options, order and enum values, group open, icon). Where the spec
  leaves room:
  - Values are read and written only on instances; on a descriptor those
    calls, and `paramDefine` on an instance, get `kOfxStatErrBadHandle`.
  - `paramSetValue` on a parameter with keys sets a key at the instance's
    current time, otherwise the static value. `paramSetValueAtTime` on a
    parameter that does not animate gets `kOfxStatErrValue`.
  - A choice index outside its options, or a string choice not among its
    enum values, gets `kOfxStatErrValue`. Booleans are stored as 0 or 1.
  - Custom and Bytes values over 1 MiB get `kOfxStatErrMemory` (P-0 Q3).
    A Bytes default (an `OfxBytes *`) is copied when Describe ends.
  - Returned strings and byte buffers stay valid for the parameter's next
    16 reads.
  - Derivatives use a central difference over 1/1000 frame and integrals
    Simpson's rule over 64 steps, for double types only.
  - "A key at the indicated time" means within 1/10000 frame.
  - `paramDeleteAllKeys` keeps the value shown at the current time as the
    static value. `paramCopy` needs the same type, replaces the target's
    keys, and copies keys inside the range shifted by the offset.
  - `paramEditBegin`/`End` group changes into one undo step.
  - Group, page and push-button parameters have no value
    (`kOfxStatErrUnsupported`). **Parametric parameters are not supported
    yet**: `paramDefine` gives `kOfxStatErrUnsupported`.
- **Image effect suite (V1).** `clipDefine` works on descriptors only,
  and defining a name again returns the existing clip. `clipGetHandle`
  and `clipGetImage` work on instances only. Instance clip properties are
  read-only. `clipGetImage` returns the whole image the host has for that
  time (the region is a hint); no image, or an unconnected clip, gives
  `kOfxStatFailed`. Images are bottom-up, 64-byte aligned, with
  `kOfxImagePropData` at the bottom-left pixel. Float to 8 or 16 bits
  rounds half to even. Image memory may be allocated with a null
  instance; lock and unlock nest.
- **Memory suite (V1).** Blocks are 64-byte aligned. Freeing a pointer the
  suite did not hand out, or freeing twice, gives `kOfxStatErrBadHandle`.

## Actions (O-A, `voxy_ofx_host::suites::driver`)

The driver runs a plugin's actions. In O-A it runs only in the in-process
test harness; from O-B it runs inside the sandbox. The editor never calls
it.

- **Load:** the plugin must report `OfxImageEffectPluginAPI` version 1
  and have both entry points; then `setHost` and Load.
- **Describe**, then **DescribeInContext** for each context both the
  plugin and voXY support (others are skipped silently). A context
  descriptor starts from the Describe properties.
- **GetClipPreferences:** the host offers, per clip, the best depth both
  sides support (float first), RGBA or the clip's first supported layout,
  the project pixel aspect, premultiplied output, no fielding. The answers
  update the instance's clips.
- **GetRegionOfDefinition:** when the plugin replies default, filters
  get the source's region and generators the project's.
- **GetRegionsOfInterest:** the default for every input is the requested
  region.
- **IsIdentity:** a reply naming a clip means "use that clip at that
  time".
- **Render:** with field none, sequential and interactive status 0, full
  quality. Afterwards the output is made premultiplied (D2):
  unpremultiplied results are premultiplied, opaque ones get alpha 1.
- **Begin/EndSequenceRender**, and **InstanceChanged** for user edits,
  wrapped in Begin/EndInstanceChanged.
- **DestroyInstance** when an instance is dropped, **Unload** when the
  plugin is.

## Sandbox (O-B)

`voxy-ofx-sandbox --bundle <path> --channel <spec>` runs one bundle's
plugins. It connects to the editor's local socket (`ns:` names on Linux
and Windows, an `fs:` socket file in the session folder on macOS),
answers `Hello` with its protocol version and process ID, maps the shared
memory it is sent, and serves requests until `Unload` or until the channel
closes (then it unloads everything and exits 0).

- The binary is the bundle's `Contents/<arch>/NAME.ofx`, or a path to a
  binary. Plugins that fail Load or Describe are listed in the
  description's `failed` with the reason; the rest still work.
- For a render, each input slot is copied into an image the plugin reads,
  and the output image is copied back into its slot afterwards.
- A heartbeat thread bumps the counter in the mapping every 25 ms, and
  passes the editor's abort flag on to every instance.
- Parameter values come from the request's snapshot. A plugin asking for
  another time gets the snapshot value for now; asking the editor back
  comes in a later O-B step. Changes the plugin makes return as
  `ParamEdit`s with the action's reply.
- `OfxSetHost` (the optional binary-level export in 1.5) is not called
  yet.

## Editor side of a sandbox (O-B, `voxy_ofx_host::client`)

- **Launch:** the editor listens, starts the sandbox (found next to its
  own executable), waits up to 10 s for it to connect, checks the
  protocol version and hands it its mapping. A bundle that does not load
  fails the launch with the sandbox's reason.
- **Timeouts** (defaults, adjustable in Preferences later): Describe 10 s,
  CreateInstance 5 s, Render 30 s, everything else 5 s.
- **Crash, hang, protocol error:** the sandbox process is killed, the
  error names the action (and the exit status or signal for a crash), and
  every later request fails at once with it. The editor's process is never
  affected; tests run a crashing and a hanging plugin to prove it.
- **Blocklist:** a failure blocks the plugin for the session. After 3
  failures the block is persistent (kept in Preferences). Retry clears it
  and restarts the sandbox cleanly.
- Dropping a healthy sandbox sends Unload (0.5 s grace) and then ends the
  process.

## Plugins as voXY effects (O-B, `voxy_ofx_host::mapping`)

The filter context of each plugin becomes an effect definition
`ofx:<pluginIdentifier>`, version = the plugin's major version (a newer
minor version loads normally). Name: the plugin's label (or identifier);
category: its grouping (or "OpenFX"); description: its description.

| OFX parameter | voXY |
|---|---|
| Double, 2D, 3D | `Scalar`, `Vec2`, `Vec3`. Unit by double type: angle → degrees, scale → factor, X/Y types → pixels, normalised coordinates → normalised (sent as is), others none. |
| Integer, 2D, 3D | `Int`, or `Vec2`/`Vec3` shown as integers |
| Boolean | `Bool` |
| Choice | `Enum` with the option labels; a default past the options becomes 0 |
| StrChoice | `String`; the hint lists the allowed values |
| String | `String` |
| RGB / RGBA | `Color` (RGB hides alpha); components passed as they are |
| Custom | `String`, not animatable (hold-only) |
| Bytes | hidden `String`, base64 (standard alphabet, padded, strict) |
| Group | a collapsible group (open state from the plugin) |
| Page | flattened |
| PushButton | read-only for now; buttons come with the parameter UI step |
| Parametric | refused at define time (not supported yet) |

- Hard range from Min/Max unless they are the "no limit" defaults; the
  display range from DisplayMin/Max, kept inside the hard range.
- Animatable follows `kOfxParamPropAnimates`; secret parameters are
  hidden; labels and hints come from the plugin.

## Rendering (O-B, design §6.9)

An `ofx:` effect in a layer's chain is the renderer's `External` kernel.
The commands so far are submitted, the chain buffer (premultiplied, in the
working space, f16) is read back as floats, an `ExternalEffects` provider
(the OpenFX host) runs the plugin on it, and the result is written into
the next chain texture; GPU effects after it continue as usual.

- Without a provider, for a type it does not run, or when the plugin
  fails, the image passes through and the render report lists the effect
  with the reason (for the effect header's error badge).
- The region of definition stays the input's for now; the plugin's own
  GetRegionOfDefinition and GetRegionsOfInterest join in a later step.

## Still to be written (O-A to O-D)

Render scale and multi-resolution, thread-safety levels across
instances, timeouts and abort through the sandbox (O-B), the transition
and general contexts (O-C), interacts (O-D), parametric parameters.
