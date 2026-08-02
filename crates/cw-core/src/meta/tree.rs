//! Walking a parsed metadata file into a tree of named values.
//!
//! Port of `MetaXml.WriteNode` / `PsoXml.WriteNode` / `RbfXml` from
//! `CodeWalker.Core/GameFiles/MetaTypes/MetaXml.cs`, which is how CodeWalker
//! turns a `.ymt` into readable XML.
//!
//! What is different here: the reference appends XML text as it walks, and this
//! builds a tree instead. The walk, the type dispatch and the value formatting
//! are the same; only the sink differs. A tree is what the Explorer needs — it
//! can be collapsed, searched and rendered — and XML text can be produced from
//! it, while the reverse needs a parser.
//!
//! Where the reference writes `<!--ERROR: ...-->` this produces a node with
//! [`MetaNode::error`] set and the same message. Those cases are real: a `.ymt`
//! can reference a structure whose definition is not in the file, and hiding
//! that would make a partial read look complete.

use serde::Serialize;

use crate::utils::hash_string;

use super::pso::{PsoDataType, PsoFile};
use super::rbf::{RbfEntry, RbfStructure, RbfValue};
use super::rsc::{Meta, MetaDataType, MetaPointer, ARRAY_INFO, POINTER_BLOCK, STRING_BLOCK};

/// How deep the walk will go. The formats have no depth limit and no cycle
/// check: a block pointing back at an ancestor would recurse forever.
const MAX_DEPTH: usize = 64;

/// How many elements one array node will expand. A scenario region names tens
/// of thousands of points, and a tree that large is neither renderable nor
/// readable; the node says how many were left out.
const MAX_ARRAY_ITEMS: usize = 4096;

/// One node of a metadata tree.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MetaNode {
    /// The field name, or `Item` inside an array.
    pub name: String,
    /// The structure or element type, when the file names one.
    pub type_name: Option<String>,
    /// The formatted value, for a leaf.
    pub value: Option<String>,
    /// The lookup name, for an entry of a map. The reference writes it as an
    /// XML attribute; a map entry is not addressable without it.
    pub key: Option<String>,
    pub children: Vec<MetaNode>,
    /// True for the nodes the reference emits as an XML comment rather than as
    /// data: a missing structure definition, an unsupported layout.
    pub error: bool,
}

impl MetaNode {
    fn leaf(name: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: Some(value.into()),
            ..Default::default()
        }
    }

    fn branch(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Default::default()
        }
    }

    fn error(name: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            value: Some(message.into()),
            error: true,
            ..Default::default()
        }
    }

    fn with_type(mut self, type_name: impl Into<String>) -> Self {
        self.type_name = Some(type_name.into());
        self
    }

    /// Total nodes in this subtree, the node itself included.
    pub fn count(&self) -> usize {
        1 + self.children.iter().map(Self::count).sum::<usize>()
    }
}

/// Format a float the way the reference's `FloatUtil.ToString` does: seven
/// significant digits if that reads back as the same number, nine otherwise.
///
/// Not the same as Rust's shortest round-trip: `1086.60546875` is `1086.6055`
/// at eight digits and `1086.60547` at nine, and the reference prints the
/// second because it never tries eight.
fn float(value: f32) -> String {
    if value.is_nan() {
        return "NaN".into();
    }
    if value.is_infinite() {
        return if value > 0.0 { "Infinity" } else { "-Infinity" }.into();
    }
    let short = general(value, 7);
    if short.parse::<f32>() == Ok(value) {
        short
    } else {
        general(value, 9)
    }
}

/// C#'s `G{digits}` for a finite float: round to that many significant digits,
/// drop the trailing zeros, and switch to exponent notation unless the decimal
/// exponent is greater than -5 and less than `digits`.
fn general(value: f32, digits: usize) -> String {
    if value == 0.0 {
        // .NET Framework has no sign on a formatted negative zero.
        return "0".into();
    }

    // Far more digits than are wanted, so the rounding below is this function's
    // and not the formatter's: Rust rounds a tie to the even digit and .NET
    // rounds it away from zero, and `1430.328125` at nine digits is such a tie.
    const EXACT: usize = 40;
    let scientific = format!("{value:.EXACT$e}");
    let (mantissa, exponent) = scientific
        .split_once('e')
        .expect("`{:e}` writes an exponent");
    let mut exponent: i32 = exponent.parse().expect("`{:e}` writes a decimal exponent");
    let sign = if mantissa.starts_with('-') { "-" } else { "" };
    let mut digit: Vec<u8> = mantissa.bytes().filter(u8::is_ascii_digit).collect();

    if digit.len() > digits && digit[digits] >= b'5' {
        digit.truncate(digits);
        let mut carried = true;
        for i in (0..digits).rev() {
            if digit[i] == b'9' {
                digit[i] = b'0';
            } else {
                digit[i] += 1;
                carried = false;
                break;
            }
        }
        if carried {
            digit.insert(0, b'1');
            digit.truncate(digits);
            exponent += 1;
        }
    }
    digit.truncate(digits);

    let significant = String::from_utf8(digit).expect("digits are ASCII");
    let significant = significant.trim_end_matches('0');
    let significant = if significant.is_empty() {
        "0"
    } else {
        significant
    };

    if exponent <= -5 || exponent >= digits as i32 {
        let head = &significant[..1];
        let tail = &significant[1..];
        let point = if tail.is_empty() {
            String::new()
        } else {
            format!(".{tail}")
        };
        return format!("{sign}{head}{point}E{:+03}", exponent);
    }

    if exponent < 0 {
        let zeros = "0".repeat((-exponent - 1) as usize);
        return format!("{sign}0.{zeros}{significant}");
    }

    let whole = exponent as usize + 1;
    if significant.len() <= whole {
        let zeros = "0".repeat(whole - significant.len());
        format!("{sign}{significant}{zeros}")
    } else {
        format!("{sign}{}.{}", &significant[..whole], &significant[whole..])
    }
}

fn vector(values: &[f32]) -> String {
    values
        .iter()
        .map(|v| float(*v))
        .collect::<Vec<_>>()
        .join(", ")
}

// ---------------------------------------------------------------------------
// RSC meta
// ---------------------------------------------------------------------------

/// Walk an RSC meta from its root block.
pub fn from_meta(meta: &Meta) -> MetaNode {
    let Some(block) = meta.block(meta.root_block_index) else {
        return MetaNode::error(
            "meta",
            format!("couldn't find block {}", meta.root_block_index),
        );
    };
    let name = hash_string(block.structure_name_hash);
    let mut node = write_meta_node(meta, meta.root_block_index, 0, block.structure_name_hash, 0);
    node.name = if name.is_empty() { "meta".into() } else { name };
    node
}

