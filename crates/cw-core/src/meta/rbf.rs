//! RBF: the oldest of the three metadata encodings, and the only self-
//! describing one.
//!
//! Port of `RbfFile` and its value types from
//! `CodeWalker.Core/GameFiles/MetaTypes/Rbf.cs`. Read only; the writer is not
//! ported.
//!
//! An RBF file is a token stream, not a table of offsets: `RBF0`, then a run of
//! records that open elements, add values and close elements again. Names are
//! interned the first time they appear — a record either references a
//! descriptor already declared or *is* the declaration of the next one, which
//! is why the reader has to keep the descriptor list rather than seeking.
//!
//! One thing that is not guessable: **the first N children of an element are
//! its attributes**, where N is a count read at the open record. There is no
//! marker on the children themselves.

use crate::utils::DataReader;
use crate::{CwError, Result};

/// `RBF0`, little-endian.
const IDENT: u32 = 0x3046_4252;

/// Guard on a length read out of the file before it sizes an allocation.
const MAX_LENGTH: usize = 64 * 1024 * 1024;

/// Guard on nesting. The format has no depth limit and a malformed file can
/// open elements without ever closing them.
const MAX_DEPTH: usize = 512;

/// One value in an RBF tree.
#[derive(Debug, Clone, PartialEq)]
pub enum RbfValue {
    /// An element: attributes first, then children.
    Structure(RbfStructure),
    Uint(u32),
    Boolean(bool),
    Float(f32),
    Float3([f32; 3]),
    String(String),
    /// A raw blob. These carry no name in the file.
    Bytes(Vec<u8>),
}

/// A named value.
#[derive(Debug, Clone, PartialEq)]
pub struct RbfEntry {
    pub name: String,
    pub value: RbfValue,
}

/// An element.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RbfStructure {
    pub name: String,
    pub attributes: Vec<RbfEntry>,
    pub children: Vec<RbfEntry>,
}

impl RbfStructure {
    pub fn child(&self, name: &str) -> Option<&RbfEntry> {
        self.children.iter().find(|c| c.name == name)
    }

    pub fn attribute(&self, name: &str) -> Option<&RbfEntry> {
        self.attributes.iter().find(|a| a.name == name)
    }
}

/// Whether a payload starts with the RBF magic.
pub fn is_rbf(data: &[u8]) -> bool {
    data.len() >= 4 && u32::from_le_bytes([data[0], data[1], data[2], data[3]]) == IDENT
}

/// Parse an RBF payload into its root element.
pub fn read(data: &[u8]) -> Result<RbfStructure> {
    if !is_rbf(data) {
        return Err(CwError::Invalid {
            what: "rbf",
            detail: "the file identifier does not match".into(),
        });
    }

    let mut r = DataReader::new(data);
    r.set_position(4);

    // (name, type) per descriptor, in declaration order. A record names a
    // descriptor by its index in this list.
    let mut descriptors: Vec<(String, u8)> = Vec::new();
    // The element being filled, and the elements above it. `pending` is how
    // many more of the current element's children are really attributes.
    let mut current: Option<(RbfStructure, usize)> = None;
    let mut stack: Vec<(RbfStructure, usize, String)> = Vec::new();
    let mut current_name = String::new();

    while r.remaining() > 0 {
        let descriptor_index = r.read_u8()? as usize;

        if descriptor_index == 0xFF {
            expect_ff(&mut r)?;
            let Some((finished, _)) = current.take() else {
                return Err(CwError::Invalid {
                    what: "rbf",
                    detail: "a close tag with nothing open".into(),
                });
            };
            match stack.pop() {
                Some((mut parent, pending, name)) => {
                    let entry = RbfEntry {
                        name: finished.name.clone(),
                        value: RbfValue::Structure(finished),
                    };
                    let pending = add_child(&mut parent, pending, entry);
                    current = Some((parent, pending));
                    current_name = name;
                }
                // The root closed. The reference then insists the stream is
                // finished, and so does this: trailing bytes mean the file is
                // not what it claims.
                None => {
                    if r.remaining() != 0 {
                        return Err(CwError::Invalid {
                            what: "rbf",
                            detail: format!("{} bytes after the root closed", r.remaining()),
                        });
                    }
                    return Ok(finished);
                }
            }
            continue;
        }

        if descriptor_index == 0xFD {
            expect_ff(&mut r)?;
            let length = r.read_i32()?;
            if length < 0 || length as usize > MAX_LENGTH {
                return Err(CwError::Invalid {
                    what: "rbf",
                    detail: format!("a byte block of {length} bytes"),
                });
            }
            let bytes = r.read_bytes(length as usize)?.to_vec();
            let Some((element, pending)) = current.as_mut() else {
                return Err(CwError::Invalid {
                    what: "rbf",
                    detail: "a value before any element was opened".into(),
                });
            };
            // Byte blocks carry no descriptor, so they have no name of their
            // own. The reference leaves it null.
            let entry = RbfEntry {
                name: String::new(),
                value: RbfValue::Bytes(bytes),
            };
            *pending = add_child(element, *pending, entry);
            continue;
        }

        let data_type = r.read_u8()?;
        if descriptor_index == descriptors.len() {
            let name_length = r.read_i16()?;
            if name_length < 0 || name_length as usize > MAX_LENGTH {
                return Err(CwError::Invalid {
                    what: "rbf",
                    detail: format!("a descriptor name of {name_length} bytes"),
                });
            }
            let bytes = r.read_bytes(name_length as usize)?;
            descriptors.push((ascii(bytes), data_type));
        } else if descriptor_index > descriptors.len() {
            return Err(CwError::Invalid {
                what: "rbf",
                detail: format!("descriptor {descriptor_index} referenced before it was declared"),
            });
        }

        // A record may disagree with its descriptor's declared type. The
        // reference has that check commented out and follows the record, so
        // this does too.
        let name = descriptors[descriptor_index].0.clone();

        if data_type == 0 {
            let _unknown_0 = r.read_i16()?;
            let _unknown_1 = r.read_i16()?;
            let attributes = r.read_i16()?.max(0) as usize;

            if let Some((element, pending)) = current.take() {
                if stack.len() >= MAX_DEPTH {
                    return Err(CwError::Invalid {
                        what: "rbf",
                        detail: format!("nested deeper than {MAX_DEPTH} elements"),
                    });
                }
                stack.push((element, pending, current_name.clone()));
            }
            current_name = name.clone();
            current = Some((
                RbfStructure {
                    name,
                    ..Default::default()
                },
                attributes,
            ));
            continue;
        }

        let value = match data_type {
            0x10 => RbfValue::Uint(r.read_u32()?),
            0x20 => RbfValue::Boolean(true),
            0x30 => RbfValue::Boolean(false),
            0x40 => RbfValue::Float(r.read_f32()?),
            0x50 => RbfValue::Float3([r.read_f32()?, r.read_f32()?, r.read_f32()?]),
            0x60 => {
                let length = r.read_i16()?;
                if length < 0 || length as usize > MAX_LENGTH {
                    return Err(CwError::Invalid {
                        what: "rbf",
                        detail: format!("a string of {length} bytes"),
                    });
                }
                RbfValue::String(ascii(r.read_bytes(length as usize)?))
            }
            other => {
                return Err(CwError::Invalid {
                    what: "rbf",
                    detail: format!("unsupported data type {other:#04x}"),
                })
            }
        };

        let Some((element, pending)) = current.as_mut() else {
            return Err(CwError::Invalid {
                what: "rbf",
                detail: "a value before any element was opened".into(),
            });
        };
        *pending = add_child(element, *pending, RbfEntry { name, value });
    }

    Err(CwError::Invalid {
        what: "rbf",
        detail: "unexpected end of stream".into(),
    })
}

