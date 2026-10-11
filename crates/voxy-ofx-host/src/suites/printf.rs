//! A C `printf` formatter for the message suite's variadic arguments.
//!
//! Written here rather than calling the C library's `vsnprintf`, whose
//! symbol is not exported by the Windows runtime. It covers the C99
//! conversions (`d i u o x X c s p f F e E g G %`) with flags, width,
//! precision and length modifiers. `%n` is ignored (it would write into
//! the plugin's memory). `%a`, `%A`, `%L…` (long double) and anything
//! unknown stop the formatting, because the argument size is not known and
//! reading on could misread every later argument; the text so far is kept
//! and `…` is appended.

use std::ffi::{CStr, VaList, c_char, c_int, c_long, c_longlong, c_uint, c_ulong, c_ulonglong};
use std::fmt::Write as _;

/// The C flags, width and precision of one conversion.
#[allow(clippy::struct_excessive_bools)]
#[derive(Debug, Default, Clone, Copy)]
struct Spec {
    left: bool,
    plus: bool,
    space: bool,
    alt: bool,
    zero: bool,
    width: usize,
    precision: Option<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Len {
    Default,
    Char,
    Short,
    Long,
    LongLong,
    Size,
    Max,
    Ptrdiff,
}

/// Formats `fmt` with the arguments in `args`.
///
/// # Safety
/// `args` holds arguments matching `fmt`, as C requires of the caller.
pub(super) unsafe fn format(fmt: &CStr, args: &mut VaList<'_>) -> String {
    let b = fmt.to_bytes();
    let mut out = String::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] != b'%' {
            let start = i;
            while i < b.len() && b[i] != b'%' {
                i += 1;
            }
            out.push_str(&String::from_utf8_lossy(&b[start..i]));
            continue;
        }
        i += 1;
        let mut spec = Spec::default();
        while let Some(&c) = b.get(i) {
            match c {
                b'-' => spec.left = true,
                b'+' => spec.plus = true,
                b' ' => spec.space = true,
                b'#' => spec.alt = true,
                b'0' => spec.zero = true,
                _ => break,
            }
            i += 1;
        }
        if b.get(i) == Some(&b'*') {
            // SAFETY: the caller's contract: a `*` width is an int.
            let w = unsafe { args.next_arg::<c_int>() };
            if w < 0 {
                spec.left = true;
            }
            spec.width = w.unsigned_abs() as usize;
            i += 1;
        } else {
            while let Some(d) = b.get(i).filter(|c| c.is_ascii_digit()) {
                spec.width = spec.width * 10 + usize::from(d - b'0');
                i += 1;
            }
        }
        if b.get(i) == Some(&b'.') {
            i += 1;
            if b.get(i) == Some(&b'*') {
                // SAFETY: as above.
                let p = unsafe { args.next_arg::<c_int>() };
                spec.precision = usize::try_from(p).ok();
                i += 1;
            } else {
                let mut p = 0;
                while let Some(d) = b.get(i).filter(|c| c.is_ascii_digit()) {
                    p = p * 10 + usize::from(d - b'0');
                    i += 1;
                }
                spec.precision = Some(p);
            }
        }
        let mut len = Len::Default;
        loop {
            len = match (b.get(i), len) {
                (Some(b'h'), Len::Short) => Len::Char,
                (Some(b'h'), _) => Len::Short,
                (Some(b'l'), Len::Long) => Len::LongLong,
                (Some(b'l'), _) => Len::Long,
                (Some(b'z'), _) => Len::Size,
                (Some(b'j'), _) => Len::Max,
                (Some(b't'), _) => Len::Ptrdiff,
                _ => break,
            };
            i += 1;
        }
        let Some(&conv) = b.get(i) else {
            break;
        };
        i += 1;
        // SAFETY: the caller's contract: each conversion has its argument.
        let piece = unsafe { conversion(conv, len, spec, args) };
        if let Some(piece) = piece {
            out.push_str(&piece);
        } else {
            out.push('…');
            break;
        }
    }
    out
}

/// One conversion, or `None` when it cannot be read safely.
///
/// # Safety
/// The next argument in `args` has the type `conv` and `len` name.
unsafe fn conversion(conv: u8, len: Len, spec: Spec, args: &mut VaList<'_>) -> Option<String> {
    // SAFETY (whole body): the caller's contract.
    unsafe {
        Some(match conv {
            b'%' => "%".to_owned(),
            b'd' | b'i' => {
                let v: i64 = match len {
                    Len::Default => i64::from(args.next_arg::<c_int>()),
                    Len::Char => i64::from(args.next_arg::<c_int>() as i8),
                    Len::Short => i64::from(args.next_arg::<c_int>() as i16),
                    // `long` is 32 bits on Windows, 64 elsewhere.
                    #[allow(clippy::useless_conversion)]
                    Len::Long => i64::from(args.next_arg::<c_long>()),
                    Len::LongLong | Len::Max => args.next_arg::<c_longlong>(),
                    Len::Size | Len::Ptrdiff => args.next_arg::<isize>() as i64,
                };
                let sign = if v < 0 {
                    "-"
                } else if spec.plus {
                    "+"
                } else if spec.space {
                    " "
                } else {
                    ""
                };
                integer(sign, &v.unsigned_abs().to_string(), "", spec)
            }
            b'u' | b'o' | b'x' | b'X' => {
                let v: u64 = match len {
                    Len::Default => u64::from(args.next_arg::<c_uint>()),
                    Len::Char => u64::from(args.next_arg::<c_uint>() as u8),
                    Len::Short => u64::from(args.next_arg::<c_uint>() as u16),
                    #[allow(clippy::useless_conversion)]
                    Len::Long => u64::from(args.next_arg::<c_ulong>()),
                    Len::LongLong | Len::Max => args.next_arg::<c_ulonglong>(),
                    Len::Size | Len::Ptrdiff => args.next_arg::<usize>() as u64,
                };
                let (digits, prefix) = match conv {
                    b'o' => (format!("{v:o}"), if spec.alt { "0" } else { "" }),
                    b'x' => (format!("{v:x}"), if spec.alt && v != 0 { "0x" } else { "" }),
                    b'X' => (format!("{v:X}"), if spec.alt && v != 0 { "0X" } else { "" }),
                    _ => (v.to_string(), ""),
                };
                integer("", &digits, prefix, spec)
            }
            b'c' => pad(
                char::from(args.next_arg::<c_int>() as u8).to_string(),
                spec,
                false,
            ),
            b's' => {
                let p = args.next_arg::<*const c_char>();
                let s = if p.is_null() {
                    "(null)".to_owned()
                } else {
                    let bytes = CStr::from_ptr(p).to_bytes();
                    let bytes = match spec.precision {
                        Some(n) => &bytes[..n.min(bytes.len())],
                        None => bytes,
                    };
                    String::from_utf8_lossy(bytes).into_owned()
                };
                pad(s, spec, false)
            }
            b'p' => {
                let p = args.next_arg::<*const std::ffi::c_void>();
                pad(format!("{p:p}"), spec, false)
            }
            b'n' => {
                // Skip the pointer argument; never write through it.
                let _ = args.next_arg::<*mut c_int>();
                String::new()
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' if len != Len::Default && len != Len::Long => {
                return None;
            }
            b'f' | b'F' | b'e' | b'E' | b'g' | b'G' => float(conv, args.next_arg::<f64>(), spec),
            _ => return None,
        })
    }
}