fn write_meta_node(
    meta: &Meta,
    block_id: i32,
    offset: usize,
    structure_name: u32,
    depth: usize,
) -> MetaNode {
    let name = hash_string(structure_name);
    if depth >= MAX_DEPTH {
        return MetaNode::error(name, format!("nested deeper than {MAX_DEPTH} structures"));
    }

    let Some(block) = meta.block(block_id) else {
        return MetaNode::error(name, format!("couldn't find block {block_id}"));
    };
    let Some(structure) = meta.structure(structure_name) else {
        return MetaNode::error(name.clone(), format!("couldn't find structure info {name}"));
    };

    let data = &block.data;
    let mut node = MetaNode::branch(name).with_type(hash_string(structure_name));

    // The ARRAYINFO entry is not a field: it describes the element type of the
    // array field that precedes it, and the reference keeps the last one seen.
    let mut array_info = None;

    for entry in &structure.entries {
        if entry.name_hash == ARRAY_INFO {
            array_info = Some(*entry);
            continue;
        }
        let field = hash_string(entry.name_hash);
        let at = offset.wrapping_add(entry.data_offset.max(0) as usize);

        let child = match entry.data_type {
            MetaDataType::Array => match array_info {
                Some(info) => write_meta_array(meta, data, info, &field, at, depth),
                None => MetaNode::error(field, "array with no ARRAYINFO entry"),
            },
            MetaDataType::ArrayOfBytes => {
                let count = entry.reference_key as usize;
                let subtype = array_info.map(|i| i.data_type);
                MetaNode::leaf(field, parsed_byte_array(data, at, count, subtype))
            }
            MetaDataType::ArrayOfChars => {
                let length = entry.reference_key as usize;
                let bytes = slice(data, at, length);
                let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
                MetaNode::leaf(field, text(&bytes[..end]))
            }
            MetaDataType::Boolean => MetaNode::leaf(
                field,
                if u8_at(data, at) != 0 {
                    "true"
                } else {
                    "false"
                },
            ),
            // The reference prints the raw byte for a ByteEnum rather than
            // resolving it, and so does this.
            MetaDataType::ByteEnum => MetaNode::leaf(field, u8_at(data, at).to_string()),
            MetaDataType::CharPointer => {
                let pointer = MetaPointer::unpack(u64_at(data, at));
                let length = u16_at(data, at + 8) as usize;
                MetaNode::leaf(
                    field,
                    meta_string(meta, pointer, length).unwrap_or_default(),
                )
            }
            MetaDataType::DataBlockPointer => {
                let pointer = MetaPointer::unpack(u64_at(data, at));
                match meta.block(pointer.block_id) {
                    Some(target) => MetaNode::leaf(
                        field,
                        parsed_byte_array(&target.data, 0, target.data.len(), None),
                    )
                    .with_type("ByteArray"),
                    None => {
                        MetaNode::error(field, format!("couldn't find block {}", pointer.block_id))
                    }
                }
            }
            MetaDataType::Float => MetaNode::leaf(field, float(f32_at(data, at))),
            MetaDataType::FloatXyz => MetaNode::leaf(
                field,
                vector(&[f32_at(data, at), f32_at(data, at + 4), f32_at(data, at + 8)]),
            )
            .with_type("Vector3"),
            MetaDataType::FloatXyzw => MetaNode::leaf(
                field,
                vector(&[
                    f32_at(data, at),
                    f32_at(data, at + 4),
                    f32_at(data, at + 8),
                    f32_at(data, at + 12),
                ]),
            )
            .with_type("Vector4"),
            MetaDataType::Hash => MetaNode::leaf(field, hash_string(u32_at(data, at))),
            MetaDataType::IntEnum | MetaDataType::IntFlags1 | MetaDataType::IntFlags2 => {
                let value = u32_at(data, at) as i32;
                MetaNode::leaf(
                    field,
                    enum_string(meta, entry.reference_key, entry.data_type, value),
                )
            }
            MetaDataType::ShortFlags => {
                let value = u16_at(data, at) as i16 as i32;
                MetaNode::leaf(
                    field,
                    enum_string(meta, entry.reference_key, entry.data_type, value),
                )
            }
            MetaDataType::SignedByte => MetaNode::leaf(field, (u8_at(data, at) as i8).to_string()),
            MetaDataType::SignedInt => MetaNode::leaf(field, (u32_at(data, at) as i32).to_string()),
            MetaDataType::SignedShort => {
                MetaNode::leaf(field, (u16_at(data, at) as i16).to_string())
            }
            MetaDataType::UnsignedByte => MetaNode::leaf(field, u8_at(data, at).to_string()),
            MetaDataType::UnsignedInt => MetaNode::leaf(field, u32_at(data, at).to_string()),
            MetaDataType::UnsignedShort => MetaNode::leaf(field, u16_at(data, at).to_string()),
            MetaDataType::Structure => {
                let mut child = write_meta_node(meta, block_id, at, entry.reference_key, depth + 1);
                child.name = field;
                child
            }
            // Verbatim from the reference, which answers this case with
            // "StructurePointer not supported here! Tell dexy!".
            MetaDataType::StructurePointer => MetaNode::error(
                field,
                "a structure pointer outside an array is not supported",
            ),
            MetaDataType::Unknown(raw) => {
                MetaNode::error(field, format!("unexpected entry data type {raw:#04x}"))
            }
        };
        node.children.push(child);
    }

    node
}

