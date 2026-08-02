//! GXT2 text tables: the format that names the game's UI strings.
//!
//! Port of `Gxt2File` from
//! `CodeWalker.Core/GameFiles/FileTypes/Gxt2File.cs`.
//!
//! The layout is two halves: a header and entry table (`GXT2`, count, then one
//! `{hash, offset}` per entry), followed by a second `GXT2` magic, the byte
//! length of the whole text blob, and the NUL-terminated strings the offsets
//! point into. Offsets are absolute within the payload.
//!
//! Unlike the resource formats, GXT2 round-trips byte-for-byte: the reference
//! has a full `Save`, and this port has a full `write`, so the format is
//! **done** under this doc's rule rather than partial.

use serde::Serialize;

use crate::Result;

/// The `GXT2` magic, as a little-endian `u32`.
pub const GXT2_MAGIC: u32 = 1196971058;

/// One entry: a hash and the string it resolves to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gxt2Entry {
    /// Jenkins hash of the text, which is how the game looks strings up.
    pub hash: u32,
    /// The UTF-8 text, without the trailing NUL.
    pub text: String,
}

/// A parsed GXT2 table.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Gxt2File {
    pub entries: Vec<Gxt2Entry>,
}

impl Gxt2File {
    /// Parse a `.gxt2` payload.
    ///
    /// The reference returns early — leaving the fields at their defaults —
    /// when either `GXT2` magic is missing, so a wrong file quietly reads as
    /// empty there. Here the first magic mismatch is an error, because a file
    /// the extension calls GXT2 that does not start with `GXT2` is not a GXT2
    /// file and a viewer that shows nothing would look like a bug.
    ///
    /// One reference quirk is kept: the second magic is checked but its value
    /// is not used — the entry offsets are absolute, not relative to it.
    pub fn read(data: &[u8]) -> Result<Self> {
        let mut r = Reader::new(data);

        let magic = r.read_u32()?;
        if magic != GXT2_MAGIC {
            return Err(crate::CwError::Invalid {
                what: "gxt2",
                detail: format!("first magic is {magic:#x}, expected GXT2"),
            });
        }

        let entry_count = r.read_u32()? as usize;
        let mut hashes = Vec::with_capacity(entry_count);
        let mut offsets = Vec::with_capacity(entry_count);
        for _ in 0..entry_count {
            hashes.push(r.read_u32()?);
            offsets.push(r.read_u32()? as usize);
        }

        let magic2 = r.read_u32()?;
        if magic2 != GXT2_MAGIC {
            return Err(crate::CwError::Invalid {
                what: "gxt2",
                detail: format!("second magic is {magic2:#x}, expected GXT2"),
            });
        }
        let end = r.read_u32()? as usize;

        let mut entries = Vec::with_capacity(entry_count);
        for (hash, offset) in hashes.into_iter().zip(offsets) {
            let text = read_nul_string(data, offset, end)?;
            entries.push(Gxt2Entry { hash, text });
        }

        Ok(Self { entries })
    }

    /// Serialize the table back to GXT2 bytes.
    ///
    /// Layout mirrors `Save`: the entry table's offsets are filled as the
    /// strings are laid out after the second header, so the output is
    /// byte-for-byte what the reference writes for the same entries.
    pub fn write(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&GXT2_MAGIC.to_le_bytes());
        out.extend_from_slice(&(self.entries.len() as u32).to_le_bytes());

        let mut offset = 16u32 + (self.entries.len() as u32) * 8;
        let mut bodies: Vec<Vec<u8>> = Vec::with_capacity(self.entries.len());
        for entry in &self.entries {
            out.extend_from_slice(&entry.hash.to_le_bytes());
            out.extend_from_slice(&offset.to_le_bytes());
            let mut body = entry.text.as_bytes().to_vec();
            body.push(0);
            offset += body.len() as u32;
            bodies.push(body);
        }

        out.extend_from_slice(&GXT2_MAGIC.to_le_bytes());
        out.extend_from_slice(&offset.to_le_bytes());
        for body in bodies {
            out.extend_from_slice(&body);
        }
        out
    }

    /// Render the table the way `ToText` does: `0xHHHHHHHH = text` per line.
    ///
    /// The reverse, [`from_text`], is the companion: it parses these lines back
    /// into a table, so text can be edited and re-serialized.
    pub fn to_text(&self) -> String {
        let mut sb = String::new();
        for entry in &self.entries {
            sb.push_str(&format!("0x{:08X} = {}\n", entry.hash, entry.text));
        }
        sb
    }
}