/// Pads to the width: spaces left or right, or zeros after the sign.
fn pad(s: String, spec: Spec, numeric: bool) -> String {
    let n = s.chars().count();
    if n >= spec.width {
        return s;
    }
    let fill = spec.width - n;
    if spec.left {
        format!("{s}{}", " ".repeat(fill))
    } else if spec.zero && numeric {
        let split = s
            .find(|c: char| c.is_ascii_digit() || c.is_ascii_alphabetic())
            .unwrap_or(0);
        let (head, tail) = s.split_at(split);
        // Keep a 0x prefix in front of the zeros.
        let (head, tail) = if tail.starts_with("0x") || tail.starts_with("0X") {
            s.split_at(split + 2)
        } else {
            (head, tail)
        };
        format!("{head}{}{tail}", "0".repeat(fill))
    } else {
        format!("{}{s}", " ".repeat(fill))
    }
}

fn integer(sign: &str, digits: &str, prefix: &str, spec: Spec) -> String {
    let digits = match spec.precision {
        Some(0) if digits == "0" => String::new(),
        Some(p) if digits.len() < p => format!("{}{digits}", "0".repeat(p - digits.len())),
        _ => digits.to_owned(),
    };
    let zero = spec.zero && spec.precision.is_none() && !spec.left;
    pad(
        format!("{sign}{prefix}{digits}"),
        Spec { zero, ..spec },
        true,
    )
}

/// `%e` in C style: at least two exponent digits with a sign.
fn exp_style(v: f64, prec: usize, upper: bool) -> String {
    let s = format!("{v:.prec$e}");
    let (mant, exp) = s.split_once('e').unwrap_or((&s, "0"));
    let exp: i32 = exp.parse().unwrap_or(0);
    let e = if upper { 'E' } else { 'e' };
    let sign = if exp < 0 { '-' } else { '+' };
    format!("{mant}{e}{sign}{:02}", exp.unsigned_abs())
}

fn float(conv: u8, value: f64, spec: Spec) -> String {
    let upper = conv.is_ascii_uppercase();
    let body = if value.is_nan() {
        "nan".to_owned()
    } else if value.is_infinite() {
        "inf".to_owned()
    } else {
        let mag = value.abs();
        let prec = spec.precision.unwrap_or(6);
        match conv.to_ascii_lowercase() {
            b'f' => format!("{mag:.prec$}"),
            b'e' => exp_style(mag, prec, upper),
            _ => {
                // %g: precision is significant digits; pick the style the
                // exponent asks for, then drop trailing zeros unless `#`.
                let p = prec.max(1);
                let exp = if mag == 0.0 {
                    0
                } else {
                    let e = exp_style(mag, p - 1, false);
                    e.rsplit_once('e')
                        .and_then(|(_, x)| x.parse::<i32>().ok())
                        .unwrap_or(0)
                };
                let p_i = i32::try_from(p).unwrap_or(i32::MAX);
                let mut s = if exp < -4 || exp >= p_i {
                    exp_style(mag, p - 1, upper)
                } else {
                    let decimals = usize::try_from(p_i - 1 - exp).unwrap_or(0);
                    format!("{mag:.decimals$}")
                };
                if !spec.alt {
                    let (mant, tail) = match s.find(['e', 'E']) {
                        Some(k) => (s[..k].to_owned(), s[k..].to_owned()),
                        None => (s.clone(), String::new()),
                    };
                    let mant = if mant.contains('.') {
                        mant.trim_end_matches('0').trim_end_matches('.').to_owned()
                    } else {
                        mant
                    };
                    s = mant + &tail;
                }
                s
            }
        }
    };
    let body = if upper { body.to_uppercase() } else { body };
    let sign = if value.is_sign_negative() && !value.is_nan() {
        "-"
    } else if spec.plus {
        "+"
    } else if spec.space {
        " "
    } else {
        ""
    };
    let mut s = String::new();
    let _ = write!(s, "{sign}{body}");
    pad(
        s,
        Spec {
            zero: spec.zero && !spec.left && value.is_finite(),
            ..spec
        },
        true,
    )
}
