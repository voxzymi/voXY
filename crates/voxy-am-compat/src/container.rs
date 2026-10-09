//! File containers: a zip package or a bare XML file.
//!
//! Nothing here assumes what an Alight Motion file contains. [`Container`]
//! only answers "is this a zip, and if so, what entries does it hold?". The
//! samples will show which entries matter.
//!
//! **Lossless re-export.** [`ZipPackage::write`] with no replacements returns
//! the original bytes unchanged. With replacements, every untouched entry is
//! copied with its compressed bytes, name, order, method and timestamp
//! as-is; only replaced entries are recompressed (with their original
//! method).
//!
//! **Untrusted input.** Archives are bounded: at most [`MAX_ENTRIES`]
//! entries and [`MAX_ENTRY_BYTES`] per decompressed entry, enforced on the
//! bytes actually read, not just the sizes the archive declares.

use std::collections::BTreeMap;
use std::io::{Cursor, Read, Write};

use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

/// Maximum number of entries in an archive.
pub const MAX_ENTRIES: usize = 100_000;
/// Maximum decompressed size of one entry (1 GiB).
pub const MAX_ENTRY_BYTES: u64 = 1 << 30;

#[derive(Debug, thiserror::Error)]
pub enum ContainerError {
    #[error("malformed zip archive: {0}")]
    Zip(#[from] zip::result::ZipError),
    #[error("could not read archive data: {0}")]
    Io(#[from] std::io::Error),
    #[error("archive has {0} entries; the limit is {MAX_ENTRIES}")]
    TooManyEntries(usize),
    #[error("entry `{0}` is larger than {MAX_ENTRY_BYTES} bytes")]
    EntryTooLarge(String),
    #[error("no entry named `{0}`")]
    NoSuchEntry(String),
    #[error("entry `{0}` is a directory")]
    IsDirectory(String),
}

/// What a file turned out to be.
#[derive(Debug, Clone)]
pub enum Container {
    /// Not a zip archive. Usually XML, but that is for the caller to
    /// establish by parsing it.
    Bare(Vec<u8>),
    Zip(ZipPackage),
}

impl Container {
    /// Recognizes a zip archive by its signature; anything else is bare.
    pub fn open(bytes: Vec<u8>) -> Result<Self, ContainerError> {
        if bytes.starts_with(b"PK\x03\x04") || bytes.starts_with(b"PK\x05\x06") {
            Ok(Container::Zip(ZipPackage::open(bytes)?))
        } else {
            Ok(Container::Bare(bytes))
        }
    }
}

/// Metadata of one archive entry, in archive order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryInfo {
    pub index: usize,
    pub name: String,
    pub is_dir: bool,
    /// Declared decompressed size.
    pub size: u64,
    pub compressed_size: u64,
    pub method: String,
}

/// A zip archive kept as its original bytes plus an entry index.
#[derive(Debug, Clone)]
pub struct ZipPackage {
    source: Vec<u8>,
    entries: Vec<EntryInfo>,
}

impl ZipPackage {
    pub fn open(source: Vec<u8>) -> Result<Self, ContainerError> {
        let mut archive = ZipArchive::new(Cursor::new(source.as_slice()))?;
        if archive.len() > MAX_ENTRIES {
            return Err(ContainerError::TooManyEntries(archive.len()));
        }
        let mut entries = Vec::with_capacity(archive.len());
        for index in 0..archive.len() {
            let f = archive.by_index_raw(index)?;
            entries.push(EntryInfo {
                index,
                name: f.name()?.into_owned(),
                is_dir: f.is_dir(),
                size: f.size(),
                compressed_size: f.compressed_size(),
                method: format!("{:?}", f.compression()),
            });
        }
        Ok(ZipPackage { source, entries })
    }

    pub fn entries(&self) -> &[EntryInfo] {
        &self.entries
    }

    /// The original archive bytes.
    pub fn source(&self) -> &[u8] {
        &self.source
    }

    /// Decompresses one entry, enforcing [`MAX_ENTRY_BYTES`].
    pub fn read(&self, name: &str) -> Result<Vec<u8>, ContainerError> {
        let info = self
            .entries
            .iter()
            .find(|e| e.name == name)
            .ok_or_else(|| ContainerError::NoSuchEntry(name.to_owned()))?;
        if info.is_dir {
            return Err(ContainerError::IsDirectory(name.to_owned()));
        }
        if info.size > MAX_ENTRY_BYTES {
            return Err(ContainerError::EntryTooLarge(name.to_owned()));
        }
        let mut archive = ZipArchive::new(Cursor::new(self.source.as_slice()))?;
        let f = archive.by_index(info.index)?;
        let mut out = Vec::with_capacity(usize::try_from(info.size.min(64 << 20)).unwrap_or(0));
        // Never trust the declared size: cap what is actually inflated.
        f.take(MAX_ENTRY_BYTES + 1).read_to_end(&mut out)?;
        if out.len() as u64 > MAX_ENTRY_BYTES {
            return Err(ContainerError::EntryTooLarge(name.to_owned()));
        }
        Ok(out)
    }

