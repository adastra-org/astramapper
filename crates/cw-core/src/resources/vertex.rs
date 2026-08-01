//! Vertex declarations: what a geometry's vertex bytes actually mean.
//!
//! Port of `CodeWalker.Core/GameFiles/Resources/VertexType.cs` and the
//! `VertexDeclaration` class in `Drawable.cs`.
//!
//! A declaration is two words. `flags` is a bitmask over the 16 semantics
//! (position, normal, texcoord0…) saying which are present; `types` is 16
//! nibbles, one per semantic, saying how each is stored. A component's byte
//! offset is the sum of the sizes of the present components below it, so the
//! layout is entirely derived — the file stores no offsets.
//!
//! Consequence worth stating: `flags` and `types` are indexed by the *semantic*,
//! not by the packing order. Nibble 3 describes the normal whether or not the
//! normal is present, and `Flags` decides presence. Reading the nibbles in
//! sequence would work on the common declaration and silently misparse the rest.

use serde::Serialize;

use crate::Result;

use super::reader::ResourceReader;

/// What a vertex semantic means. The index *is* the flag bit.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum VertexSemantic {
    Position,
    BlendWeights,
    BlendIndices,
    Normal,
    Colour0,
    Colour1,
    TexCoord0,
    TexCoord1,
    TexCoord2,
    TexCoord3,
    TexCoord4,
    TexCoord5,
    TexCoord6,
    TexCoord7,
    Tangent,
    Binormal,
}

impl VertexSemantic {
    pub const POSITION: usize = 0;
    pub const NORMAL: usize = 3;
    pub const COLOUR0: usize = 4;
    pub const TEXCOORD0: usize = 6;

    pub fn from_index(index: usize) -> Option<Self> {
        use VertexSemantic::*;
        Some(match index {
            0 => Position,
            1 => BlendWeights,
            2 => BlendIndices,
            3 => Normal,
            4 => Colour0,
            5 => Colour1,
            6 => TexCoord0,
            7 => TexCoord1,
            8 => TexCoord2,
            9 => TexCoord3,
            10 => TexCoord4,
            11 => TexCoord5,
            12 => TexCoord6,
            13 => TexCoord7,
            14 => Tangent,
            15 => Binormal,
            _ => return None,
        })
    }
}

/// How one component is stored. The nibble value, verbatim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum ComponentType {
    Nothing,
    Half2,
    Float,
    Half4,
    FloatUnk,
    Float2,
    Float3,
    Float4,
    UByte4,
    Colour,
    Rgba8SNorm,
    /// Nibbles 11-15. The reference names them `Unk1`..`Unk5`, sizes them zero
    /// and never sees one.
    Unknown(u8),
}

impl ComponentType {
    pub fn from_nibble(value: u8) -> Self {
        use ComponentType::*;
        match value & 0xF {
            0 => Nothing,
            1 => Half2,
            2 => Float,
            3 => Half4,
            4 => FloatUnk,
            5 => Float2,
            6 => Float3,
            7 => Float4,
            8 => UByte4,
            9 => Colour,
            10 => Rgba8SNorm,
            other => Unknown(other),
        }
    }

    /// Bytes this component occupies in a vertex. `FloatUnk` and the unknown
    /// nibbles are zero in the reference, which is what makes a declaration
    /// using one decode to a shorter stride rather than an error.
    pub fn size(self) -> usize {
        use ComponentType::*;
        match self {
            Nothing | FloatUnk | Unknown(_) => 0,
            Half2 | Float | UByte4 | Colour | Rgba8SNorm => 4,
            Half4 | Float2 => 8,
            Float3 => 12,
            Float4 => 16,
        }
    }

    /// How many scalars it decodes to.
    pub fn count(self) -> usize {
        use ComponentType::*;
        match self {
            Nothing | FloatUnk | Unknown(_) => 0,
            Float => 1,
            Half2 | Float2 => 2,
            Float3 => 3,
            Half4 | Float4 | UByte4 | Colour | Rgba8SNorm => 4,
        }
    }
}

/// The 16-byte declaration block.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct VertexDeclaration {
    /// Bit `k` set means semantic `k` is present.
    pub flags: u32,
    pub stride: u16,
    pub unknown_6h: u8,
    pub count: u8,
    /// 16 nibbles, one [`ComponentType`] per semantic.
    pub types: u64,
}

impl VertexDeclaration {
    pub fn read(r: &mut ResourceReader<'_>) -> Result<Self> {
        Ok(Self {
            flags: r.read_u32()?,
            stride: r.read_u16()?,
            unknown_6h: r.read_u8()?,
            count: r.read_u8()?,
            types: r.read_u64()?,
        })
    }

    pub fn has(&self, semantic: usize) -> bool {
        semantic < 16 && (self.flags >> semantic) & 1 == 1
    }

    pub fn component_type(&self, semantic: usize) -> ComponentType {
        ComponentType::from_nibble(((self.types >> (semantic * 4)) & 0xF) as u8)
    }