fn write_meta_array(
    meta: &Meta,
    data: &[u8],
    info: super::rsc::MetaEntry,
    field: &str,
    at: usize,
    depth: usize,
) -> MetaNode {
    // Every array header is the same 16 bytes: a packed pointer and two counts.
    let pointer = MetaPointer::unpack(u64_at(data, at));
    let count = u16_at(data, at + 8) as usize;

    match info.data_type {
        MetaDataType::Structure => {
            let mut node = MetaNode::branch(field).with_type(hash_string(info.reference_key));
            // An empty array is an empty node, whatever its element type is:
            // the reference only looks the type up inside `if (aCount > 0)`, so
            // a file that names a structure it never instantiates is not an
            // error there and must not be one here.
            if count == 0 {
                return node;
            }
            let Some(structure) = meta.structure(info.reference_key) else {
                return MetaNode::error(
                    field,
                    format!(
                        "couldn't find structure info {}",
                        hash_string(info.reference_key)
                    ),
                );
            };
            let stride = structure.size.max(1) as usize;

            let mut block_id = pointer.block_id;
            let mut offset = pointer.offset;
            for _ in 0..count.min(MAX_ARRAY_ITEMS) {
                let mut item =
                    write_meta_node(meta, block_id, offset, info.reference_key, depth + 1);
                item.name = "Item".into();
                node.children.push(item);

                offset += stride;
                // A long array runs off the end of its block and continues in
                // the next one.
                match meta.block(block_id) {
                    Some(block) if offset >= block.len() => {
                        offset = 0;
                        block_id += 1;
                    }
                    _ => {}
                }
            }
            note_truncation(&mut node, count);
            node
        }
        MetaDataType::StructurePointer => {
            let mut node = MetaNode::branch(field);
            let Some(block) = meta.block(pointer.block_id) else {
                return node;
            };
            if block.structure_name_hash != POINTER_BLOCK {
                return MetaNode::error(field, "a pointer array outside a pointer block");
            }
            for i in 0..count.min(MAX_ARRAY_ITEMS) {
                let at = pointer.offset + i * 8;
                if at + 8 > block.len() {
                    break;
                }
                let item_pointer = MetaPointer::unpack(u64_at(&block.data, at));
                let name = meta
                    .block(item_pointer.block_id)
                    .map(|b| b.structure_name_hash)
                    .unwrap_or(0);
                let mut item = write_meta_node(
                    meta,
                    item_pointer.block_id,
                    item_pointer.offset,
                    name,
                    depth + 1,
                );
                item.name = "Item".into();
                node.children.push(item);
            }
            note_truncation(&mut node, count);
            node
        }
        MetaDataType::UnsignedInt => MetaNode::leaf(
            field,
            join(
                read_array(meta, pointer, count, 4, None)
                    .chunks_exact(4)
                    .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]).to_string()),
            ),
        )
        .with_type("uint"),
        MetaDataType::UnsignedShort => MetaNode::leaf(
            field,
            join(
                read_array(meta, pointer, count, 2, None)
                    .chunks_exact(2)
                    .map(|c| u16::from_le_bytes([c[0], c[1]]).to_string()),
            ),
        )
        .with_type("ushort"),
        MetaDataType::UnsignedByte => MetaNode::leaf(
            field,
            join(
                read_array(meta, pointer, count, 1, None)
                    .iter()
                    .map(|b| b.to_string()),
            ),
        )
        .with_type("byte"),
        MetaDataType::Float => MetaNode::leaf(
            field,
            join(
                read_array(meta, pointer, count, 4, None)
                    .chunks_exact(4)
                    .map(|c| float(f32::from_le_bytes([c[0], c[1], c[2], c[3]]))),
            ),
        )
        .with_type("float"),
        // Hashes and vectors are one node per element rather than one line of
        // text: the reference writes these two with `WriteItemArray` and the
        // numeric arrays above with `WriteRawArray`, and the difference is not
        // cosmetic — a hash resolves to a name, which does not belong in a
        // space-separated run.
        MetaDataType::Hash => items(
            field,
            "Hash",
            read_array(meta, pointer, count, 4, None)
                .chunks_exact(4)
                .map(|c| hash_string(u32::from_le_bytes([c[0], c[1], c[2], c[3]]))),
            count,
        ),
        // Stored as `Vector4`: the reference reads the padded form and prints
        // all four components.
        MetaDataType::FloatXyz => items(
            field,
            "Vector3/4",
            read_array(meta, pointer, count, 16, None)
                .chunks_exact(16)
                .map(|c| {
                    let f =
                        |i: usize| float(f32::from_le_bytes([c[i], c[i + 1], c[i + 2], c[i + 3]]));
                    format!("{}, {}, {}, {}", f(0), f(4), f(8), f(12))
                }),
            count,
        ),
        MetaDataType::CharPointer => {
            MetaNode::error(field, "an array of char pointers is not supported")
        }
        MetaDataType::DataBlockPointer => {
            MetaNode::error(field, "an array of data block pointers is not supported")
        }
        other => MetaNode::error(
            field,
            format!("unexpected array entry data type {}", other.label()),
        ),
    }
}

/// Read `count` elements of `item_size` bytes starting at a packed pointer,
/// stepping into the following block when one runs out.
///
/// `MetaTypes.ConvertDataArray` refuses when the block's structure name is not
/// the pseudo-type the caller expects, and returns `null` — which the XML
/// writer renders as an empty array. That refusal is reproduced when `expected`
/// is given.
fn read_array(
    meta: &Meta,
    pointer: MetaPointer,
    count: usize,
    item_size: usize,
    expected: Option<u32>,
) -> Vec<u8> {
    let mut out = Vec::new();
    if count == 0 || item_size == 0 {
        return out;
    }
    let mut block_id = pointer.block_id;
    let mut offset = pointer.offset;
    let mut left = count.min(MAX_ARRAY_ITEMS);

    while left > 0 {
        let Some(block) = meta.block(block_id) else {
            break;
        };
        if expected.is_some_and(|name| block.structure_name_hash != name) {
            break;
        }
        if offset >= block.len() {
            break;
        }
        let available = (block.len() - offset) / item_size;
        let take = available.min(left);
        if take == 0 {
            break;
        }
        out.extend_from_slice(&block.data[offset..offset + take * item_size]);
        left -= take;
        offset = 0;
        block_id += 1;
    }
    out
}

/// `MetaTypes.GetString`: a string lives in a block whose structure name is the
/// STRING pseudo-type, and the pointer's count is its length.
fn meta_string(meta: &Meta, pointer: MetaPointer, length: usize) -> Option<String> {
    let block = meta.block(pointer.block_id)?;
    if block.structure_name_hash != STRING_BLOCK {
        return None;
    }
    // The reference refuses when the last byte is at or past the end, not past
    // it — an off-by-one it relies on, since a string block ends with its NUL.
    if pointer.offset + length >= block.len() {
        return None;
    }
    Some(text(&block.data[pointer.offset..pointer.offset + length]))
}

/// `MetaXml.GetEnumString`: a name for a value, or a comma-separated list of
/// set bits for the flag types.
fn enum_string(meta: &Meta, enum_name: u32, data_type: MetaDataType, value: i32) -> String {
    let Some(enumeration) = meta.enumeration(enum_name) else {
        return value.to_string();
    };

    // ShortFlags is deliberately *not* in this list: the reference has it
    // commented out and resolves it as a plain enum.
    let is_flags = matches!(data_type, MetaDataType::IntFlags1 | MetaDataType::IntFlags2);

    if is_flags {
        let mut names = Vec::new();
        for (name, bit) in &enumeration.entries {
            if (0..32).contains(bit) && value & (1 << bit) != 0 {
                names.push(hash_string(*name));
            }
        }
        return names.join(", ");
    }

    enumeration
        .entries
        .iter()
        .find(|(_, v)| *v == value)
        .map(|(name, _)| hash_string(*name))
        .unwrap_or_else(|| value.to_string())
}

// ---------------------------------------------------------------------------
// PSO
// ---------------------------------------------------------------------------

