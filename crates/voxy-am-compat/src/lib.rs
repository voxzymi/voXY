//! Alight Motion interoperability for voXY.
//!
//! Intentionally empty until Compat 2a. The schema is derived only from sample
//! files exported by the project owner from their own copy of the app, and is
//! documented in `docs/am-format.md` as it is discovered. Nothing about the
//! format is assumed in advance.
//!
//! Approved architecture (see `docs/design-proposal.md` §7):
//! 1. A lossless XML tree that writes unedited files back byte-for-byte.
//! 2. Typed structs on top, each keeping unrecognized attributes and children.
//! 3. Data-driven mapping tables in `mappings/am/`.
