//! Sample inventory: what elements, attributes and values appear, where.
//!
//! This is the raw material for `docs/am-format.md`. It records only what
//! the files literally contain: element paths, attribute names, how values
//! are *written* (integer, decimal, hex, …), example values, and which
//! samples show each. It never interprets what anything means; that is
//! established by differential analysis and recorded by hand as
//! confirmed or inferred.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::xml::{Element, Node, XmlDocument};

/// How many distinct example values to keep per attribute.
const MAX_EXAMPLES: usize = 6;
/// Longest example value kept, in characters.
const MAX_EXAMPLE_LEN: usize = 48;

/// The written form of a value. Syntax only, never meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Shape {
    Empty,
    /// `true` / `false`.
    Bool,
    /// Optional sign and digits.
    Integer,
    /// A number with a fraction or exponent.
    Decimal,
    /// `#` or `0x` followed by hex digits.
    Hex,
    /// Several numbers separated by commas, spaces or semicolons.
    NumberList,
    Text,
}

impl Shape {
    pub fn of(value: &str) -> Shape {
        let v = value.trim();
        if v.is_empty() {
            return Shape::Empty;
        }
        if v == "true" || v == "false" {
            return Shape::Bool;
        }
        if is_integer(v) {
            return Shape::Integer;
        }
        if is_decimal(v) {
            return Shape::Decimal;
        }
        let hex = v
            .strip_prefix('#')
            .or_else(|| v.strip_prefix("0x"))
            .or_else(|| v.strip_prefix("0X"));
        if hex.is_some_and(|h| !h.is_empty() && h.chars().all(|c| c.is_ascii_hexdigit())) {
            return Shape::Hex;
        }
        let parts: Vec<&str> = v.split([',', ' ', ';']).filter(|p| !p.is_empty()).collect();
        if parts.len() > 1 && parts.iter().all(|p| is_integer(p) || is_decimal(p)) {
            return Shape::NumberList;
        }
        Shape::Text
    }

    fn label(self) -> &'static str {
        match self {
            Shape::Empty => "empty",
            Shape::Bool => "bool",
            Shape::Integer => "integer",
            Shape::Decimal => "decimal",
            Shape::Hex => "hex",
            Shape::NumberList => "number list",
            Shape::Text => "text",
        }
    }
}

fn is_integer(v: &str) -> bool {
    let d = v.strip_prefix(['-', '+']).unwrap_or(v);
    !d.is_empty() && d.chars().all(|c| c.is_ascii_digit())
}

fn is_decimal(v: &str) -> bool {
    v.parse::<f64>().is_ok_and(f64::is_finite)
        && v.chars().any(|c| matches!(c, '.' | 'e' | 'E'))
        && v.chars()
            .all(|c| c.is_ascii_digit() || matches!(c, '.' | 'e' | 'E' | '-' | '+'))
}

/// Observations about one attribute (or an element's text).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ValueStats {
    pub count: usize,
    pub shapes: BTreeSet<Shape>,
    pub examples: BTreeSet<String>,
    pub min: Option<f64>,
    pub max: Option<f64>,
}

impl ValueStats {
    fn add(&mut self, value: &str) {
        self.count += 1;
        let shape = Shape::of(value);
        self.shapes.insert(shape);
        if matches!(shape, Shape::Integer | Shape::Decimal)
            && let Ok(x) = value.trim().parse::<f64>()
        {
            self.min = Some(self.min.map_or(x, |m| m.min(x)));
            self.max = Some(self.max.map_or(x, |m| m.max(x)));
        }
        if self.examples.len() < MAX_EXAMPLES {
            let mut ex: String = value.chars().take(MAX_EXAMPLE_LEN).collect();
            if value.chars().count() > MAX_EXAMPLE_LEN {
                ex.push('…');
            }
            self.examples.insert(ex);
        }
    }

    fn summary(&self) -> String {
        let shapes: Vec<_> = self.shapes.iter().map(|s| s.label()).collect();
        let mut s = shapes.join(", ");
        if let (Some(lo), Some(hi)) = (self.min, self.max) {
            let _ = write!(s, " ({lo} … {hi})");
        }
        s
    }
}

/// Observations about one element path.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ElementStats {
    pub count: usize,
    /// Names of the samples in which this path occurs.
    pub samples: BTreeSet<String>,
    pub attributes: BTreeMap<String, ValueStats>,
    pub children: BTreeSet<String>,
    /// Non-whitespace text content.
    pub text: ValueStats,
}

/// An inventory across any number of samples.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Inventory {
    /// Keyed by element path, e.g. `root/child/leaf`.
    pub elements: BTreeMap<String, ElementStats>,
    /// Samples added, in order.
    pub samples: Vec<String>,
}