/// Walk a PSO from its root block.
pub fn from_pso(pso: &PsoFile) -> MetaNode {
    let Some(block) = pso.block(pso.root_id) else {
        return MetaNode::error("pso", format!("couldn't find block {}", pso.root_id));
    };
    let name = hash_string(block.name_hash);
    let mut node = write_pso_node(pso, pso.root_id, 0, block.name_hash, 0);
    node.name = if name.is_empty() { "pso".into() } else { name };
    node
}

fn write_pso_node(
    pso: &PsoFile,
    block_id: i32,
    offset: usize,
    structure_name: u32,
    depth: usize,
) -> MetaNode {
    let name = hash_string(structure_name);
    if depth >= MAX_DEPTH {
        return MetaNode::error(name, format!("nested deeper than {MAX_DEPTH} structures"));
    }

    let Some(block) = pso.block(block_id) else {
        return MetaNode::error(name, format!("couldn't find block {block_id}"));
    };
    // "how to tell when to do this??" — the reference's own comment on this
    // fallback, kept because dropping it loses whole subtrees.
    let offset = if offset >= block.length.max(0) as usize {
        offset >> 8
    } else {
        offset
    };
    let base = block.offset.max(0) as usize + offset;

    let Some(structure) = pso.structure(structure_name) else {
        return MetaNode::error(name.clone(), format!("couldn't find structure info {name}"));
    };

    let data = &pso.data;
    let mut node = MetaNode::branch(name.clone()).with_type(name);

    for entry in &structure.entries {
        if entry.name_hash == ARRAY_INFO {
            continue;
        }
        let field = hash_string(entry.name_hash);
        let at = base + entry.data_offset as usize;

        let child = match entry.data_type {
            PsoDataType::Array => {
                write_pso_array(pso, structure, *entry, &field, block_id, offset, depth)
            }
            PsoDataType::Bool => MetaNode::leaf(
                field,
                if u8_at(data, at) != 0 {
                    "true"
                } else {
                    "false"
                },
            ),
            PsoDataType::SByte => MetaNode::leaf(field, (u8_at(data, at) as i8).to_string()),
            PsoDataType::UByte => MetaNode::leaf(field, u8_at(data, at).to_string()),
            PsoDataType::SShort => MetaNode::leaf(field, (be_u16(data, at) as i16).to_string()),
            PsoDataType::UShort => MetaNode::leaf(field, be_u16(data, at).to_string()),
            PsoDataType::HFloat => MetaNode::leaf(field, (be_u16(data, at) as i16).to_string()),
            PsoDataType::SInt => MetaNode::leaf(field, (be_u32(data, at) as i32).to_string()),
            PsoDataType::UInt => match entry.unknown_5h {
                // Subtype 1 is an unsigned int the reference prints as hex.
                1 => MetaNode::leaf(field, format!("0x{:08X}", be_u32(data, at))),
                _ => MetaNode::leaf(field, (be_u32(data, at) as i32).to_string()),
            },
            PsoDataType::Long => MetaNode::leaf(field, pso_u64(data, at).to_string()),
            PsoDataType::Float => MetaNode::leaf(field, float(be_f32(data, at))),
            PsoDataType::Float2 => {
                MetaNode::leaf(field, vector(&[be_f32(data, at), be_f32(data, at + 4)]))
                    .with_type("Vector2")
            }
            // Float3, Float3a and Float4a are all read as three floats by the
            // reference, whatever their nominal width.
            PsoDataType::Float3 | PsoDataType::Float3a | PsoDataType::Float4a => MetaNode::leaf(
                field,
                vector(&[be_f32(data, at), be_f32(data, at + 4), be_f32(data, at + 8)]),
            )
            .with_type("Vector3"),
            PsoDataType::Float4 => MetaNode::leaf(
                field,
                vector(&[
                    be_f32(data, at),
                    be_f32(data, at + 4),
                    be_f32(data, at + 8),
                    be_f32(data, at + 12),
                ]),
            )
            .with_type("Vector4"),
            PsoDataType::String => {
                MetaNode::leaf(field, pso_string(pso, *entry, at).unwrap_or_default())
            }
            PsoDataType::Enum => {
                let value = match entry.unknown_5h {
                    2 => u8_at(data, at) as i32,
                    _ => be_u32(data, at) as i32,
                };
                // A value with no matching entry resolves to `HashString(0)`,
                // which is the empty string — not the number. The reference
                // writes `<contents />` for an unmapped enum and this has to
                // agree, because a number there would read as a value the file
                // does not carry.
                let name = hash_string(
                    pso.enumeration(entry.reference_key)
                        .and_then(|e| e.find(value))
                        .unwrap_or(0),
                );
                MetaNode::leaf(field, name)
            }
            PsoDataType::Flags => {
                let value = match entry.unknown_5h {
                    1 => be_u16(data, at) as u32,
                    2 => u8_at(data, at) as u32,
                    _ => be_u32(data, at),
                };
                // The enum for a flags field is not named directly: the low bits
                // of the reference key index another entry of the same
                // structure, and *that* entry names it.
                let index = (entry.reference_key & 0xFFF) as usize;
                let flags = structure
                    .entry(index)
                    .filter(|e| e.name_hash == ARRAY_INFO)
                    .and_then(|e| pso.enumeration(e.reference_key));
                match flags {
                    Some(flags) => {
                        let mut names = Vec::new();
                        for (name, bit) in &flags.entries {
                            if (0..32).contains(bit) && value & (1 << bit) != 0 {
                                names.push(hash_string(*name));
                            }
                        }
                        MetaNode::leaf(field, names.join(", "))
                    }
                    None => MetaNode::leaf(field, value.to_string()),
                }
            }
            PsoDataType::Structure => match entry.unknown_5h {
                0 => {
                    let mut child = write_pso_node(
                        pso,
                        block_id,
                        offset + entry.data_offset as usize,
                        entry.reference_key,
                        depth + 1,
                    );
                    child.name = field;
                    child
                }
                3 | 4 => {
                    let pointer = MetaPointer::unpack(pso_u64(data, at));
                    match pso.block(pointer.block_id) {
                        Some(target) => {
                            let mut child = write_pso_node(
                                pso,
                                pointer.block_id,
                                pointer.offset,
                                entry.reference_key,
                                depth + 1,
                            );
                            child.name = field;
                            child.type_name = Some(hash_string(target.name_hash));
                            child
                        }
                        // A null or out-of-range pointer is an empty element in
                        // the reference, not an error.
                        None => MetaNode::branch(field),
                    }
                }
                other => MetaNode::error(field, format!("unexpected structure subtype {other}")),
            },
            PsoDataType::Map => write_pso_map(pso, *entry, &field, at, depth),
            PsoDataType::Unknown(raw) => {
                MetaNode::error(field, format!("unexpected entry data type {raw:#04x}"))
            }
        };
        node.children.push(child);
    }

    node
}

