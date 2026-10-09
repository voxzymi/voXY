//! First look at a sample file: what container it is, what each entry is,
//! and whether the lossless layers reproduce it byte-for-byte.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use crate::container::{Container, ContainerError};
use crate::inventory::Inventory;
use crate::xml::XmlDocument;

/// What one file (or archive entry) contains.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Content {
    /// Parsed as XML; `round_trips` is whether writing it back is
    /// byte-identical.
    Xml {
        root: Option<String>,
        round_trips: bool,
    },
    /// Not XML. `kind` is a guess from the leading bytes ("PNG image", …).
    Binary {
        kind: &'static str,
    },
    Directory,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryReport {
    pub name: String,
    pub size: u64,
    pub method: Option<String>,
    pub content: Content,
}

/// The result of inspecting one sample.
#[derive(Debug, Clone)]
pub struct SampleReport {
    pub name: String,
    pub size: usize,
    pub is_zip: bool,
    /// For zips: whether an unedited re-export is byte-identical.
    pub package_round_trips: Option<bool>,
    pub entries: Vec<EntryReport>,
    /// Parsed XML documents, keyed by entry name (`""` for a bare file).
    pub documents: BTreeMap<String, XmlDocument>,
}

/// Guesses a binary format from its signature. Labels only; nothing
/// depends on them.
pub fn sniff(bytes: &[u8]) -> &'static str {
    let starts = |sig: &[u8]| bytes.starts_with(sig);
    if starts(b"\x89PNG\r\n\x1a\n") {
        "PNG image"
    } else if starts(&[0xFF, 0xD8, 0xFF]) {
        "JPEG image"
    } else if starts(b"GIF8") {
        "GIF image"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        "WebP image"
    } else if bytes.len() >= 12 && &bytes[0..4] == b"RIFF" && &bytes[8..12] == b"WAVE" {
        "WAV audio"
    } else if bytes.len() >= 8 && &bytes[4..8] == b"ftyp" {
        "ISO media (MP4/MOV/M4A)"
    } else if starts(b"ID3") || starts(&[0xFF, 0xFB]) {
        "MP3 audio"
    } else if starts(b"OggS") {
        "Ogg media"
    } else if starts(&[0x00, 0x01, 0x00, 0x00]) || starts(b"OTTO") || starts(b"true") {
        "font (TrueType/OpenType)"
    } else if starts(b"PK\x03\x04") {
        "zip archive"
    } else if starts(b"\x1f\x8b") {
        "gzip data"
    } else if starts(b"{") || starts(b"[") {
        "JSON-like text"
    } else if std::str::from_utf8(bytes).is_ok() {
        "UTF-8 text"
    } else {
        "unknown binary"
    }
}

fn classify(bytes: &[u8]) -> (Content, Option<XmlDocument>) {
    let looks_xml = {
        let t = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
        let t = t.trim_ascii_start();
        t.starts_with(b"<")
    };
    if looks_xml && let Ok(doc) = XmlDocument::parse(bytes) {
        let round_trips = doc.to_bytes() == bytes;
        let root = doc.root().map(|r| r.name().to_owned());
        return (Content::Xml { root, round_trips }, Some(doc));
    }
    (Content::Binary { kind: sniff(bytes) }, None)
}

/// Inspects a sample file given its name and bytes.
pub fn inspect(name: &str, bytes: Vec<u8>) -> Result<SampleReport, ContainerError> {
    let size = bytes.len();
    let mut documents = BTreeMap::new();
    match Container::open(bytes)? {
        Container::Bare(b) => {
            let (content, doc) = classify(&b);
            if let Some(d) = doc {
                documents.insert(String::new(), d);
            }
            Ok(SampleReport {
                name: name.to_owned(),
                size,
                is_zip: false,
                package_round_trips: None,
                entries: vec![EntryReport {
                    name: name.to_owned(),
                    size: b.len() as u64,
                    method: None,
                    content,
                }],
                documents,
            })
        }
        Container::Zip(z) => {
            let package_round_trips = Some(z.write(&BTreeMap::new())? == z.source());
            let mut entries = Vec::new();
            for e in z.entries() {
                let content = if e.is_dir {
                    Content::Directory
                } else {
                    let data = z.read(&e.name)?;
                    let (content, doc) = classify(&data);
                    if let Some(d) = doc {
                        documents.insert(e.name.clone(), d);
                    }
                    content
                };
                entries.push(EntryReport {
                    name: e.name.clone(),
                    size: e.size,
                    method: Some(e.method.clone()),
                    content,
                });
            }
            Ok(SampleReport {
                name: name.to_owned(),
                size,
                is_zip: true,
                package_round_trips,
                entries,
                documents,
            })
        }
    }
}

