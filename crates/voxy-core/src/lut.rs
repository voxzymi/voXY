//! 3D colour lookup tables in the `.cube` text format (Adobe/Resolve), for
//! the LUT effect. Pure parsing: reading the file is the caller's job.
//!
//! Supported: `LUT_3D_SIZE` (2 to [`MAX_SIZE`]), `DOMAIN_MIN`/`DOMAIN_MAX`,
//! `TITLE`, `#` comments and blank lines. Entries are `r g b` with red
//! changing fastest, then green, then blue. 1D tables (`LUT_1D_SIZE`) are
//! refused with a clear reason. A damaged file is an error, never a panic.

/// The largest table side accepted (129³ entries is about 26 MB as `f32`).
pub const MAX_SIZE: usize = 129;

/// A parsed 3D LUT.
#[derive(Debug, Clone, PartialEq)]
pub struct CubeLut {
    pub title: Option<String>,
    /// Entries per side.
    pub size: usize,
    /// The input range mapped onto the table, per channel.
    pub domain_min: [f32; 3],
    pub domain_max: [f32; 3],
    /// `size³` entries, red fastest.
    pub data: Vec<[f32; 3]>,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum LutError {
    #[error("line {line}: {why}")]
    Line { line: usize, why: String },
    #[error("no LUT_3D_SIZE line")]
    NoSize,
    #[error("1D LUTs are not supported yet; use a 3D .cube file")]
    OneDimensional,
    #[error("the table size must be 2 to {MAX_SIZE}, not {0}")]
    BadSize(usize),
    #[error("expected {want} entries, found {got}")]
    Count { want: usize, got: usize },
    #[error("DOMAIN_MIN must be below DOMAIN_MAX in every channel")]
    BadDomain,
}

impl CubeLut {
    /// The entry at grid position `(r, g, b)`.
    pub fn at(&self, r: usize, g: usize, b: usize) -> [f32; 3] {
        self.data[r + self.size * (g + self.size * b)]
    }

    /// The table at `rgb`, trilinearly interpolated, with inputs clamped to
    /// the domain. The reference for the LUT effect's shader.
    pub fn apply(&self, rgb: [f32; 3]) -> [f32; 3] {
        let last = (self.size - 1) as f32;
        let mut i = [0usize; 3];
        let mut f = [0.0f32; 3];
        for c in 0..3 {
            let span = self.domain_max[c] - self.domain_min[c];
            let t = ((rgb[c] - self.domain_min[c]) / span).clamp(0.0, 1.0) * last;
            let t = if t.is_finite() { t } else { 0.0 };
            i[c] = (t.floor() as usize).min(self.size - 2);
            f[c] = t - i[c] as f32;
        }
        let mut out = [0.0f32; 3];
        for corner in 0..8 {
            let (dr, dg, db) = (corner & 1, (corner >> 1) & 1, (corner >> 2) & 1);
            let w = [dr, dg, db]
                .iter()
                .zip(f)
                .map(|(d, f)| if *d == 1 { f } else { 1.0 - f })
                .product::<f32>();
            let v = self.at(i[0] + dr, i[1] + dg, i[2] + db);
            for c in 0..3 {
                out[c] += w * v[c];
            }
        }
        out
    }