    /// Byte offset of a semantic within one vertex, or `None` if absent.
    pub fn component_offset(&self, semantic: usize) -> Option<usize> {
        if !self.has(semantic) {
            return None;
        }
        let mut offset = 0;
        for k in 0..semantic {
            if self.has(k) {
                offset += self.component_type(k).size();
            }
        }
        Some(offset)
    }

    /// Stride recomputed from the components, which is what
    /// `UpdateCountAndStride` does. The reference's own commented-out check says
    /// this never disagrees with the stored `stride`; a disagreement means the
    /// declaration was misread.
    pub fn computed_stride(&self) -> usize {
        (0..16)
            .filter(|&k| self.has(k))
            .map(|k| self.component_type(k).size())
            .sum()
    }

    /// Decode one semantic out of one vertex's bytes into up to four floats.
    ///
    /// Returns `None` when the semantic is absent or the vertex is short. Byte
    /// components are normalised the way a shader would see them: `Colour` and
    /// `UByte4` over 255, `Rgba8SNorm` over 127 and signed.
    pub fn decode(&self, vertex: &[u8], semantic: usize) -> Option<[f32; 4]> {
        let offset = self.component_offset(semantic)?;
        let kind = self.component_type(semantic);
        let bytes = vertex.get(offset..offset + kind.size())?;

        let f32_at =
            |i: usize| f32::from_le_bytes([bytes[i], bytes[i + 1], bytes[i + 2], bytes[i + 3]]);
        let half_at = |i: usize| half_to_f32(u16::from_le_bytes([bytes[i], bytes[i + 1]]));

        use ComponentType::*;
        Some(match kind {
            Nothing | FloatUnk | Unknown(_) => return None,
            Float => [f32_at(0), 0.0, 0.0, 0.0],
            Float2 => [f32_at(0), f32_at(4), 0.0, 0.0],
            Float3 => [f32_at(0), f32_at(4), f32_at(8), 0.0],
            Float4 => [f32_at(0), f32_at(4), f32_at(8), f32_at(12)],
            Half2 => [half_at(0), half_at(2), 0.0, 0.0],
            Half4 => [half_at(0), half_at(2), half_at(4), half_at(6)],
            UByte4 => [
                bytes[0] as f32,
                bytes[1] as f32,
                bytes[2] as f32,
                bytes[3] as f32,
            ],
            Colour => [
                bytes[0] as f32 / 255.0,
                bytes[1] as f32 / 255.0,
                bytes[2] as f32 / 255.0,
                bytes[3] as f32 / 255.0,
            ],
            Rgba8SNorm => [
                (bytes[0] as i8 as f32 / 127.0).max(-1.0),
                (bytes[1] as i8 as f32 / 127.0).max(-1.0),
                (bytes[2] as i8 as f32 / 127.0).max(-1.0),
                (bytes[3] as i8 as f32 / 127.0).max(-1.0),
            ],
        })
    }
}

