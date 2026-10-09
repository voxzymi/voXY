# Fuzz targets

Requires a nightly toolchain and `cargo install cargo-fuzz`:

```sh
cd fuzz
cargo +nightly fuzz run am_xml          # lossless XML tree
cargo +nightly fuzz run am_container    # zip/bare containers + inspection
cargo +nightly fuzz run voxy_project    # .voxy loader
```

The same properties also run on stable as `proptest` tests in CI
(`crates/voxy-am-compat/tests/robustness.rs`,
`crates/voxy-core/tests/history_props.rs`).