    /// The identity table of `size` entries per side.
    pub fn identity(size: usize) -> Self {
        let n = size.clamp(2, MAX_SIZE);
        let step = 1.0 / (n - 1) as f32;
        let mut data = Vec::with_capacity(n * n * n);
        for b in 0..n {
            for g in 0..n {
                for r in 0..n {
                    data.push([r as f32 * step, g as f32 * step, b as f32 * step]);
                }
            }
        }
        CubeLut {
            title: None,
            size: n,
            domain_min: [0.0; 3],
            domain_max: [1.0; 3],
            data,
        }
    }
}

fn floats<const N: usize>(rest: &str, line: usize) -> Result<[f32; N], LutError> {
    let mut out = [0.0f32; N];
    let mut it = rest.split_whitespace();
    for v in &mut out {
        let t = it.next().ok_or_else(|| LutError::Line {
            line,
            why: format!("expected {N} numbers"),
        })?;
        *v = t
            .parse::<f32>()
            .ok()
            .filter(|x| x.is_finite())
            .ok_or_else(|| LutError::Line {
                line,
                why: format!("`{t}` is not a finite number"),
            })?;
    }
    if it.next().is_some() {
        return Err(LutError::Line {
            line,
            why: format!("expected {N} numbers"),
        });
    }
    Ok(out)
}

/// Parses a `.cube` file's text.
pub fn parse_cube(text: &str) -> Result<CubeLut, LutError> {
    let mut title = None;
    let mut size = None;
    let mut domain_min = [0.0f32; 3];
    let mut domain_max = [1.0f32; 3];
    let mut data: Vec<[f32; 3]> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        let l = raw.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let (word, rest) = l.split_once(char::is_whitespace).unwrap_or((l, ""));
        match word {
            "TITLE" => {
                title = Some(rest.trim().trim_matches('"').to_owned());
            }
            "LUT_1D_SIZE" => return Err(LutError::OneDimensional),
            "LUT_3D_SIZE" => {
                let n: usize = rest.trim().parse().map_err(|_| LutError::Line {
                    line,
                    why: "the size is not a whole number".into(),
                })?;
                if !(2..=MAX_SIZE).contains(&n) {
                    return Err(LutError::BadSize(n));
                }
                data.reserve_exact(n * n * n);
                size = Some(n);
            }
            "DOMAIN_MIN" => domain_min = floats::<3>(rest, line)?,
            "DOMAIN_MAX" => domain_max = floats::<3>(rest, line)?,
            // Other keywords (e.g. LUT_IN_VIDEO_RANGE) are ignored.
            w if w.chars().next().is_some_and(char::is_alphabetic) => {}
            _ => {
                let n = size.ok_or(LutError::NoSize)?;
                if data.len() == n * n * n {
                    return Err(LutError::Line {
                        line,
                        why: "more entries than the table holds".into(),
                    });
                }
                data.push(floats::<3>(l, line)?);
            }
        }
    }
    let size = size.ok_or(LutError::NoSize)?;
    let want = size * size * size;
    if data.len() != want {
        return Err(LutError::Count {
            want,
            got: data.len(),
        });
    }
    if (0..3).any(|c| domain_min[c] >= domain_max[c]) {
        return Err(LutError::BadDomain);
    }
    Ok(CubeLut {
        title,
        size,
        domain_min,
        domain_max,
        data,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(size: usize, body: &str) -> String {
        format!("# a comment\nTITLE \"Test\"\nLUT_3D_SIZE {size}\n\n{body}")
    }

    #[test]
    fn parses_a_small_table_red_fastest() {
        let body = "0 0 0\n1 0 0\n0 1 0\n1 1 0\n0 0 1\n1 0 1\n0 1 1\n1 1 1\n";
        let lut = parse_cube(&cube(2, body)).unwrap();
        assert_eq!(lut.title.as_deref(), Some("Test"));
        assert_eq!(lut.size, 2);
        assert_eq!(lut.at(1, 0, 0), [1.0, 0.0, 0.0]);
        assert_eq!(lut.at(0, 1, 1), [0.0, 1.0, 1.0]);
        assert_eq!(
            lut,
            CubeLut {
                title: Some("Test".into()),
                ..CubeLut::identity(2)
            }
        );
    }

    #[test]
    fn apply_interpolates_and_clamps_to_the_domain() {
        let id = CubeLut::identity(5);
        for v in [[0.0, 0.5, 1.0], [0.13, 0.77, 0.4]] {
            let out = id.apply(v);
            for c in 0..3 {
                assert!((out[c] - v[c]).abs() < 1e-6, "{out:?}");
            }
        }
        assert_eq!(id.apply([-1.0, 2.0, f32::NAN]), [0.0, 1.0, 0.0]);
        // Swap red and blue.
        let mut swap = CubeLut::identity(3);
        for e in &mut swap.data {
            e.swap(0, 2);
        }
        let out = swap.apply([0.2, 0.6, 0.9]);
        assert!((out[0] - 0.9).abs() < 1e-6 && (out[2] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn damaged_files_are_errors() {
        let eight = "0 0 0\n".repeat(8);
        assert_eq!(
            parse_cube(&cube(2, &"0 0 0\n".repeat(7))),
            Err(LutError::Count { want: 8, got: 7 })
        );
        assert!(matches!(
            parse_cube(&cube(2, &format!("{eight}0 0 0\n"))),
            Err(LutError::Line { .. })
        ));
        assert!(matches!(
            parse_cube(&cube(2, &eight.replacen("0 0 0", "0 nan 0", 1))),
            Err(LutError::Line { .. })
        ));
        assert!(matches!(
            parse_cube(&cube(2, &eight.replacen("0 0 0", "0 0", 1))),
            Err(LutError::Line { .. })
        ));
        assert_eq!(parse_cube(&cube(1, "")), Err(LutError::BadSize(1)));
        assert_eq!(parse_cube(&cube(400, "")), Err(LutError::BadSize(400)));
        assert_eq!(parse_cube("0 0 0\n"), Err(LutError::NoSize));
        assert_eq!(parse_cube("LUT_1D_SIZE 4\n"), Err(LutError::OneDimensional));
        assert_eq!(
            parse_cube(&format!(
                "DOMAIN_MIN 0 0 1\nDOMAIN_MAX 1 1 1\n{}",
                cube(2, &eight)
            )),
            Err(LutError::BadDomain)
        );
        // Arbitrary bytes never panic.
        for junk in ["", "LUT_3D_SIZE", "LUT_3D_SIZE -3", "\u{0}\u{1}", "TITLE"] {
            let _ = parse_cube(junk);
        }
    }
}