fn write_pso_array(
    pso: &PsoFile,
    structure: &super::pso::PsoStructure,
    entry: super::pso::PsoStructureEntry,
    field: &str,
    block_id: i32,
    offset: usize,
    depth: usize,
) -> MetaNode {
    let Some(block) = pso.block(block_id) else {
        return MetaNode::error(field, format!("couldn't find block {block_id}"));
    };
    let data = &pso.data;
    let at = block.offset.max(0) as usize + offset + entry.data_offset as usize;

    // The element type is another entry of the same structure, named by the low
    // half of the reference key. Its high half is the count for an inline array.
    let mut index = (entry.reference_key & 0xFFFF) as usize;
    if index >= structure.entries.len() {
        index = (entry.reference_key & 0xFFF) as usize;
    }
    let Some(info) = structure.entry(index).copied() else {
        return MetaNode::error(field, "ARRAYINFO not found");
    };

    // Where the elements are: inline right here, or behind a packed pointer.
    let mut count = ((entry.reference_key >> 16) & 0xFFFF) as usize;
    let mut item_block = block_id;
    let mut item_offset = offset + entry.data_offset as usize;
    let mut inline = true;
    let pointed = matches!(entry.unknown_5h, 0) || (entry.unknown_5h == 4 && info.unknown_5h == 3);
    if pointed {
        let pointer = MetaPointer::unpack(pso_u64(data, at));
        count = be_u16(data, at + 8) as usize;
        item_block = pointer.block_id;
        item_offset = pointer.offset;
        inline = false;
    }

    let item_base = |pso: &PsoFile, block_id: i32, offset: usize| -> usize {
        pso.block(block_id)
            .map(|b| b.offset.max(0) as usize + offset)
            .unwrap_or(0)
    };

    // String and pointer arrays ignore the subtype above: the reference reads an
    // array header at the field either way, so an entry marked inline still
    // points somewhere. Only the raw value arrays can really be inline.
    let header = |pso: &PsoFile| -> (usize, usize) {
        let pointer = MetaPointer::unpack(pso_u64(&pso.data, at));
        (
            item_base(pso, pointer.block_id, pointer.offset),
            be_u16(&pso.data, at + 8) as usize,
        )
    };

    match info.data_type {
        PsoDataType::Structure => {
            // A pointer array: each element is a `PsoPOINTER` into some block.
            if info.unknown_5h == 3 {
                let mut node = MetaNode::branch(field);
                let base = item_base(pso, item_block, item_offset);
                for i in 0..count.min(MAX_ARRAY_ITEMS) {
                    let pointer = MetaPointer::unpack(pso_u64(data, base + i * 8));
                    let Some(target) = pso.block(pointer.block_id) else {
                        node.children.push(MetaNode::branch("Item"));
                        continue;
                    };
                    let mut item = write_pso_node(
                        pso,
                        pointer.block_id,
                        pointer.offset,
                        target.name_hash,
                        depth + 1,
                    );
                    item.name = "Item".into();
                    node.children.push(item);
                }
                note_truncation(&mut node, count);
                return node;
            }

            let mut node = MetaNode::branch(field).with_type(hash_string(info.reference_key));
            // See the note in the RSC walker: the reference resolves the
            // element type only when there is at least one element.
            if count == 0 {
                return node;
            }
            let Some(item_structure) = pso.structure(info.reference_key) else {
                return MetaNode::error(
                    field,
                    format!("array type not found: {}", hash_string(info.reference_key)),
                );
            };
            let stride = item_structure.length.max(1) as usize;
            let mut offset = item_offset;
            for _ in 0..count.min(MAX_ARRAY_ITEMS) {
                let mut item =
                    write_pso_node(pso, item_block, offset, info.reference_key, depth + 1);
                item.name = "Item".into();
                node.children.push(item);
                offset += stride;
                if pso
                    .block(item_block)
                    .is_some_and(|b| offset >= b.length.max(0) as usize)
                {
                    break;
                }
            }
            note_truncation(&mut node, count);
            node
        }
        PsoDataType::String => match info.unknown_5h {
            // An array of 4-byte hashes.
            7 | 8 => {
                let (base, count) = header(pso);
                items(
                    field,
                    "Hash",
                    (0..count.min(MAX_ARRAY_ITEMS))
                        .map(|i| hash_string(be_u32(data, base + i * 4))),
                    count,
                )
            }
            // An array of pointers to strings: eight-byte `DataBlockPointer`s
            // for subtype 2, sixteen-byte `CharPointer`s for subtype 3.
            2 | 3 => {
                let (base, count) = header(pso);
                let stride = if info.unknown_5h == 2 { 8 } else { 16 };
                let strings = (0..count.min(MAX_ARRAY_ITEMS)).map(|i| {
                    let at = base + i * stride;
                    if info.unknown_5h == 2 {
                        pso_string_at(pso, MetaPointer::unpack(pso_u64(data, at)), None)
                    } else {
                        pso_char_pointer(pso, at)
                    }
                    .unwrap_or_default()
                });
                items(field, "string", strings, count)
            }
            other => MetaNode::error(field, format!("unexpected string array subtype {other}")),
        },
        PsoDataType::Float => {
            let base = item_base(pso, item_block, item_offset);
            MetaNode::leaf(
                field,
                join((0..count.min(MAX_ARRAY_ITEMS)).map(|i| float(be_f32(data, base + i * 4)))),
            )
            .with_type("float")
        }
        // Both signed and unsigned int arrays print *unsigned*: the reference
        // reads either through `GetUintArray`, so a hash-like value in a signed
        // array comes out as its unsigned form rather than as a negative.
        PsoDataType::UInt | PsoDataType::SInt => {
            let base = item_base(pso, item_block, item_offset);
            MetaNode::leaf(
                field,
                join(
                    (0..count.min(MAX_ARRAY_ITEMS)).map(|i| be_u32(data, base + i * 4).to_string()),
                ),
            )
            .with_type("int")
        }
        PsoDataType::UShort | PsoDataType::SShort => {
            let base = item_base(pso, item_block, item_offset);
            MetaNode::leaf(
                field,
                join(
                    (0..count.min(MAX_ARRAY_ITEMS)).map(|i| be_u16(data, base + i * 2).to_string()),
                ),
            )
            .with_type("ushort")
        }
        PsoDataType::UByte | PsoDataType::Bool => {
            let base = item_base(pso, item_block, item_offset);
            MetaNode::leaf(
                field,
                join((0..count.min(MAX_ARRAY_ITEMS)).map(|i| u8_at(data, base + i).to_string())),
            )
            .with_type(if info.data_type == PsoDataType::Bool {
                "boolean"
            } else {
                "byte"
            })
        }
        PsoDataType::Float2 => {
            let base = item_base(pso, item_block, item_offset);
            MetaNode::leaf(
                field,
                join((0..count.min(MAX_ARRAY_ITEMS)).map(|i| {
                    let at = base + i * 8;
                    format!(
                        "({}, {})",
                        float(be_f32(data, at)),
                        float(be_f32(data, at + 4))
                    )
                })),
            )
            .with_type("Vector2")
        }
        PsoDataType::Float3 => {
            // Stored padded to four floats, as the reference notes.
            let base = item_base(pso, item_block, item_offset);
            MetaNode::leaf(
                field,
                join((0..count.min(MAX_ARRAY_ITEMS)).map(|i| {
                    let at = base + i * 16;
                    format!(
                        "({}, {}, {})",
                        float(be_f32(data, at)),
                        float(be_f32(data, at + 4)),
                        float(be_f32(data, at + 8))
                    )
                })),
            )
            .with_type("Vector3")
        }
        PsoDataType::Enum => {
            let base = item_base(pso, item_block, item_offset);
            let definition = pso.enumeration(info.reference_key);
            items(
                field,
                "enum",
                (0..count.min(MAX_ARRAY_ITEMS)).map(|i| {
                    let value = be_u32(data, base + i * 4) as i32;
                    // Unmapped resolves to the empty string, as above.
                    hash_string(definition.and_then(|d| d.find(value)).unwrap_or(0))
                }),
                count,
            )
        }
        // An array of arrays: each element is another array header of its own.
        PsoDataType::Array => {
            let mut node = MetaNode::branch(field);
            let outer = ((entry.reference_key >> 16) & 0xFFFF) as usize;
            let mut at = offset + entry.data_offset as usize;
            for _ in 0..outer.min(MAX_ARRAY_ITEMS) {
                let mut item =
                    write_pso_array(pso, structure, info, "Item", block_id, at, depth + 1);
                item.name = "Item".into();
                node.children.push(item);
                at += 16;
            }
            node
        }
        other => MetaNode::error(
            field,
            format!(
                "unsupported array entry data type {} ({})",
                other.label(),
                if inline { "inline" } else { "pointed" }
            ),
        ),
    }
}

