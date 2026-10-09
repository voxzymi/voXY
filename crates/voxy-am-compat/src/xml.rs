//! A lossless XML tree.
//!
//! [`XmlDocument::parse`] keeps the exact source text of every node: the
//! declaration, comments, processing instructions, the DOCTYPE, CDATA,
//! whitespace, line endings, attribute order, attribute quoting and spacing
//! inside tags, and character or entity references in text. As a result:
//!
//! - **An unedited document writes back byte-for-byte**:
//!   `parse(x)?.to_bytes() == x`. Tests and fuzzing check this.
//! - **An edit only regenerates what it touches.** Changing an attribute
//!   rewrites that element's start tag, and every other byte is reproduced
//!   as-is.
//!
//! Tokenizing is done by `quick-xml`. This module records the byte span of
//! every event and builds the tree from those spans. The tree makes no
//! assumptions about the vocabulary (element and attribute names) of any
//! particular format. Format knowledge lives in the typed layer on top, which
//! is built from real sample files.
//!
//! Input must be UTF-8, with or without a byte-order mark. Other encodings
//! are rejected with a clear error until a sample shows they are needed.

use std::borrow::Cow;
use std::fmt::Write as _;

use quick_xml::events::Event;

const BOM: &str = "\u{feff}";

/// Why a document could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum XmlError {
    #[error("not UTF-8 text (byte {offset}); other encodings are not supported yet")]
    NotUtf8 { offset: usize },
    #[error("malformed XML at byte {offset}: {message}")]
    Syntax { offset: u64, message: String },
    #[error("malformed attributes in tag at byte {offset}: {message}")]
    Attributes { offset: u64, message: String },
    #[error("element <{name}> at byte {offset} is never closed")]
    Unclosed { name: String, offset: u64 },
    #[error("invalid escape: {0}")]
    Escape(String),
}

/// A whole XML file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct XmlDocument {
    /// The file started with a UTF-8 byte-order mark.
    pub bom: bool,
    /// Top-level nodes in order: declaration, comments, whitespace, root
    /// element, and so on.
    pub nodes: Vec<Node>,
}

/// A node, holding its exact source text where applicable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Element(Element),
    /// Character data exactly as written (escaped, references unexpanded).
    Text(String),
    /// A complete `<![CDATA[...]]>` section.
    CData(String),
    /// A complete `<!--...-->` comment.
    Comment(String),
    /// The complete `<?xml ...?>` declaration.
    Decl(String),
    /// A complete `<?target ...?>` processing instruction.
    Pi(String),
    /// The complete `<!DOCTYPE ...>`.
    DocType(String),
}

/// An attribute: name, value as written (escaped), and its quote character.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Attribute {
    pub name: String,
    raw_value: String,
    quote: char,
}

