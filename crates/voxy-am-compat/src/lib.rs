//! Alight Motion interoperability for voXY.
//!
//! The format is learned only from sample files the project owner exports
//! from their own copy of the app, and every finding is documented in
//! `docs/am-format.md`. Nothing about the format is assumed in advance.
//!
//! The approved architecture (design proposal §7) has three layers:
//!
//! 1. [`xml`]: a lossless XML tree. Unedited files are written back
//!    byte-for-byte, and edits only regenerate the tags they touch.
//! 2. A container reader/writer (zip package or bare XML) that copies
//!    unmodified entries verbatim. Not written yet; whether it is needed at
//!    all depends on what the samples turn out to be.
//! 3. Typed structs over the tree, each keeping unrecognized attributes and
//!    children, plus data-driven mapping tables in `mappings/am/`.
//!
//! Layer 1 is format-agnostic and done. Layers 2 and 3 wait for sample
//! files; see `docs/am-format.md`.

pub mod xml;