/// `PsoXml.WriteMapNode`: a lookup table, stored as an array of two-field
/// structures whose fields are named `Key` and `Item`.
fn write_pso_map(
    pso: &PsoFile,
    entry: super::pso::PsoStructureEntry,
    field: &str,
    at: usize,
    depth: usize,
) -> MetaNode {
    /// `MetaName.Key` and `MetaName.Item`.
    const KEY: u32 = 1_620_616_462;
    const ITEM: u32 = 104_834_034;

    if entry.unknown_5h != 1 {
        return MetaNode::error(
            field,
            format!("unexpected map subtype {}", entry.unknown_5h),
        );
    }
    let data = &pso.data;

    // Two headers are in use. The first word tells them apart: `0x01000000`
    // means an array header eight bytes in, anything else means the pointer is
    // at +16 with the count at +2.
    let (pointer, count) = if be_u32(data, at) == 0x0100_0000 {
        (pso_u64(data, at + 8), be_u16(data, at + 16) as usize)
    } else {
        (pso_u64(data, at + 16), be_u16(data, at + 2) as usize)
    };
    let pointer = MetaPointer::unpack(pointer);

    let Some(block) = pso.block(pointer.block_id) else {
        return match count {
            0 => MetaNode::branch(field),
            _ => MetaNode::error(
                field,
                format!("couldn't find map block {}", pointer.block_id),
            ),
        };
    };
    if count == 0 {
        return MetaNode::branch(field);
    }

    let Some(entries) = pso.structure(block.name_hash) else {
        // An unnamed block is an empty map rather than a failure.
        return match block.name_hash {
            0 => MetaNode::branch(field),
            hash => MetaNode::error(
                field,
                format!("map struct type not found: {}", hash_string(hash)),
            ),
        };
    };
    let (Some(key_entry), Some(item_entry)) = (entries.find(KEY), entries.find(ITEM)) else {
        return MetaNode::error(field, "map Key/Item entries not found");
    };
    if key_entry.data_type != PsoDataType::String {
        return MetaNode::error(field, "map Key was not a string");
    }
    if !matches!(
        item_entry.data_type,
        PsoDataType::Structure | PsoDataType::String
    ) {
        return MetaNode::error(field, "map Item was not a structure or string");
    }

    let length = block.length.max(0) as usize;
    let mut offset = pointer.offset;
    // Some maps store the offset shifted a byte over. The reference shifts it
    // back rather than reading past the block, and says it does not know how to
    // tell in advance.
    if offset >= length {
        offset >>= 8;
    }

    let stride = entries.length.max(1) as usize;
    let mut node = MetaNode::branch(field);
    for _ in 0..count.min(MAX_ARRAY_ITEMS) {
        if offset >= length {
            node.children.push(MetaNode::error(
                "Item",
                format!("offset out of range, the count is {count}"),
            ));
            break;
        }
        let start = block.offset.max(0) as usize + offset;
        let key = pso_string(pso, *key_entry, start + key_entry.data_offset as usize);

        let mut item = if item_entry.data_type == PsoDataType::String {
            let value = pso_string(pso, *item_entry, start + item_entry.data_offset as usize);
            MetaNode::leaf("Item", value.unwrap_or_default()).with_type("String")
        } else if item_entry.reference_key != 0 {
            // The value is written into the entry itself.
            let mut child = write_pso_node(
                pso,
                pointer.block_id,
                offset + item_entry.data_offset as usize,
                item_entry.reference_key,
                depth + 1,
            );
            child.name = "Item".into();
            child.type_name = Some(hash_string(item_entry.reference_key));
            child
        } else {
            // The value is somewhere else, and its block names its type.
            let target =
                MetaPointer::unpack(pso_u64(data, start + item_entry.data_offset as usize));
            match pso.block(target.block_id) {
                None => MetaNode::error("Item", "couldn't find the block for a map entry"),
                Some(target_block) => {
                    let empty = pso
                        .structure(target_block.name_hash)
                        .is_some_and(|s| s.entries.is_empty());
                    let mut child = if empty {
                        MetaNode::branch("Item")
                    } else {
                        write_pso_node(
                            pso,
                            target.block_id,
                            target.offset,
                            target_block.name_hash,
                            depth + 1,
                        )
                    };
                    child.name = "Item".into();
                    child.type_name = Some(hash_string(target_block.name_hash));
                    child
                }
            }
        };
        item.key = key;
        node.children.push(item);
        offset += stride;
    }
    note_truncation(&mut node, count);
    node
}

