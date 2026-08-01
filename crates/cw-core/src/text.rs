//! Deciding whether a payload is text, and turning it into a `String`.
//!
//! Not a port. The reference's viewers pick by extension and hand the bytes to
//! `Encoding.UTF8.GetString`, which is why opening the wrong file in CodeWalker
//! shows a screen of replacement characters. The Explorer sniffs instead,
//! because `.dat` in the archives is sometimes an INI file and sometimes a
//! compiled table, and the extension does not say which.

use serde::Serialize;

/// How many bytes the sniff looks at. Enough to catch a binary header without
/// walking a 20 MB texture dictionary.
const SNIFF_LEN: usize = 8192;

/// Above this, the text viewer would be unusable anyway and the hex viewer is
/// the right tool. Decoding still happens, truncated at a char boundary.
pub const MAX_TEXT_BYTES: usize = 4 * 1024 * 1024;

/// The encoding a payload was decoded from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Encoding {
    /// No byte-order mark; assumed UTF-8, which subsumes the ASCII that most
    /// game text actually is.
    Utf8,
    Utf8Bom,
    Utf16Le,
    Utf16Be,
}

impl Encoding {
    pub fn label(self) -> &'static str {
        match self {
            Encoding::Utf8 => "UTF-8",
            Encoding::Utf8Bom => "UTF-8 with BOM",
            Encoding::Utf16Le => "UTF-16 LE",
            Encoding::Utf16Be => "UTF-16 BE",
        }
    }
}

/// A decoded payload.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TextFile {
    pub text: String,
    pub encoding: Encoding,
    /// True when at least one byte sequence was invalid and became U+FFFD.
    /// Editing and saving such a file would not round-trip, so the UI says so.
    pub lossy: bool,
    /// True when the payload was longer than [`MAX_TEXT_BYTES`].
    pub truncated: bool,
    /// Byte length of the whole payload, truncated or not.
    pub total_bytes: usize,
}

/// The byte-order mark at the start of `data`, if any.
fn bom(data: &[u8]) -> Option<(Encoding, usize)> {
    if data.starts_with(&[0xEF, 0xBB, 0xBF]) {
        return Some((Encoding::Utf8Bom, 3));
    }
    // Order matters: FF FE 00 00 would be UTF-32 LE, which nothing here ships,
    // so the UTF-16 test is safe to make first.
    if data.starts_with(&[0xFF, 0xFE]) {
        return Some((Encoding::Utf16Le, 2));
    }
    if data.starts_with(&[0xFE, 0xFF]) {
        return Some((Encoding::Utf16Be, 2));
    }
    None
}

/// Whether a payload reads as text.
///
/// A byte-order mark settles it. Otherwise the heuristic is the usual one: a NUL
/// means binary, and so does more than a small fraction of other control bytes.
/// Empty input is text — an empty file should open in the editor, not the hex
/// view.
pub fn is_text(data: &[u8]) -> bool {
    if data.is_empty() {
        return true;
    }
    if bom(data).is_some() {
        return true;
    }

    let sample = &data[..data.len().min(SNIFF_LEN)];
    if sample.contains(&0) {
        return false;
    }

    let control = sample
        .iter()
        .filter(|b| **b < 0x20 && !matches!(b, b'\t' | b'\n' | b'\r' | 0x0C))
        .count();
    // Under 2%. Tight enough that a table of small integers reads as binary,
    // loose enough that a stray 0x1A in a config file does not.
    if control * 50 >= sample.len() {
        return false;
    }

    // Bytes above 0x7F are neither NUL nor control, so a buffer of packed
    // floats or a compressed block can walk past both checks. Requiring the
    // sample to decode as UTF-8 catches it. A sequence cut in half by the
    // sample boundary is the sample's fault, not the file's.
    match std::str::from_utf8(sample) {
        Ok(_) => true,
        Err(e) => e.error_len().is_none() && sample.len() == SNIFF_LEN,
    }
}

/// Decode a payload, whatever it turns out to be. The caller decides whether to
/// show the result — see [`is_text`].
pub fn decode(data: &[u8]) -> TextFile {
    let total_bytes = data.len();
    let (encoding, start) = bom(data).unwrap_or((Encoding::Utf8, 0));
    let body = &data[start..];

    let truncated = body.len() > MAX_TEXT_BYTES;
    let body = &body[..body.len().min(MAX_TEXT_BYTES)];

    let (text, lossy) = match encoding {
        Encoding::Utf8 | Encoding::Utf8Bom => match std::str::from_utf8(body) {
            Ok(s) => (s.to_string(), false),
            // Cutting at a byte count can land mid-character. An incomplete
            // sequence at the very end of a truncated body is the cut, not a
            // defect in the file, so drop it without calling the file lossy.
            Err(e) if truncated && e.error_len().is_none() => (
                String::from_utf8_lossy(&body[..e.valid_up_to()]).into_owned(),
                false,
            ),
            Err(_) => (String::from_utf8_lossy(body).into_owned(), true),
        },
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let big_endian = encoding == Encoding::Utf16Be;
            let units: Vec<u16> = body
                .chunks_exact(2)
                .map(|c| {
                    if big_endian {
                        u16::from_be_bytes([c[0], c[1]])
                    } else {
                        u16::from_le_bytes([c[0], c[1]])
                    }
                })
                .collect();
            match String::from_utf16(&units) {
                Ok(s) => (s, !body.len().is_multiple_of(2)),
                Err(_) => (String::from_utf16_lossy(&units), true),
            }
        }
    };

    TextFile {
        text,
        encoding,
        lossy,
        truncated,
        total_bytes,
    }
}

