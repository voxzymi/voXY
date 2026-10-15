//! Pasting images and file paths from the system clipboard (U-A §1).
//!
//! `Ctrl/Cmd+V` imports whatever the clipboard holds: a bitmap image (written
//! to a PNG and imported) or a list of file paths. Reading the real clipboard
//! needs a window, so the OS call lives in [`read_clipboard_imports`]; the
//! parsing and encoding it relies on are pure and tested.

use std::path::{Path, PathBuf};

/// Imports found on the clipboard, as file paths ready for the importer. A
/// pasted bitmap is written to `dir` (the project's asset directory, or a
/// temporary one) first. `Ok(empty)` means the clipboard held nothing to
/// import; `Err` means the clipboard could not be read at all.
pub fn read_clipboard_imports(dir: Option<&Path>) -> Result<Vec<PathBuf>, String> {
    let mut clipboard = arboard::Clipboard::new().map_err(|e| e.to_string())?;
    if let Ok(image) = clipboard.get_image() {
        let path = write_pasted_image(dir, image.width, image.height, &image.bytes)?;
        return Ok(vec![path]);
    }
    match clipboard.get_text() {
        Ok(text) => Ok(parse_clipboard_paths(&text)),
        // No text and no image is not an error: there is just nothing to paste.
        Err(_) => Ok(Vec::new()),
    }
}

/// Writes clipboard RGBA pixels to a fresh PNG under `dir` (or the system
/// temporary directory) and returns its path.
pub(crate) fn write_pasted_image(
    dir: Option<&Path>,
    width: usize,
    height: usize,
    rgba: &[u8],
) -> Result<PathBuf, String> {
    let (w, h) = (
        u32::try_from(width).map_err(|_| "clipboard image is too large")?,
        u32::try_from(height).map_err(|_| "clipboard image is too large")?,
    );
    let img = image::RgbaImage::from_raw(w, h, rgba.to_vec())
        .ok_or("the clipboard image data was malformed")?;
    let base = dir.map_or_else(std::env::temp_dir, Path::to_path_buf);
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let path = base.join(format!("Pasted-{stamp}.png"));
    img.save(&path).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Parses clipboard text into existing file paths: newline-separated plain
/// paths and `file://` URIs (with `%XX` decoding). Only paths that exist are
/// returned, so arbitrary pasted text imports nothing.
pub(crate) fn parse_clipboard_paths(text: &str) -> Vec<PathBuf> {
    text.lines()
        .filter_map(path_from_line)
        .filter(|p| p.exists())
        .collect()
}

fn path_from_line(line: &str) -> Option<PathBuf> {
    let s = line.trim();
    if s.is_empty() {
        return None;
    }
    let decoded = match s.strip_prefix("file://") {
        Some(rest) => percent_decode(rest),
        None => s.to_owned(),
    };
    Some(PathBuf::from(decoded))
}

/// Minimal `%XX` percent-decoding (enough for `file://` URIs); an invalid
/// escape is left as written.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_plain_paths_file_uris_and_skips_junk() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.png");
        let b = dir.path().join("my pic.png");
        std::fs::write(&a, b"a").unwrap();
        std::fs::write(&b, b"b").unwrap();

        // Plain path, a file:// URI with a percent-encoded space, a blank
        // line, and a line that is not a path at all.
        let text = format!(
            "{}\nfile://{}\n\nnot a path at all\n",
            a.display(),
            b.display().to_string().replace(' ', "%20"),
        );
        let got = parse_clipboard_paths(&text);
        assert_eq!(got, vec![a, b]);
    }

    #[test]
    fn arbitrary_text_imports_nothing() {
        assert_eq!(
            parse_clipboard_paths("just some copied words\nand more"),
            Vec::<PathBuf>::new()
        );
    }

    #[test]
    fn writes_a_pasted_image_as_png() {
        let dir = tempfile::tempdir().unwrap();
        let rgba = vec![255u8; 2 * 2 * 4];
        let path = write_pasted_image(Some(dir.path()), 2, 2, &rgba).unwrap();
        assert!(path.exists());
        assert_eq!(path.extension().unwrap(), "png");
        assert_eq!(image::image_dimensions(&path).unwrap(), (2, 2));

        // Malformed data (too few bytes) is a clean error, not a panic.
        assert!(write_pasted_image(Some(dir.path()), 4, 4, &[0, 0, 0]).is_err());
    }
}