/// `RbfStructure.AddChild`: the first `pending` children are attributes.
/// Returns what is left of the attribute budget.
fn add_child(element: &mut RbfStructure, pending: usize, entry: RbfEntry) -> usize {
    if pending > 0 {
        element.attributes.push(entry);
        pending - 1
    } else {
        element.children.push(entry);
        pending
    }
}

fn expect_ff(r: &mut DataReader<'_>) -> Result<()> {
    let b = r.read_u8()?;
    if b != 0xFF {
        return Err(CwError::Invalid {
            what: "rbf",
            detail: format!("expected 0xFF but found {b:#04X}"),
        });
    }
    Ok(())
}

/// The reference reads every RBF name and string as ASCII, which maps each
/// byte to one character. `from_utf8_lossy` would turn a high byte into U+FFFD
/// instead of the character the reference produces, so the bytes are widened
/// the same way it does.
fn ascii(bytes: &[u8]) -> String {
    bytes.iter().map(|b| *b as char).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build the token stream by hand: `<root a="1"><child/>3.5</root>`, in
    /// the shape the format actually stores.
    fn synthetic() -> Vec<u8> {
        let mut d = Vec::new();
        d.extend_from_slice(&IDENT.to_le_bytes());

        // Descriptor 0: "root", an element with one attribute.
        d.push(0);
        d.push(0);
        d.extend_from_slice(&4i16.to_le_bytes());
        d.extend_from_slice(b"root");
        d.extend_from_slice(&0i16.to_le_bytes());
        d.extend_from_slice(&0i16.to_le_bytes());
        d.extend_from_slice(&1i16.to_le_bytes()); // one attribute

        // Descriptor 1: "count", a uint. First child, so it is the attribute.
        d.push(1);
        d.push(0x10);
        d.extend_from_slice(&5i16.to_le_bytes());
        d.extend_from_slice(b"count");
        d.extend_from_slice(&7u32.to_le_bytes());

        // Descriptor 2: "scale", a float. Second child, so a real child.
        d.push(2);
        d.push(0x40);
        d.extend_from_slice(&5i16.to_le_bytes());
        d.extend_from_slice(b"scale");
        d.extend_from_slice(&3.5f32.to_le_bytes());

        // Descriptor 1 again, by reference this time.
        d.push(1);
        d.push(0x10);
        d.extend_from_slice(&9u32.to_le_bytes());

        d.push(0xFF);
        d.push(0xFF);
        d
    }

    #[test]
    fn reads_elements_attributes_and_values() {
        let root = read(&synthetic()).unwrap();
        assert_eq!(root.name, "root");
        assert_eq!(root.attributes.len(), 1);
        assert_eq!(root.attributes[0].name, "count");
        assert_eq!(root.attributes[0].value, RbfValue::Uint(7));
        assert_eq!(root.children.len(), 2);
        assert_eq!(root.children[0].value, RbfValue::Float(3.5));
        assert_eq!(root.children[1].name, "count");
        assert_eq!(root.children[1].value, RbfValue::Uint(9));
    }

    #[test]
    fn recognises_and_rejects_by_magic() {
        assert!(is_rbf(&synthetic()));
        assert!(!is_rbf(b"PSIN"));
        assert!(read(b"PSIN\0\0\0\0").is_err());
    }

    /// A stream that opens an element and stops must fail, not return a
    /// half-built tree.
    #[test]
    fn refuses_an_unterminated_stream() {
        let mut data = synthetic();
        data.truncate(data.len() - 2);
        assert!(read(&data).is_err());
    }
}
