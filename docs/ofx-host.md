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

## Still to be written (O-A to O-D)

Supported pixel depths and conversions, render scale, field order, pixel
aspect, colour management (none advertised), thread-safety levels, the
supported properties and suites, contexts, timeouts and abort.
