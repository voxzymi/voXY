//! Structural XML diff for differential analysis: change one setting in the
//! app, export again, and see exactly which elements and attributes moved.
//!
//! Children are matched by element name and position among siblings of the
//! same name (`layer[0]`, `layer[1]`, …), which is right for "same project,
//! one setting changed". Inserting an element in the middle shows up as a
//! cascade of changes; that is visible and honest rather than clever.
//! Comments, processing instructions and formatting whitespace are not part
//! of the structural diff (the lossless tree still preserves them).

use std::collections::BTreeMap;
use std::fmt;

use crate::xml::{Element, XmlDocument};

/// One difference, located by an element path such as `/root/layer[1]`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    AttributeChanged {
        path: String,
        name: String,
        old: String,
        new: String,
    },
    AttributeAdded {
        path: String,
        name: String,
        value: String,
    },
    AttributeRemoved {
        path: String,
        name: String,
        value: String,
    },
    TextChanged {
        path: String,
        old: String,
        new: String,
    },
    ElementAdded {
        path: String,
    },
    ElementRemoved {
        path: String,
    },
    RootRenamed {
        old: String,
        new: String,
    },
}

impl fmt::Display for Change {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Change::AttributeChanged {
                path,
                name,
                old,
                new,
            } => write!(f, "~ {path} @{name}: {old:?} -> {new:?}"),
            Change::AttributeAdded { path, name, value } => {
                write!(f, "+ {path} @{name} = {value:?}")
            }
            Change::AttributeRemoved { path, name, value } => {
                write!(f, "- {path} @{name} (was {value:?})")
            }
            Change::TextChanged { path, old, new } => {
                write!(f, "~ {path} text: {old:?} -> {new:?}")
            }
            Change::ElementAdded { path } => write!(f, "+ {path}"),
            Change::ElementRemoved { path } => write!(f, "- {path}"),
            Change::RootRenamed { old, new } => write!(f, "~ root element <{old}> -> <{new}>"),
        }
    }
}

/// Differences from `a` to `b`, in document order.
pub fn diff(a: &XmlDocument, b: &XmlDocument) -> Vec<Change> {
    let mut out = Vec::new();
    match (a.root(), b.root()) {
        (Some(ra), Some(rb)) if ra.name() == rb.name() => {
            diff_elements(ra, rb, &format!("/{}", ra.name()), &mut out);
        }
        (Some(ra), Some(rb)) => out.push(Change::RootRenamed {
            old: ra.name().to_owned(),
            new: rb.name().to_owned(),
        }),
        (Some(ra), None) => out.push(Change::ElementRemoved {
            path: format!("/{}", ra.name()),
        }),
        (None, Some(rb)) => out.push(Change::ElementAdded {
            path: format!("/{}", rb.name()),
        }),
        (None, None) => {}
    }
    out
}

fn value(e: &Element, name: &str) -> String {
    e.attribute(name)
        .map(crate::xml::Attribute::value_lossy)
        .unwrap_or_default()
}

fn own_text(e: &Element) -> String {
    let t = e.text().unwrap_or_default();
    t.trim().to_owned()
}

fn diff_elements(a: &Element, b: &Element, path: &str, out: &mut Vec<Change>) {
    for attr in a.attributes() {
        let old = value(a, &attr.name);
        if b.attribute(&attr.name).is_none() {
            out.push(Change::AttributeRemoved {
                path: path.to_owned(),
                name: attr.name.clone(),
                value: old,
            });
        } else {
            let new = value(b, &attr.name);
            if new != old {
                out.push(Change::AttributeChanged {
                    path: path.to_owned(),
                    name: attr.name.clone(),
                    old,
                    new,
                });
            }
        }
    }
    for attr in b.attributes() {
        if a.attribute(&attr.name).is_none() {
            out.push(Change::AttributeAdded {
                path: path.to_owned(),
                name: attr.name.clone(),
                value: value(b, &attr.name),
            });
        }
    }
    let (ta, tb) = (own_text(a), own_text(b));
    if ta != tb {
        out.push(Change::TextChanged {
            path: path.to_owned(),
            old: ta,
            new: tb,
        });
    }

    // Pair children by (name, index among same-named siblings), keeping
    // the order of `a` and then any extra elements of `b`.
    let index = |e: &Element| -> Vec<(String, usize)> {
        let mut seen: BTreeMap<&str, usize> = BTreeMap::new();
        e.elements()
            .map(|c| {
                let n = seen.entry(c.name()).or_default();
                *n += 1;
                (c.name().to_owned(), *n - 1)
            })
            .collect()
    };
    let (ka, kb) = (index(a), index(b));
    let ca: Vec<&Element> = a.elements().collect();
    let cb: Vec<&Element> = b.elements().collect();
    for (i, key) in ka.iter().enumerate() {
        let child_path = format!("{path}/{}[{}]", key.0, key.1);
        match kb.iter().position(|k| k == key) {
            Some(j) => diff_elements(ca[i], cb[j], &child_path, out),
            None => out.push(Change::ElementRemoved { path: child_path }),
        }
    }
    for key in &kb {
        if !ka.contains(key) {
            out.push(Change::ElementAdded {
                path: format!("{path}/{}[{}]", key.0, key.1),
            });
        }
    }
}

/// Whether two documents are equal ignoring formatting (the diff is empty).
pub fn same_structure(a: &XmlDocument, b: &XmlDocument) -> bool {
    diff(a, b).is_empty()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(s: &str) -> XmlDocument {
        XmlDocument::parse_str(s).unwrap()
    }

    #[test]
    fn identical_and_reformatted_documents_have_no_changes() {
        let a = doc(r#"<r a="1"><x/><x b="2"/></r>"#);
        let b = doc("<r  a='1'>\n  <x></x>\n  <x b=\"2\" />\n</r>");
        assert!(same_structure(&a, &a));
        assert!(same_structure(&a, &b));
    }

    #[test]
    fn reports_each_kind_of_change() {
        let a = doc(r#"<r a="1" gone="x"><x v="1"/><x v="2"/><t>old</t></r>"#);
        let b = doc(r#"<r a="2" new="y"><x v="1"/><t>new</t><z/></r>"#);
        let lines: Vec<String> = diff(&a, &b).iter().map(ToString::to_string).collect();
        assert_eq!(
            lines,
            [
                r#"~ /r @a: "1" -> "2""#,
                r#"- /r @gone (was "x")"#,
                r#"+ /r @new = "y""#,
                "- /r/x[1]",
                r#"~ /r/t[0] text: "old" -> "new""#,
                "+ /r/z[0]",
            ]
        );
    }

    #[test]
    fn entities_compare_by_value() {
        assert!(same_structure(
            &doc(r#"<r a="&amp;"/>"#),
            &doc(r#"<r a="&#38;"/>"#)
        ));
    }

    #[test]
    fn different_roots() {
        assert_eq!(diff(&doc("<a/>"), &doc("<b/>")).len(), 1);
    }
}
