//! Malformed input never panics, and anything that parses writes back
//! byte-for-byte.

use proptest::prelude::*;
use voxy_am_compat::container::Container;
use voxy_am_compat::xml::XmlDocument;

/// Strings built from XML-significant characters, so a useful share parse.
fn xmlish() -> impl Strategy<Value = String> {
    let atoms = prop_oneof![
        Just("<a>".to_owned()),
        Just("</a>".to_owned()),
        Just("<b x=\"1\">".to_owned()),
        Just("</b>".to_owned()),
        Just("<c/>".to_owned()),
        Just("<d  y = '2' z=\"&amp;\" />".to_owned()),
        Just("<!-- c -->".to_owned()),
        Just("<![CDATA[ <x> ]]>".to_owned()),
        Just("<?pi data?>".to_owned()),
        Just("&lt;".to_owned()),
        Just("&#169;".to_owned()),
        Just("\r\n".to_owned()),
        Just("\t".to_owned()),
        Just(" text ".to_owned()),
        Just("ü".to_owned()),
        "[<>/=\"'&;!?a-c \n-]{1,4}",
    ];
    prop::collection::vec(atoms, 0..24).prop_map(|v| v.concat())
}

proptest! {
    #[test]
    fn parsed_documents_round_trip_exactly(s in xmlish()) {
        if let Ok(doc) = XmlDocument::parse_str(&s) {
            prop_assert_eq!(doc.to_string_lossless(), s);
        }
    }

    #[test]
    fn arbitrary_bytes_never_panic(bytes in prop::collection::vec(any::<u8>(), 0..256)) {
        let _ = XmlDocument::parse(&bytes);
        let _ = Container::open(bytes);
    }

    #[test]
    fn well_formed_nesting_always_parses(depth in 0usize..40, attrs in 0usize..5) {
        use std::fmt::Write as _;
        let mut attr = String::new();
        for i in 0..attrs {
            let _ = write!(attr, " k{i}=\"v{i}\"");
        }
        let mut s = String::new();
        for i in 0..depth {
            let _ = write!(s, "<e{i}{attr}>");
        }
        for i in (0..depth).rev() {
            let _ = write!(s, "</e{i}>");
        }
        let doc = XmlDocument::parse_str(&s).unwrap();
        prop_assert_eq!(doc.to_string_lossless(), s);
    }
}