impl Attribute {
    /// The value with references expanded.
    pub fn value(&self) -> Result<Cow<'_, str>, XmlError> {
        unescape(&self.raw_value)
    }

    /// The value exactly as written between the quotes.
    pub fn raw_value(&self) -> &str {
        &self.raw_value
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Element {
    name: String,
    attrs: Vec<Attribute>,
    pub children: Vec<Node>,
    /// Written as `<name/>` in the source.
    self_closing: bool,
    /// Original start tag. Cleared when the name or attributes change.
    raw_start: Option<String>,
    /// Original end tag (e.g. `</name >`). `None` for self-closing tags and
    /// new elements.
    raw_end: Option<String>,
}

impl Element {
    /// A new element with no attributes or children.
    pub fn new(name: impl Into<String>) -> Self {
        Element {
            name: name.into(),
            attrs: Vec::new(),
            children: Vec::new(),
            self_closing: true,
            raw_start: None,
            raw_end: None,
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// Attributes in source order.
    pub fn attributes(&self) -> &[Attribute] {
        &self.attrs
    }

    pub fn attribute(&self, name: &str) -> Option<&Attribute> {
        self.attrs.iter().find(|a| a.name == name)
    }

    /// The unescaped value of attribute `name`.
    pub fn attr(&self, name: &str) -> Option<Result<Cow<'_, str>, XmlError>> {
        self.attribute(name).map(Attribute::value)
    }

    /// Sets an attribute, keeping its position (and quote style) if it
    /// exists, otherwise appending it. Regenerates this start tag only.
    pub fn set_attr(&mut self, name: &str, value: &str) {
        let changed = if let Some(a) = self.attrs.iter_mut().find(|a| a.name == name) {
            let raw = escape_attr(value, a.quote);
            let changed = a.raw_value != raw;
            a.raw_value = raw;
            changed
        } else {
            self.attrs.push(Attribute {
                name: name.to_owned(),
                raw_value: escape_attr(value, '"'),
                quote: '"',
            });
            true
        };
        if changed {
            self.raw_start = None;
        }
    }

    /// Removes an attribute; returns whether it existed.
    pub fn remove_attr(&mut self, name: &str) -> bool {
        let before = self.attrs.len();
        self.attrs.retain(|a| a.name != name);
        let removed = self.attrs.len() != before;
        if removed {
            self.raw_start = None;
        }
        removed
    }

    pub fn rename(&mut self, name: impl Into<String>) {
        let name = name.into();
        if name != self.name {
            self.name = name;
            self.raw_start = None;
            self.raw_end = None;
        }
    }

    /// Child elements, in order.
    pub fn elements(&self) -> impl Iterator<Item = &Element> {
        self.children.iter().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
    }

    pub fn elements_mut(&mut self) -> impl Iterator<Item = &mut Element> {
        self.children.iter_mut().filter_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
    }

    /// First child element named `name`.
    pub fn child(&self, name: &str) -> Option<&Element> {
        self.elements().find(|e| e.name == name)
    }

    /// Concatenated, unescaped text of the direct text and CDATA children.
    pub fn text(&self) -> Result<String, XmlError> {
        let mut out = String::new();
        for n in &self.children {
            match n {
                Node::Text(t) => out.push_str(&unescape(t)?),
                Node::CData(c) => out.push_str(cdata_content(c)),
                _ => {}
            }
        }
        Ok(out)
    }

    /// Replaces all children with one text node.
    pub fn set_text(&mut self, text: &str) {
        self.children = vec![Node::Text(escape_text(text))];
    }

    /// Whether this element's own tags are still exactly as in the source.
    pub fn is_pristine(&self) -> bool {
        self.raw_start.is_some()
    }

    fn write(&self, out: &mut String) {
        // `<a/>` is kept only while the element still has no children.
        let close_inline = self.self_closing && self.children.is_empty();
        match &self.raw_start {
            Some(raw) if self.self_closing == close_inline => out.push_str(raw),
            _ => {
                out.push('<');
                out.push_str(&self.name);
                for a in &self.attrs {
                    let _ = write!(out, " {}={q}{}{q}", a.name, a.raw_value, q = a.quote);
                }
                out.push_str(if close_inline { "/>" } else { ">" });
            }
        }
        if close_inline {
            return;
        }
        for c in &self.children {
            c.write(out);
        }
        match &self.raw_end {
            Some(raw) => out.push_str(raw),
            None => {
                let _ = write!(out, "</{}>", self.name);
            }
        }
    }
}

impl Node {
    fn write(&self, out: &mut String) {
        match self {
            Node::Element(e) => e.write(out),
            Node::Text(s)
            | Node::CData(s)
            | Node::Comment(s)
            | Node::Decl(s)
            | Node::Pi(s)
            | Node::DocType(s) => out.push_str(s),
        }
    }
}

impl XmlDocument {
    /// Parses UTF-8 XML (an optional BOM is kept).
    pub fn parse(bytes: &[u8]) -> Result<Self, XmlError> {
        let text = std::str::from_utf8(bytes).map_err(|e| XmlError::NotUtf8 {
            offset: e.valid_up_to(),
        })?;
        Self::parse_str(text)
    }

    pub fn parse_str(text: &str) -> Result<Self, XmlError> {
        let (bom, body) = match text.strip_prefix(BOM) {
            Some(rest) => (true, rest),
            None => (false, text),
        };
        let base = if bom { BOM.len() as u64 } else { 0 };

        let mut reader = quick_xml::Reader::from_str(body);
        let config = reader.config_mut();
        config.trim_text_start = false;
        config.trim_text_end = false;
        config.expand_empty_elements = false;
        config.check_end_names = true;
        config.allow_unmatched_ends = false;

        // Open elements, innermost last; each with its start offset.
        let mut stack: Vec<(Element, u64)> = Vec::new();
        let mut top: Vec<Node> = Vec::new();
        let mut prev = 0usize;

        loop {
            let event = reader.read_event().map_err(|e| XmlError::Syntax {
                offset: base + reader.error_position(),
                message: e.to_string(),
            })?;
            let end = reader.buffer_position() as usize;
            let raw = &body[prev..end];
            let start_offset = base + prev as u64;
            prev = end;
            let node = match event {
                Event::Eof => break,
                Event::Start(_) => {
                    let mut e = parse_start_tag(raw, start_offset)?;
                    e.self_closing = false;
                    stack.push((e, start_offset));
                    continue;
                }
                Event::Empty(_) => Node::Element(parse_start_tag(raw, start_offset)?),
                Event::End(_) => {
                    let (mut e, _) = stack.pop().ok_or_else(|| XmlError::Syntax {
                        offset: start_offset,
                        message: "unexpected end tag".into(),
                    })?;
                    e.raw_end = Some(raw.to_owned());
                    Node::Element(e)
                }
                // References are kept inside the surrounding text, verbatim.
                Event::Text(_) | Event::GeneralRef(_) => Node::Text(raw.to_owned()),
                Event::CData(_) => Node::CData(raw.to_owned()),
                Event::Comment(_) => Node::Comment(raw.to_owned()),
                Event::Decl(_) => Node::Decl(raw.to_owned()),
                Event::PI(_) => Node::Pi(raw.to_owned()),
                Event::DocType(_) => Node::DocType(raw.to_owned()),
            };
            let siblings = match stack.last_mut() {
                Some((parent, _)) => &mut parent.children,
                None => &mut top,
            };
            // Merge adjacent text pieces (text split around references).
            if let Node::Text(t) = &node
                && let Some(Node::Text(last)) = siblings.last_mut()
            {
                last.push_str(t);
            } else {
                siblings.push(node);
            }
        }
        if let Some((e, offset)) = stack.pop() {
            return Err(XmlError::Unclosed {
                name: e.name,
                offset,
            });
        }
        debug_assert_eq!(prev, body.len(), "every byte must belong to a node");
        Ok(XmlDocument { bom, nodes: top })
    }

    /// The first top-level element.
    pub fn root(&self) -> Option<&Element> {
        self.nodes.iter().find_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
    }

    pub fn root_mut(&mut self) -> Option<&mut Element> {
        self.nodes.iter_mut().find_map(|n| match n {
            Node::Element(e) => Some(e),
            _ => None,
        })
    }

    /// Serializes the document. Unedited parts are reproduced exactly.
    pub fn to_string_lossless(&self) -> String {
        let mut out = String::new();
        if self.bom {
            out.push_str(BOM);
        }
        for n in &self.nodes {
            n.write(&mut out);
        }
        out
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_string_lossless().into_bytes()
    }
}

/// Parses a complete start tag (`<name a="1" b='2'>` or `.../>`).
fn parse_start_tag(raw: &str, offset: u64) -> Result<Element, XmlError> {
    let err = |message: &str| XmlError::Attributes {
        offset,
        message: message.to_owned(),
    };
    let self_closing = raw.ends_with("/>");
    let inner = raw
        .strip_prefix('<')
        .and_then(|s| s.strip_suffix(if self_closing { "/>" } else { ">" }))
        .ok_or_else(|| err("not a start tag"))?;
    let name_end = inner
        .find(|c: char| c.is_ascii_whitespace())
        .unwrap_or(inner.len());
    let name = &inner[..name_end];
    if name.is_empty() {
        return Err(err("missing element name"));
    }
    let mut attrs: Vec<Attribute> = Vec::new();
    let mut rest = &inner[name_end..];
    loop {
        rest = rest.trim_start_matches(|c: char| c.is_ascii_whitespace());
        if rest.is_empty() {
            break;
        }
        let eq = rest
            .find('=')
            .ok_or_else(|| err("attribute without value"))?;
        let attr_name = rest[..eq].trim_end_matches(|c: char| c.is_ascii_whitespace());
        if attr_name.is_empty() || attr_name.contains(|c: char| c.is_ascii_whitespace()) {
            return Err(err("malformed attribute name"));
        }
        rest = rest[eq + 1..].trim_start_matches(|c: char| c.is_ascii_whitespace());
        let quote = rest
            .chars()
            .next()
            .ok_or_else(|| err("missing attribute value"))?;
        if quote != '"' && quote != '\'' {
            return Err(err("unquoted attribute value"));
        }
        let close = rest[1..]
            .find(quote)
            .ok_or_else(|| err("unterminated attribute value"))?;
        let raw_value = &rest[1..=close];
        if raw_value.contains('<') {
            return Err(err("`<` in attribute value"));
        }
        if attrs.iter().any(|a| a.name == attr_name) {
            return Err(err(&format!("duplicate attribute `{attr_name}`")));
        }
        attrs.push(Attribute {
            name: attr_name.to_owned(),
            raw_value: raw_value.to_owned(),
            quote,
        });
        rest = &rest[close + 2..];
    }
    Ok(Element {
        name: name.to_owned(),
        attrs,
        children: Vec::new(),
        self_closing,
        raw_start: Some(raw.to_owned()),
        raw_end: None,
    })
}

fn cdata_content(raw: &str) -> &str {
    raw.strip_prefix("<![CDATA[")
        .and_then(|s| s.strip_suffix("]]>"))
        .unwrap_or(raw)
}

/// Expands the predefined entities and character references.
pub fn unescape(raw: &str) -> Result<Cow<'_, str>, XmlError> {
    quick_xml::escape::unescape(raw).map_err(|e| XmlError::Escape(e.to_string()))
}

/// Escapes text content: `&`, `<` and `>`.
pub fn escape_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            c => out.push(c),
        }
    }
    out
}