/// Encode edited text back to bytes in the same encoding it was read as, so
/// saving a file that came in as UTF-16 does not silently convert it.
pub fn encode(text: &str, encoding: Encoding) -> Vec<u8> {
    match encoding {
        Encoding::Utf8 => text.as_bytes().to_vec(),
        Encoding::Utf8Bom => {
            let mut out = vec![0xEF, 0xBB, 0xBF];
            out.extend_from_slice(text.as_bytes());
            out
        }
        Encoding::Utf16Le | Encoding::Utf16Be => {
            let big_endian = encoding == Encoding::Utf16Be;
            let mut out = if big_endian {
                vec![0xFE, 0xFF]
            } else {
                vec![0xFF, 0xFE]
            };
            for unit in text.encode_utf16() {
                out.extend_from_slice(&if big_endian {
                    unit.to_be_bytes()
                } else {
                    unit.to_le_bytes()
                });
            }
            out
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_plain_ascii_as_text() {
        assert!(is_text(
            b"<CVehicleModelInfo>\n  <handlingId>ADDER</handlingId>\n"
        ));
        assert!(is_text(b""));
    }

    /// A resource payload starts with a vtable pointer and pointer fields, so
    /// NUL bytes appear immediately. That is the cheapest possible signal.
    #[test]
    fn recognises_a_resource_payload_as_binary() {
        let mut payload = vec![0x18, 0x2E, 0x62, 0x40];
        payload.extend_from_slice(&[0u8; 60]);
        assert!(!is_text(&payload));
    }

    #[test]
    fn treats_a_wall_of_control_bytes_as_binary() {
        let data: Vec<u8> = (0..500)
            .map(|i| if i % 3 == 0 { 0x01 } else { b'a' })
            .collect();
        assert!(!is_text(&data));
    }

    /// High bytes are neither NUL nor control, so the first two checks pass a
    /// buffer of packed floats. Only the UTF-8 check rejects it.
    #[test]
    fn treats_invalid_utf8_as_binary() {
        assert!(!is_text(&[0xFFu8; 64]));
        let floats: Vec<u8> = (0..256).flat_map(|i| (i as f32).to_le_bytes()).collect();
        assert!(!is_text(&floats));
    }

    #[test]
    fn accepts_valid_utf8_above_ascii() {
        assert!(is_text("año, ñandú, 日本語".as_bytes()));
    }

    /// A multi-byte character straddling the 8 KB sniff boundary must not make
    /// a text file look binary.
    #[test]
    fn tolerates_a_character_cut_by_the_sniff_boundary() {
        let mut data = vec![b'a'; SNIFF_LEN - 1];
        data.extend_from_slice("ñ".as_bytes());
        data.extend(std::iter::repeat_n(b'a', 100));
        assert!(is_text(&data));
    }

    #[test]
    fn reads_a_utf8_bom() {
        let mut data = vec![0xEF, 0xBB, 0xBF];
        data.extend_from_slice("hola".as_bytes());
        let decoded = decode(&data);
        assert_eq!(decoded.encoding, Encoding::Utf8Bom);
        assert_eq!(decoded.text, "hola");
        assert!(!decoded.lossy);
        assert_eq!(decoded.total_bytes, 7);
    }

    #[test]
    fn reads_utf16_in_both_orders() {
        let le = encode("año", Encoding::Utf16Le);
        assert!(is_text(&le), "a BOM makes it text even with NUL bytes");
        let decoded = decode(&le);
        assert_eq!(decoded.encoding, Encoding::Utf16Le);
        assert_eq!(decoded.text, "año");

        let be = encode("año", Encoding::Utf16Be);
        let decoded = decode(&be);
        assert_eq!(decoded.encoding, Encoding::Utf16Be);
        assert_eq!(decoded.text, "año");
    }

    /// Invalid UTF-8 decodes rather than failing, but is flagged: editing and
    /// saving it would not reproduce the original bytes.
    #[test]
    fn flags_a_lossy_decode() {
        let decoded = decode(&[b'o', b'k', 0xC3, 0x28]);
        assert!(decoded.lossy);
        assert!(decoded.text.contains('\u{FFFD}'));
    }

    #[test]
    fn round_trips_every_encoding() {
        for encoding in [
            Encoding::Utf8,
            Encoding::Utf8Bom,
            Encoding::Utf16Le,
            Encoding::Utf16Be,
        ] {
            let bytes = encode("línea 1\nlínea 2\n", encoding);
            let decoded = decode(&bytes);
            assert_eq!(decoded.encoding, encoding, "{}", encoding.label());
            assert_eq!(decoded.text, "línea 1\nlínea 2\n");
            assert!(!decoded.lossy);
        }
    }

    #[test]
    fn truncates_an_oversized_payload() {
        let data = vec![b'x'; MAX_TEXT_BYTES + 10];
        let decoded = decode(&data);
        assert!(decoded.truncated);
        assert_eq!(decoded.text.len(), MAX_TEXT_BYTES);
        assert_eq!(decoded.total_bytes, MAX_TEXT_BYTES + 10);
    }
}