impl SampleReport {
    /// Adds this sample's XML documents to an inventory, one sample label
    /// per document (`file` or `file!entry`).
    pub fn add_to(&self, inventory: &mut Inventory) {
        for (entry, doc) in &self.documents {
            let label = if entry.is_empty() {
                self.name.clone()
            } else {
                format!("{}!{entry}", self.name)
            };
            inventory.add(&label, doc);
        }
    }

    pub fn to_markdown(&self) -> String {
        let mut md = String::new();
        let _ = writeln!(md, "## Sample `{}`\n", self.name);
        let _ = writeln!(
            md,
            "- size: {} bytes\n- container: {}",
            self.size,
            if self.is_zip {
                "zip archive"
            } else {
                "single file (not a zip)"
            }
        );
        if let Some(ok) = self.package_round_trips {
            let _ = writeln!(
                md,
                "- unedited re-export byte-identical: {}",
                if ok { "yes" } else { "**no**" }
            );
        }
        let _ = writeln!(
            md,
            "\n| Entry | Size | Method | Content |\n|---|---|---|---|"
        );
        for e in &self.entries {
            let content = match &e.content {
                Content::Xml { root, round_trips } => format!(
                    "XML, root `<{}>`, lossless round trip: {}",
                    root.as_deref().unwrap_or("?"),
                    if *round_trips { "yes" } else { "**no**" }
                ),
                Content::Binary { kind } => (*kind).to_owned(),
                Content::Directory => "directory".to_owned(),
            };
            let _ = writeln!(
                md,
                "| `{}` | {} | {} | {content} |",
                e.name,
                e.size,
                e.method.as_deref().unwrap_or("—")
            );
        }
        md.push('\n');
        md
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};
    use zip::write::SimpleFileOptions;

    #[test]
    fn bare_xml_sample() {
        let r = inspect("a.xml", b"<?xml version=\"1.0\"?>\n<r x='1'/>\n".to_vec()).unwrap();
        assert!(!r.is_zip);
        assert_eq!(
            r.entries[0].content,
            Content::Xml {
                root: Some("r".into()),
                round_trips: true
            }
        );
        assert!(r.to_markdown().contains("lossless round trip: yes"));
    }

    #[test]
    fn zip_sample_with_mixed_entries() {
        let mut w = zip::ZipWriter::new(Cursor::new(Vec::new()));
        let o = SimpleFileOptions::default();
        w.start_file("main.xml", o).unwrap();
        w.write_all(b"<doc><item/></doc>").unwrap();
        w.start_file("pic.png", o).unwrap();
        w.write_all(b"\x89PNG\r\n\x1a\nrest").unwrap();
        let bytes = w.finish().unwrap().into_inner();
        let r = inspect("s.zip", bytes).unwrap();
        assert!(r.is_zip);
        assert_eq!(r.package_round_trips, Some(true));
        assert_eq!(r.entries[1].content, Content::Binary { kind: "PNG image" });
        let mut inv = Inventory::default();
        r.add_to(&mut inv);
        assert!(inv.elements.contains_key("doc/item"));
        assert_eq!(inv.samples, ["s.zip!main.xml"]);
    }

    #[test]
    fn non_xml_bare_file() {
        let r = inspect("x.bin", vec![0, 159, 146, 150]).unwrap();
        assert_eq!(
            r.entries[0].content,
            Content::Binary {
                kind: "unknown binary"
            }
        );
    }
}
