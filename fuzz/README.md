# Fuzz targets

Requires a nightly toolchain and `cargo install cargo-fuzz`:

```sh
cd fuzz
cargo +nightly fuzz run am_xml          # lossless XML tree
cargo +nightly fuzz run am_container    # zip/bare containers + inspection
cargo +nightly fuzz run voxy_project    # .voxy loader
cargo +nightly fuzz run ease_json       # easing parameters (spring, bounce, ...) and tracks
cargo +nightly fuzz run ofx_frame       # sandbox protocol frames (OpenFX host)
```

The same properties also run on stable as `proptest` tests in CI
(`crates/voxy-am-compat/tests/robustness.rs`, `crates/voxy-ofx-host/tests/protocol.rs`,
`crates/voxy-core/tests/history_props.rs`, `crates/voxy-core/tests/shapes.rs`).

Alight Motion import of the parametric graph types gets its own fuzz target
with Milestone H-D, once their encoding is confirmed from samples.