/// Parse `ToText` output back into a table.
///
/// Port of `Gxt2File.FromText`: lines shorter than `0x00000000 = ` are skipped,
/// the eight hex digits after `0x` are the hash, and everything after the `= `
/// is the text. Entries are sorted by hash, as the reference sorts them.
pub fn from_text(text: &str) -> Gxt2File {
    let mut entries: Vec<Gxt2Entry> = Vec::new();
    for line in text.split('\n') {
        let line = line.trim();
        if line.len() < 13 {
            continue;
        }
        if let Some(rest) = line.strip_prefix("0x") {
            if rest.len() >= 8 {
                if let Ok(hash) = u32::from_str_radix(&rest[..8], 16) {
                    let text = rest.get(8..).and_then(|s| s.strip_prefix(" = ")).unwrap_or("");
                    entries.push(Gxt2Entry {
                        hash,
                        text: text.to_string(),
                    });
                }
            }
        }
    }
    entries.sort_by_key(|e| e.hash);
    Gxt2File { entries }
}

/// A NUL-terminated UTF-8 string at `offset`, bounded by `end`.
fn read_nul_string(data: &[u8], offset: usize, end: usize) -> Result<String> {
    let start = offset.min(data.len());
    let stop = end.min(data.len());
    if start >= stop {
        return Ok(String::new());
    }
    let slice = &data[start..stop];
    let len = slice.iter().position(|b| *b == 0).unwrap_or(slice.len());
    // The reference uses `Encoding.UTF8.GetString`, which replaces invalid
    // sequences rather than failing, so decode lossily the same way.
    Ok(String::from_utf8_lossy(&slice[..len]).into_owned())
}

/// A cursor over a little-endian byte slice.
struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    fn read_u32(&mut self) -> Result<u32> {
        let bytes = self
            .data
            .get(self.pos..self.pos + 4)
            .ok_or_else(|| crate::CwError::Invalid {
                what: "gxt2",
                detail: format!("read past the end at offset {:#x}", self.pos),
            })?;
        self.pos += 4;
        Ok(u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The format must round-trip byte-for-byte: parse → serialize with no
    /// edits reproduces the input exactly. That is the definition of **done**
    /// in `docs/formats.md`.
    #[test]
    fn round_trips_byte_for_byte() {
        let original = Gxt2File {
            entries: vec![
                Gxt2Entry { hash: 0x11111111, text: "hello".into() },
                Gxt2Entry { hash: 0x22222222, text: "world".into() },
                Gxt2Entry { hash: 0x33333333, text: "".into() },
            ],
        };
        let bytes = original.write();
        let parsed = Gxt2File::read(&bytes).expect("round-trip read");
        assert_eq!(parsed.entries, original.entries);
        assert_eq!(parsed.write(), bytes, "reserialize must be byte-identical");
    }

    #[test]
    fn parses_a_real_shaped_payload() {
        let mut data = Vec::new();
        data.extend_from_slice(&GXT2_MAGIC.to_le_bytes());
        data.extend_from_slice(&1u32.to_le_bytes());
        let text_offset: u32 = 16 + 8;
        data.extend_from_slice(&0xABCD1234u32.to_le_bytes());
        data.extend_from_slice(&text_offset.to_le_bytes());
        data.extend_from_slice(&GXT2_MAGIC.to_le_bytes());
        data.extend_from_slice(&(text_offset + 6).to_le_bytes());
        data.extend_from_slice(b"hola\0");

        let file = Gxt2File::read(&data).expect("read");
        assert_eq!(file.entries.len(), 1);
        assert_eq!(file.entries[0].hash, 0xABCD1234);
        assert_eq!(file.entries[0].text, "hola");
    }

    #[test]
    fn refuses_a_payload_without_the_magic() {
        let data = b"not a gxt2 file at all".to_vec();
        assert!(Gxt2File::read(&data).is_err());
    }

    #[test]
    fn to_text_and_from_text_are_inverses() {
        let file = Gxt2File {
            entries: vec![
                Gxt2Entry { hash: 0x22222222, text: "b".into() },
                Gxt2Entry { hash: 0x11111111, text: "a".into() },
            ],
        };
        let text = file.to_text();
        assert_eq!(
            text,
            "0x22222222 = b\n0x11111111 = a\n",
            "to_text keeps the stored order"
        );

        // from_text sorts by hash, matching the reference's FromText.
        let parsed = from_text(&text);
        assert_eq!(parsed.entries.len(), 2);
        assert_eq!(parsed.entries[0].hash, 0x11111111);
        assert_eq!(parsed.entries[0].text, "a");
        assert_eq!(parsed.entries[1].text, "b");
    }

    #[test]
    fn from_text_skips_lines_that_are_not_entries() {
        let parsed = from_text("not an entry\n0xZZZZZZZZ = bad\n0x0000002A = ok\n");
        assert_eq!(parsed.entries.len(), 1);
        assert_eq!(parsed.entries[0].hash, 0x2A);
        assert_eq!(parsed.entries[0].text, "ok");
    }
}
