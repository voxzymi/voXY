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
//! 2. [`container`]: a zip package or a bare file. Unedited packages are
//!    written back byte-for-byte; edited ones copy untouched entries
//!    verbatim.
//! 3. Typed structs over the tree, each keeping unrecognized attributes and
//!    children, plus data-driven mapping tables in `mappings/am/`.
//!
//! Layers 1 and 2 are format-agnostic and done, together with the sample
//! tooling used to learn the format: [`inspect`] (what a file contains),
//! [`inventory`] (every element, attribute and value shape seen) and
//! [`diff`] (what changed between two exports). Layer 3 waits for sample
//! files; see `docs/am-format.md`.

pub mod container;
pub mod diff;
pub mod inspect;
pub mod inventory;
pub mod xml;
