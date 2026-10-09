//! The lossless XML tree never panics, and whatever parses writes back
//! byte-for-byte.
#![no_main]

use libfuzzer_sys::fuzz_target;
use voxy_am_compat::xml::XmlDocument;

fuzz_target!(|data: &[u8]| {
    if let Ok(doc) = XmlDocument::parse(data) {
        assert_eq!(doc.to_bytes(), data);
        // Structural diff against itself is empty.
        assert!(voxy_am_compat::diff::diff(&doc, &doc).is_empty());
    }
});