impl Inventory {
    /// Records every element of `doc` under the sample name `sample`.
    pub fn add(&mut self, sample: &str, doc: &XmlDocument) {
        self.samples.push(sample.to_owned());
        for n in &doc.nodes {
            if let Node::Element(e) = n {
                self.add_element(sample, e, "");
            }
        }
    }

    fn add_element(&mut self, sample: &str, e: &Element, parent: &str) {
        let path = if parent.is_empty() {
            e.name().to_owned()
        } else {
            format!("{parent}/{}", e.name())
        };
        let stats = self.elements.entry(path.clone()).or_default();
        stats.count += 1;
        stats.samples.insert(sample.to_owned());
        for a in e.attributes() {
            let value = a.value_lossy();
            stats
                .attributes
                .entry(a.name.clone())
                .or_default()
                .add(&value);
        }
        for c in e.elements() {
            stats.children.insert(c.name().to_owned());
        }
        if let Ok(text) = e.text()
            && !text.trim().is_empty()
        {
            stats.text.add(&text);
        }
        for c in e.elements() {
            self.add_element(sample, c, &path);
        }
    }

    /// A Markdown report for the format notes.
    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        let _ = writeln!(md, "Samples: {}\n", self.samples.join(", "));
        let _ = writeln!(
            md,
            "Shapes describe how values are written, not what they mean.\n"
        );
        for (path, e) in &self.elements {
            let _ = writeln!(md, "### `{path}`\n");
            let samples: Vec<_> = e.samples.iter().map(String::as_str).collect();
            let _ = writeln!(md, "- occurrences: {} (in {})", e.count, samples.join(", "));
            if !e.children.is_empty() {
                let children: Vec<_> = e.children.iter().map(|c| format!("`{c}`")).collect();
                let _ = writeln!(md, "- child elements: {}", children.join(", "));
            }
            if e.text.count > 0 {
                let _ = writeln!(
                    md,
                    "- text: {} — e.g. {}",
                    e.text.summary(),
                    examples(&e.text)
                );
            }
            if !e.attributes.is_empty() {
                let _ = writeln!(
                    md,
                    "\n| Attribute | Seen | Shape | Examples |\n|---|---|---|---|"
                );
                for (name, a) in &e.attributes {
                    let _ = writeln!(
                        md,
                        "| `{name}` | {} | {} | {} |",
                        a.count,
                        a.summary(),
                        examples(a)
                    );
                }
            }
            md.push('\n');
        }
        md
    }
}

fn examples(v: &ValueStats) -> String {
    v.examples
        .iter()
        .map(|e| format!("`{}`", e.replace('|', "\\|").replace('`', "'")))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shapes() {
        assert_eq!(Shape::of(""), Shape::Empty);
        assert_eq!(Shape::of("true"), Shape::Bool);
        assert_eq!(Shape::of("-12"), Shape::Integer);
        assert_eq!(Shape::of("0.5"), Shape::Decimal);
        assert_eq!(Shape::of("1e-3"), Shape::Decimal);
        assert_eq!(Shape::of("#ff00aa"), Shape::Hex);
        assert_eq!(Shape::of("0xFF"), Shape::Hex);
        assert_eq!(Shape::of("1,2.5,3"), Shape::NumberList);
        assert_eq!(Shape::of("1 2"), Shape::NumberList);
        assert_eq!(Shape::of("hello"), Shape::Text);
        assert_eq!(Shape::of("nan"), Shape::Text);
        assert_eq!(Shape::of("e"), Shape::Text);
    }

    #[test]
    #[allow(clippy::many_single_char_names)]
    fn inventories_paths_attributes_and_samples() {
        // Generic XML; no real format's vocabulary.
        let a = XmlDocument::parse_str(r#"<r v="1"><x n="0.5"/><x n="2" k="a"/><t>hi</t></r>"#)
            .unwrap();
        let b = XmlDocument::parse_str(r#"<r v="3"><y/></r>"#).unwrap();
        let mut inv = Inventory::default();
        inv.add("s1", &a);
        inv.add("s2", &b);
        let r = &inv.elements["r"];
        assert_eq!(r.count, 2);
        assert_eq!(r.samples.len(), 2);
        assert_eq!(
            r.children,
            ["t", "x", "y"].map(String::from).into_iter().collect()
        );
        let v = &r.attributes["v"];
        assert_eq!((v.min, v.max), (Some(1.0), Some(3.0)));
        let x = &inv.elements["r/x"];
        assert_eq!(x.count, 2);
        assert_eq!(
            x.attributes["n"].shapes,
            [Shape::Integer, Shape::Decimal].into_iter().collect()
        );
        assert_eq!(x.attributes["k"].count, 1);
        assert_eq!(
            inv.elements["r/t"].text.examples.iter().next().unwrap(),
            "hi"
        );
        let md = inv.to_markdown();
        assert!(
            md.contains("### `r/x`") && md.contains("| `n` | 2 |"),
            "{md}"
        );
    }
}