/// IEEE 754 binary16 to binary32.
///
/// Written out rather than pulled from a crate because it is fifteen lines and
/// the subnormal branch is the only part worth getting right: GTA V texcoords
/// are small enough to land there.
pub fn half_to_f32(bits: u16) -> f32 {
    let sign = (bits as u32 & 0x8000) << 16;
    let exponent = (bits >> 10) & 0x1F;
    let mantissa = bits as u32 & 0x3FF;

    match exponent {
        // Zero or subnormal. A subnormal is `mantissa * 2^-24` with no implicit
        // leading one, so renormalise: shift left until bit 10 is set, which
        // becomes the implicit one, and pay for each shift in the exponent.
        // After `k` shifts the value is `(1.m') * 2^(-14-k)`, biased `113 - k`.
        0 => {
            if mantissa == 0 {
                return f32::from_bits(sign);
            }
            let mut m = mantissa;
            let mut k = 0u32;
            while m & 0x400 == 0 {
                m <<= 1;
                k += 1;
            }
            let exp = (113 - k) << 23;
            f32::from_bits(sign | exp | ((m & 0x3FF) << 13))
        }
        // Infinity or NaN.
        0x1F => f32::from_bits(sign | 0x7F80_0000 | (mantissa << 13)),
        _ => {
            let exp = ((exponent as u32) + 127 - 15) << 23;
            f32::from_bits(sign | exp | (mantissa << 13))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `0x7755555555996996` is the declaration nearly every YDR uses. Decoding
    /// it by hand is the check that the nibbles are indexed by semantic:
    /// position is Float3 (6), normal Float3 (6), the two colours Colour (9),
    /// texcoords Float2 (5), tangent and binormal Float4 (7).
    #[test]
    fn decodes_the_common_gtav_declaration() {
        let decl = VertexDeclaration {
            flags: 0xD9, // position, normal, colour0, texcoord0, texcoord1
            stride: 0,
            unknown_6h: 0,
            count: 0,
            types: 0x7755_5555_5599_6996,
        };

        assert_eq!(
            decl.component_type(VertexSemantic::POSITION),
            ComponentType::Float3
        );
        assert_eq!(
            decl.component_type(VertexSemantic::NORMAL),
            ComponentType::Float3
        );
        assert_eq!(
            decl.component_type(VertexSemantic::COLOUR0),
            ComponentType::Colour
        );
        assert_eq!(
            decl.component_type(VertexSemantic::TEXCOORD0),
            ComponentType::Float2
        );
        assert_eq!(decl.component_type(15), ComponentType::Float4);
    }

    /// Offsets are cumulative over *present* components only. With normal,
    /// colour0 and two texcoords set, position sits at 0, normal at 12,
    /// colour0 at 24, texcoord0 at 28 and texcoord1 at 36.
    #[test]
    fn offsets_skip_absent_components() {
        let decl = VertexDeclaration {
            flags: 0b1101_1001,
            stride: 0,
            unknown_6h: 0,
            count: 0,
            types: 0x7755_5555_5599_6996,
        };

        assert_eq!(decl.component_offset(0), Some(0));
        assert_eq!(decl.component_offset(3), Some(12));
        assert_eq!(decl.component_offset(4), Some(24));
        assert_eq!(decl.component_offset(6), Some(28));
        assert_eq!(decl.component_offset(7), Some(36));
        assert_eq!(decl.component_offset(1), None, "blend weights are not set");
        assert_eq!(decl.computed_stride(), 44);
    }

    #[test]
    fn sizes_the_component_types_as_the_reference_does() {
        use ComponentType::*;
        assert_eq!(Nothing.size(), 0);
        assert_eq!(Half2.size(), 4);
        assert_eq!(Float.size(), 4);
        assert_eq!(Half4.size(), 8);
        assert_eq!(FloatUnk.size(), 0, "the reference sizes FloatUnk at zero");
        assert_eq!(Float2.size(), 8);
        assert_eq!(Float3.size(), 12);
        assert_eq!(Float4.size(), 16);
        assert_eq!(UByte4.size(), 4);
        assert_eq!(Colour.size(), 4);
        assert_eq!(Rgba8SNorm.size(), 4);
        assert_eq!(Unknown(13).size(), 0);
    }

    #[test]
    fn decodes_a_vertex() {
        let decl = VertexDeclaration {
            flags: 0b0101_1001, // position, normal, colour0, texcoord0
            stride: 32,
            unknown_6h: 0,
            count: 4,
            types: 0x7755_5555_5599_6996,
        };

        let mut vertex = Vec::new();
        vertex.extend_from_slice(&1.5f32.to_le_bytes());
        vertex.extend_from_slice(&(-2.5f32).to_le_bytes());
        vertex.extend_from_slice(&3.0f32.to_le_bytes());
        vertex.extend_from_slice(&0.0f32.to_le_bytes());
        vertex.extend_from_slice(&1.0f32.to_le_bytes());
        vertex.extend_from_slice(&0.0f32.to_le_bytes());
        vertex.extend_from_slice(&[255, 128, 0, 255]);
        vertex.extend_from_slice(&0.25f32.to_le_bytes());
        vertex.extend_from_slice(&0.75f32.to_le_bytes());

        let pos = decl.decode(&vertex, 0).unwrap();
        assert_eq!([pos[0], pos[1], pos[2]], [1.5, -2.5, 3.0]);

        let normal = decl.decode(&vertex, 3).unwrap();
        assert_eq!([normal[0], normal[1], normal[2]], [0.0, 1.0, 0.0]);

        let colour = decl.decode(&vertex, 4).unwrap();
        assert_eq!(colour[0], 1.0);
        assert!((colour[1] - 128.0 / 255.0).abs() < f32::EPSILON);

        let uv = decl.decode(&vertex, 6).unwrap();
        assert_eq!([uv[0], uv[1]], [0.25, 0.75]);

        assert!(decl.decode(&vertex, 14).is_none(), "tangent is not present");
    }

    /// A vertex shorter than the declaration claims decodes to `None` rather
    /// than panicking. Mods ship these.
    #[test]
    fn refuses_to_read_past_a_short_vertex() {
        let decl = VertexDeclaration {
            flags: 0b0000_0001,
            stride: 12,
            unknown_6h: 0,
            count: 1,
            types: 0x7755_5555_5599_6996,
        };
        assert!(decl.decode(&[0u8; 8], 0).is_none());
    }

    #[test]
    fn converts_half_floats() {
        assert_eq!(half_to_f32(0x0000), 0.0);
        assert_eq!(half_to_f32(0x8000), -0.0);
        assert_eq!(half_to_f32(0x3C00), 1.0);
        assert_eq!(half_to_f32(0xC000), -2.0);
        assert_eq!(half_to_f32(0x3800), 0.5);
        assert!(half_to_f32(0x7C00).is_infinite());
        assert!(half_to_f32(0x7E00).is_nan());
        // Smallest subnormal: 2^-24.
        assert_eq!(half_to_f32(0x0001), 2.0f32.powi(-24));
        // Largest subnormal: (1023/1024) * 2^-14.
        assert_eq!(half_to_f32(0x03FF), (1023.0 / 1024.0) * 2.0f32.powi(-14));
    }
}