/// `PsoXml.GetStringValue`.
fn pso_string(pso: &PsoFile, entry: super::pso::PsoStructureEntry, at: usize) -> Option<String> {
    let data = &pso.data;
    match entry.unknown_5h {
        // A fixed-length string stored in place, its length in the reference key.
        0 => {
            let length = ((entry.reference_key >> 16) & 0xFFFF) as usize;
            let bytes = slice(data, at, length);
            let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
            Some(text(&bytes[..end]))
        }
        // A pointer to a NUL-terminated string.
        1 | 2 => pso_string_at(pso, MetaPointer::unpack(pso_u64(data, at)), None),
        // A pointer with its lengths beside it.
        3 => Some(pso_char_pointer(pso, at).unwrap_or_default()),
        // A hash rather than a string.
        7 | 8 => Some(hash_string(be_u32(data, at))),
        _ => None,
    }
}

/// `PsoTypes.GetString(PsoFile, CharPointer)`: a sixteen-byte pointer carrying
/// two lengths. The shorter one wins when both are set, and it is the one that
/// excludes the terminator, so taking the first would read a byte past the
/// block and give nothing back.
fn pso_char_pointer(pso: &PsoFile, at: usize) -> Option<String> {
    let data = &pso.data;
    let count1 = be_u16(data, at + 8) as usize;
    let count2 = be_u16(data, at + 10) as usize;
    if count1 == 0 {
        return None;
    }
    let length = if count2 != 0 {
        count1.min(count2)
    } else {
        count1
    };
    pso_string_at(pso, MetaPointer::unpack(pso_u64(data, at)), Some(length))
}

/// A string in the data section, addressed by a packed pointer. With a length
/// it is a fixed slice, without one it runs to the first NUL.
fn pso_string_at(pso: &PsoFile, pointer: MetaPointer, length: Option<usize>) -> Option<String> {
    let block = pso.block(pointer.block_id)?;
    let start = block.offset.max(0) as usize + pointer.offset;
    match length {
        Some(0) => None,
        Some(length) => {
            if pointer.offset + length >= block.length.max(0) as usize {
                return None;
            }
            Some(text(slice(&pso.data, start, length)))
        }
        None => {
            let rest = pso.data.get(start..)?;
            let end = rest.iter().position(|b| *b == 0).unwrap_or(rest.len());
            Some(text(&rest[..end]))
        }
    }
}

// ---------------------------------------------------------------------------
// RBF
// ---------------------------------------------------------------------------

/// Convert an RBF tree, which is already a tree of named values.
pub fn from_rbf(root: &RbfStructure) -> MetaNode {
    let mut node = MetaNode::branch(root.name.clone());
    for attribute in &root.attributes {
        let mut child = rbf_node(attribute, true);
        child.type_name = Some("attribute".into());
        node.children.push(child);
    }
    for child in &root.children {
        // A byte block carries no name of its own: the reference writes it as
        // the text of the element holding it, so `<Name>` with one byte child is
        // the string, not a parent of one.
        if let RbfValue::Bytes(bytes) = &child.value {
            let text = rbf_bytes(root, bytes);
            match &mut node.value {
                Some(value) => value.push_str(&text),
                None => node.value = Some(text),
            }
            continue;
        }
        node.children.push(rbf_node(child, false));
    }
    node
}

/// The text of a byte block. An element says what its bytes mean with a
/// `content` attribute; without one they are a NUL-terminated string.
fn rbf_bytes(parent: &RbfStructure, bytes: &[u8]) -> String {
    let content = parent.attribute("content").map(|a| &a.value);
    match content {
        Some(RbfValue::String(kind)) if kind == "char_array" => {
            join(bytes.iter().map(|b| b.to_string()))
        }
        Some(RbfValue::String(kind)) if kind == "short_array" => join(
            bytes
                .chunks_exact(2)
                .map(|c| u16::from_le_bytes([c[0], c[1]]).to_string()),
        ),
        Some(RbfValue::String(kind)) => format!("unexpected content type: {kind}"),
        _ => text(&bytes[..bytes.len().saturating_sub(1)]),
    }
}

/// One entry of an element. An unsigned value reads as decimal in an attribute
/// and as hex in a child, which is the reference's choice and not a guess.
fn rbf_node(entry: &RbfEntry, attribute: bool) -> MetaNode {
    match &entry.value {
        RbfValue::Structure(structure) => {
            let mut node = from_rbf(structure);
            node.name = entry.name.clone();
            node
        }
        RbfValue::Uint(value) if attribute => MetaNode::leaf(entry.name.clone(), value.to_string()),
        RbfValue::Uint(value) => MetaNode::leaf(entry.name.clone(), format!("0x{value:X}")),
        // Capitalised, which is what `bool.ToString()` gives the reference.
        RbfValue::Boolean(value) => {
            MetaNode::leaf(entry.name.clone(), if *value { "True" } else { "False" })
        }
        RbfValue::Float(value) => MetaNode::leaf(entry.name.clone(), float(*value)),
        RbfValue::Float3(values) => {
            MetaNode::leaf(entry.name.clone(), vector(values)).with_type("Vector3")
        }
        RbfValue::String(value) => MetaNode::leaf(entry.name.clone(), value.clone()),
        // Handled by `from_rbf`: a byte block belongs to the element above it.
        RbfValue::Bytes(bytes) => MetaNode::leaf(entry.name.clone(), text(bytes)),
    }
}

// ---------------------------------------------------------------------------
// Shared helpers
// ---------------------------------------------------------------------------

/// An array node with one `Item` child per element, which is how the reference
/// writes the arrays whose elements are names rather than numbers.
fn items(
    field: &str,
    type_name: &str,
    values: impl Iterator<Item = String>,
    count: usize,
) -> MetaNode {
    let mut node = MetaNode::branch(field).with_type(type_name);
    for value in values.take(MAX_ARRAY_ITEMS) {
        node.children.push(MetaNode::leaf("Item", value));
    }
    note_truncation(&mut node, count);
    node
}

fn note_truncation(node: &mut MetaNode, count: usize) {
    if count > MAX_ARRAY_ITEMS {
        node.children.push(MetaNode::leaf(
            "…",
            format!("{} more items not expanded", count - MAX_ARRAY_ITEMS),
        ));
    }
}

fn join(values: impl Iterator<Item = String>) -> String {
    values.collect::<Vec<_>>().join(", ")
}

/// Bytes at `at`, zero-padded past the end.
///
/// The reference indexes its data arrays directly and throws on a bad offset,
/// which `MetaXml` then catches per file. Reading short as zeros keeps the rest
/// of the tree, which is the same trade the resource reader already makes.
fn slice(data: &[u8], at: usize, length: usize) -> &[u8] {
    let start = at.min(data.len());
    let end = start.saturating_add(length).min(data.len());
    &data[start..end]
}