/// Escapes an attribute value for the given quote character.
pub fn escape_attr(s: &str, quote: char) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '"' if quote == '"' => out.push_str("&quot;"),
            '\'' if quote == '\'' => out.push_str("&apos;"),
            // Literal tabs/newlines in attributes are normalized to spaces by
            // XML parsers; keep them as character references.
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c => out.push(c),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generic XML exercising every construct the tree must preserve. None of
    /// it is modelled on any particular application's format.
    const KITCHEN_SINK: &str = "\u{feff}<?xml version=\"1.0\" encoding=\"utf-8\" standalone='yes'?>\r\n\
<!DOCTYPE note [\r\n  <!ELEMENT note (#PCDATA)>\r\n]>\r\n\
<!-- leading comment -->\r\n\
<?app-hint mode=\"x\"?>\r\n\
<root  xmlns:ns = \"urn:example\"   a='1'\tb=\"two &amp; &#x41;\" >\r\n\
\t<ns:child id=\"c1\"/>\r\n\
\t<child   id = 'c2' ></child  >\r\n\
\t<text>Tom &amp; Jerry &lt;3 &#169; ünïcödé</text>\r\n\
\t<data><![CDATA[ <raw> & stuff ]]></data>\r\n\
\t<empty></empty>\r\n\
    <!-- inner -->\r\n\
</root>\r\n\
<!-- trailing -->\r\n";

    #[test]
    fn unedited_documents_round_trip_byte_for_byte() {
        let doc = XmlDocument::parse(KITCHEN_SINK.as_bytes()).unwrap();
        assert!(doc.bom);
        assert_eq!(doc.to_string_lossless(), KITCHEN_SINK);

        for src in [
            "<a/>",
            "<a></a>",
            "<a>text</a>",
            "  <a b=\"c\"/>  ",
            "<a>&lt;&gt;&amp;&quot;&apos;&#10;</a>",
            "<?xml version='1.0'?><a><b><c/></b></a>\n",
            "",
            "just text",
        ] {
            let doc = XmlDocument::parse_str(src).unwrap();
            assert_eq!(doc.to_string_lossless(), src, "{src:?}");
        }
    }

    #[test]
    fn navigation_and_values() {
        let doc = XmlDocument::parse(KITCHEN_SINK.as_bytes()).unwrap();
        let root = doc.root().unwrap();
        assert_eq!(root.name(), "root");
        let names: Vec<_> = root.attributes().iter().map(|a| a.name.as_str()).collect();
        assert_eq!(names, ["xmlns:ns", "a", "b"]);
        assert_eq!(root.attr("b").unwrap().unwrap(), "two & A");
        assert_eq!(root.attribute("b").unwrap().raw_value(), "two &amp; &#x41;");
        let elems: Vec<_> = root.elements().map(Element::name).collect();
        assert_eq!(elems, ["ns:child", "child", "text", "data", "empty"]);
        assert_eq!(
            root.child("text").unwrap().text().unwrap(),
            "Tom & Jerry <3 © ünïcödé"
        );
        assert_eq!(
            root.child("data").unwrap().text().unwrap(),
            " <raw> & stuff "
        );
    }

    #[test]
    fn edits_only_touch_their_own_tag() {
        let mut doc = XmlDocument::parse(KITCHEN_SINK.as_bytes()).unwrap();
        let root = doc.root_mut().unwrap();
        let child = root.elements_mut().nth(1).unwrap();
        child.set_attr("id", "c2 \"new\"");
        let out = doc.to_string_lossless();
        // Only the edited start tag changed; its quote style was kept.
        let expected = KITCHEN_SINK.replace("<child   id = 'c2' >", "<child id='c2 \"new\"'>");
        assert_eq!(out, expected);
        // And it parses back to the new value.
        let again = XmlDocument::parse_str(&out).unwrap();
        let v = again
            .root()
            .unwrap()
            .elements()
            .nth(1)
            .unwrap()
            .attr("id")
            .unwrap()
            .unwrap()
            .into_owned();
        assert_eq!(v, "c2 \"new\"");
    }

    #[test]
    fn adding_children_to_self_closing_element() {
        let mut doc = XmlDocument::parse_str("<a><b x=\"1\"/></a>").unwrap();
        let b = doc.root_mut().unwrap().elements_mut().next().unwrap();
        b.children.push(Node::Element(Element::new("c")));
        assert_eq!(doc.to_string_lossless(), "<a><b x=\"1\"><c/></b></a>");
    }

    #[test]
    fn set_text_and_new_attributes_escape() {
        let mut doc = XmlDocument::parse_str("<a k='v'>old</a>").unwrap();
        let a = doc.root_mut().unwrap();
        a.set_text("x < y & z");
        a.set_attr("n", "1\n2");
        assert_eq!(
            doc.to_string_lossless(),
            "<a k='v' n=\"1&#10;2\">x &lt; y &amp; z</a>"
        );
        assert_eq!(a_text(&doc), "x < y & z");
    }

    fn a_text(doc: &XmlDocument) -> String {
        doc.root().unwrap().text().unwrap()
    }

    #[test]
    fn removing_and_renaming() {
        let mut doc = XmlDocument::parse_str("<a x='1' y='2'><b/></a>").unwrap();
        let a = doc.root_mut().unwrap();
        assert!(a.remove_attr("x"));
        assert!(!a.remove_attr("x"));
        a.rename("z");
        assert_eq!(doc.to_string_lossless(), "<z y='2'><b/></z>");
    }

    #[test]
    fn noop_edit_keeps_raw_tag() {
        let mut doc = XmlDocument::parse_str("<a  x = '1' />").unwrap();
        doc.root_mut().unwrap().set_attr("x", "1");
        assert!(doc.root().unwrap().is_pristine());
        assert_eq!(doc.to_string_lossless(), "<a  x = '1' />");
    }

    #[test]
    fn malformed_input_is_an_error() {
        for bad in [
            "<a>",
            "<a></b>",
            "</a>",
            "<a x=1/>",
            "<a x='1' x='2'/>",
            "<a x='1/>",
        ] {
            assert!(XmlDocument::parse_str(bad).is_err(), "{bad:?}");
        }
        assert!(matches!(
            XmlDocument::parse(&[0x3c, 0xff, 0x3e]),
            Err(XmlError::NotUtf8 { offset: 1 })
        ));
    }
}