    /// Writes the package with some entries' contents replaced. With no
    /// replacements, returns the original bytes exactly.
    pub fn write(
        &self,
        replacements: &BTreeMap<String, Vec<u8>>,
    ) -> Result<Vec<u8>, ContainerError> {
        if replacements.is_empty() {
            return Ok(self.source.clone());
        }
        for name in replacements.keys() {
            if !self.entries.iter().any(|e| &e.name == name && !e.is_dir) {
                return Err(ContainerError::NoSuchEntry(name.clone()));
            }
        }
        let mut archive = ZipArchive::new(Cursor::new(self.source.as_slice()))?;
        let mut writer = ZipWriter::new(Cursor::new(Vec::new()));
        for info in &self.entries {
            let f = archive.by_index_raw(info.index)?;
            match replacements.get(&info.name) {
                None => writer.raw_copy_file(f)?,
                Some(data) => {
                    let method = f.compression();
                    let method = if method == CompressionMethod::Stored {
                        CompressionMethod::Stored
                    } else {
                        CompressionMethod::Deflated
                    };
                    let mut options = SimpleFileOptions::default()
                        .compression_method(method)
                        .large_file(data.len() as u64 >= u64::from(u32::MAX));
                    if let Some(t) = f.last_modified() {
                        options = options.last_modified_time(t);
                    }
                    if let Some(mode) = f.unix_mode() {
                        options = options.unix_permissions(mode);
                    }
                    drop(f);
                    writer.start_file(info.name.as_str(), options)?;
                    writer.write_all(data)?;
                }
            }
        }
        Ok(writer.finish()?.into_inner())
    }

    /// All entry contents, keyed by name (directories excluded).
    pub fn read_all(&self) -> Result<BTreeMap<String, Vec<u8>>, ContainerError> {
        self.entries
            .iter()
            .filter(|e| !e.is_dir)
            .map(|e| Ok((e.name.clone(), self.read(&e.name)?)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a generic test archive (no Alight Motion content).
    fn sample_zip() -> Vec<u8> {
        let mut w = ZipWriter::new(Cursor::new(Vec::new()));
        let deflate = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
        let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
        w.start_file("doc.xml", deflate).unwrap();
        w.write_all(b"<?xml version=\"1.0\"?>\n<root a=\"1\"><child/></root>\n")
            .unwrap();
        w.add_directory("media/", stored).unwrap();
        w.start_file("media/blob.bin", stored).unwrap();
        w.write_all(&[0u8, 1, 2, 3, 255]).unwrap();
        w.start_file("notes.txt", deflate).unwrap();
        w.write_all(&b"hello ".repeat(100)).unwrap();
        w.finish().unwrap().into_inner()
    }

    #[test]
    fn detects_zip_and_bare() {
        assert!(matches!(
            Container::open(sample_zip()).unwrap(),
            Container::Zip(_)
        ));
        assert!(matches!(
            Container::open(b"<a/>".to_vec()).unwrap(),
            Container::Bare(_)
        ));
        assert!(matches!(
            Container::open(Vec::new()).unwrap(),
            Container::Bare(_)
        ));
    }

    #[test]
    fn lists_and_reads_entries_in_order() {
        let Container::Zip(z) = Container::open(sample_zip()).unwrap() else {
            unreachable!()
        };
        let names: Vec<_> = z.entries().iter().map(|e| e.name.as_str()).collect();
        assert_eq!(names, ["doc.xml", "media/", "media/blob.bin", "notes.txt"]);
        assert!(z.entries()[1].is_dir);
        assert_eq!(z.read("media/blob.bin").unwrap(), [0, 1, 2, 3, 255]);
        assert!(matches!(
            z.read("missing"),
            Err(ContainerError::NoSuchEntry(_))
        ));
        assert!(matches!(
            z.read("media/"),
            Err(ContainerError::IsDirectory(_))
        ));
    }

    #[test]
    fn unedited_package_is_byte_identical() {
        let bytes = sample_zip();
        let Container::Zip(z) = Container::open(bytes.clone()).unwrap() else {
            unreachable!()
        };
        assert_eq!(z.write(&BTreeMap::new()).unwrap(), bytes);
    }

    #[test]
    fn replacing_one_entry_keeps_the_others_verbatim() {
        let Container::Zip(z) = Container::open(sample_zip()).unwrap() else {
            unreachable!()
        };
        let before = z.read_all().unwrap();
        let replaced = BTreeMap::from([("doc.xml".to_owned(), b"<root a=\"2\"/>".to_vec())]);
        let out = z.write(&replaced).unwrap();
        let Container::Zip(z2) = Container::open(out).unwrap() else {
            unreachable!()
        };
        // Same entries, same order, same methods.
        let strip = |e: &EntryInfo| (e.name.clone(), e.is_dir, e.method.clone());
        assert_eq!(
            z.entries().iter().map(strip).collect::<Vec<_>>(),
            z2.entries().iter().map(strip).collect::<Vec<_>>()
        );
        let after = z2.read_all().unwrap();
        assert_eq!(after["doc.xml"], b"<root a=\"2\"/>");
        for name in ["media/blob.bin", "notes.txt"] {
            assert_eq!(after[name], before[name], "{name}");
        }
        // Untouched entries keep their compressed size (raw copy, not recompressed).
        for (a, b) in z.entries().iter().zip(z2.entries()) {
            if a.name != "doc.xml" {
                assert_eq!(a.compressed_size, b.compressed_size, "{}", a.name);
            }
        }
    }

    #[test]
    fn replacing_unknown_entries_is_refused() {
        let Container::Zip(z) = Container::open(sample_zip()).unwrap() else {
            unreachable!()
        };
        let bad = BTreeMap::from([("nope.xml".to_owned(), Vec::new())]);
        assert!(matches!(z.write(&bad), Err(ContainerError::NoSuchEntry(_))));
    }

    #[test]
    fn truncated_archives_are_errors_not_panics() {
        let bytes = sample_zip();
        for cut in [4, 10, 30, bytes.len() / 2, bytes.len() - 1] {
            let _ = Container::open(bytes[..cut].to_vec());
        }
    }
}