fn u8_at(data: &[u8], at: usize) -> u8 {
    data.get(at).copied().unwrap_or(0)
}

fn u16_at(data: &[u8], at: usize) -> u16 {
    let b = slice(data, at, 2);
    if b.len() < 2 {
        return 0;
    }
    u16::from_le_bytes([b[0], b[1]])
}

fn u32_at(data: &[u8], at: usize) -> u32 {
    let b = slice(data, at, 4);
    if b.len() < 4 {
        return 0;
    }
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

fn u64_at(data: &[u8], at: usize) -> u64 {
    let b = slice(data, at, 8);
    if b.len() < 8 {
        return 0;
    }
    u64::from_le_bytes([b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7]])
}

fn f32_at(data: &[u8], at: usize) -> f32 {
    f32::from_bits(u32_at(data, at))
}

fn be_u16(data: &[u8], at: usize) -> u16 {
    u16_at(data, at).swap_bytes()
}

fn be_u32(data: &[u8], at: usize) -> u32 {
    u32_at(data, at).swap_bytes()
}

/// A 64-bit value out of a PSO, the way the reference reads one.
///
/// **Not a byte swap.** `MetaTypes.SwapBytes(ulong)` swaps the bytes *within*
/// each 32-bit half and deliberately leaves the halves where they are — its
/// own comment is `[not swapping 32bit blocks! careful!]`. Every 64-bit field
/// in a PSO goes through it, so a packed pointer's block id and offset live in
/// the first four bytes and the last four are ignored. A plain `to_be` here
/// reads the block id out of the wrong word and resolves every array to block
/// zero.
fn pso_u64(data: &[u8], at: usize) -> u64 {
    let x = u64_at(data, at);
    let low = (x as u32).swap_bytes() as u64;
    let high = ((x >> 32) as u32).swap_bytes() as u64;
    (high << 32) | low
}

fn be_f32(data: &[u8], at: usize) -> f32 {
    f32::from_bits(be_u32(data, at))
}

/// Decode a stored string.
///
/// **A deliberate divergence.** The reference uses `Encoding.ASCII`, which
/// substitutes `?` for every byte above 0x7F — so `credits.ymt` renders a
/// Polish name as `??ukasz Bogaj` in CodeWalker. The bytes are UTF-8, and
/// decoding them as UTF-8 gives the name back. Nothing else in the file is
/// affected: the two agree byte for byte on ASCII, which is every structure
/// name, every asset name and almost every value in the game's metadata.
fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// A byte array as the reference prints it: hex pairs by default, decimal for
/// the signed and unsigned byte subtypes.
/// `MetaXml.WriteParsedArrayOfBytesNode`. The count is in elements and the
/// element type sets the stride, so five floats are twenty bytes and not five.
/// Without a known element type the reference writes the bytes as hex.
fn parsed_byte_array(
    data: &[u8],
    at: usize,
    count: usize,
    subtype: Option<MetaDataType>,
) -> String {
    let count = count.min(MAX_ARRAY_ITEMS);
    match subtype {
        Some(MetaDataType::SignedByte) => {
            join((0..count).map(|n| (u8_at(data, at + n) as i8).to_string()))
        }
        Some(MetaDataType::UnsignedByte) => {
            join((0..count).map(|n| u8_at(data, at + n).to_string()))
        }
        Some(MetaDataType::SignedShort) => {
            join((0..count).map(|n| (u16_at(data, at + n * 2) as i16).to_string()))
        }
        Some(MetaDataType::UnsignedShort) => {
            join((0..count).map(|n| u16_at(data, at + n * 2).to_string()))
        }
        Some(MetaDataType::SignedInt) => {
            join((0..count).map(|n| (u32_at(data, at + n * 4) as i32).to_string()))
        }
        Some(MetaDataType::UnsignedInt) => {
            join((0..count).map(|n| u32_at(data, at + n * 4).to_string()))
        }
        Some(MetaDataType::Float) => join((0..count).map(|n| float(f32_at(data, at + n * 4)))),
        _ => (0..count)
            .map(|n| format!("{:02X}", u8_at(data, at + n)))
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_floats_the_short_way() {
        assert_eq!(float(1.0), "1");
        assert_eq!(float(0.5), "0.5");
        assert_eq!(vector(&[1.0, 2.5, -3.0]), "1, 2.5, -3");
    }

    #[test]
    fn falls_back_to_nine_digits_when_seven_do_not_read_back() {
        // Seven digits is `1086.605`, which is a different float.
        assert_eq!(float(1_086.605_5), "1086.60547");
        // A tie at the ninth digit, which rounds away from zero and not to even.
        assert_eq!(float(1_430.328_1), "1430.32813");
        assert_eq!(float(0.1), "0.1");
        assert_eq!(float(-0.0), "0");
    }

    #[test]
    fn leaves_the_fixed_range_in_exponent_notation() {
        assert_eq!(float(1.0e-7), "1E-07");
        assert_eq!(float(1.0e20), "1E+20");
        assert_eq!(float(0.000_01), "1E-05");
        assert_eq!(float(0.000_1), "0.0001");
        assert_eq!(float(1_000_000.0), "1000000");
        assert_eq!(float(12_345_678.0), "12345678");
    }

    #[test]
    fn reads_short_data_as_zeros_rather_than_panicking() {
        let data = [1u8, 2];
        assert_eq!(u32_at(&data, 0), 0);
        assert_eq!(u16_at(&data, 0), 0x0201);
        assert_eq!(u8_at(&data, 9), 0);
        assert_eq!(slice(&data, 5, 4), &[] as &[u8]);
    }

    #[test]
    fn counts_a_subtree() {
        let mut root = MetaNode::branch("root");
        root.children.push(MetaNode::leaf("a", "1"));
        let mut branch = MetaNode::branch("b");
        branch.children.push(MetaNode::leaf("c", "2"));
        root.children.push(branch);
        assert_eq!(root.count(), 4);
    }

    #[test]
    fn converts_an_rbf_tree() {
        let root = RbfStructure {
            name: "CMapParentTxds".into(),
            attributes: vec![RbfEntry {
                name: "version".into(),
                value: RbfValue::Uint(2),
            }],
            children: vec![RbfEntry {
                name: "item".into(),
                value: RbfValue::Structure(RbfStructure {
                    name: "item".into(),
                    attributes: Vec::new(),
                    children: vec![RbfEntry {
                        name: "parent".into(),
                        value: RbfValue::String("gtxd".into()),
                    }],
                }),
            }],
        };

        let node = from_rbf(&root);
        assert_eq!(node.name, "CMapParentTxds");
        assert_eq!(node.children.len(), 2);
        assert_eq!(node.children[0].type_name.as_deref(), Some("attribute"));
        assert_eq!(node.children[1].children[0].value.as_deref(), Some("gtxd"));
    }
}
