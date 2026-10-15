//! Standard base64 with padding, strict on decode (OpenFX Bytes
//! parameters on disk, owner decision P-0 Q3).

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Encodes `data`.
pub fn encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [
            chunk[0],
            *chunk.get(1).unwrap_or(&0),
            *chunk.get(2).unwrap_or(&0),
        ];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for (i, shift) in [18, 12, 6, 0].into_iter().enumerate() {
            if i <= chunk.len() {
                out.push(char::from(B64[(n >> shift) as usize & 63]));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Decodes standard base64 with padding; `None` for anything else.
pub fn decode(text: &str) -> Option<Vec<u8>> {
    let t = text.as_bytes();
    if !t.len().is_multiple_of(4) {
        return None;
    }
    let val = |c: u8| B64.iter().position(|b| *b == c).map(|p| p as u32);
    let mut out = Vec::with_capacity(t.len() / 4 * 3);
    for (k, q) in t.chunks(4).enumerate() {
        let last = k + 1 == t.len() / 4;
        let pad = q.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 || (pad > 0 && !last) {
            return None;
        }
        let mut n = 0u32;
        for (i, c) in q.iter().enumerate() {
            let v = if i >= 4 - pad { 0 } else { val(*c)? };
            n = (n << 6) | v;
        }
        let bytes = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(&bytes[..3 - pad]);
    }
    Some(out)
}
