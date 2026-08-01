//! AES and NG decryption for RPF archives.
//!
//! Port of `CodeWalker.Core/GameFiles/Utils/GTACrypto.cs`.
//!
//! The C# version reaches into `GTA5Keys`' static fields from inside the
//! algorithms. Here the key material is passed in explicitly (see
//! [`crate::utils::keys::GtaKeys`]), which removes a real initialization-order
//! hazard: `GTA5Hash`'s static constructor snapshots `GTA5Keys.PC_LUT`, so in C#
//! the hash silently uses a null LUT if the type is touched before the keys
//! load.

use aes::cipher::{BlockCipherDecrypt, BlockCipherEncrypt, KeyInit};
use aes::Aes256;

/// Decrypt in place with AES-256 in ECB mode, no padding.
///
/// Port of `GTACrypto.DecryptAESData`. Trailing bytes that do not fill a
/// 16-byte block are left untouched, exactly as the reference does by
/// truncating `length` to a multiple of 16.
pub fn decrypt_aes_data(data: &[u8], key: &[u8; 32], rounds: usize) -> Vec<u8> {
    let mut buffer = data.to_vec();
    let length = buffer.len() - buffer.len() % 16;
    if length == 0 {
        return buffer;
    }

    let cipher = Aes256::new(key.into());
    for _ in 0..rounds {
        for block in buffer[..length].chunks_exact_mut(16) {
            let block: &mut [u8; 16] = block.try_into().expect("chunks_exact_mut(16) yields 16");
            cipher.decrypt_block(block.into());
        }
    }
    buffer
}

/// Port of `GTACrypto.EncryptAESData`. Needed to write archives back out.
pub fn encrypt_aes_data(data: &[u8], key: &[u8; 32], rounds: usize) -> Vec<u8> {
    let mut buffer = data.to_vec();
    let length = buffer.len() - buffer.len() % 16;
    if length == 0 {
        return buffer;
    }

    let cipher = Aes256::new(key.into());
    for _ in 0..rounds {
        for block in buffer[..length].chunks_exact_mut(16) {
            let block: &mut [u8; 16] = block.try_into().expect("chunks_exact_mut(16) yields 16");
            cipher.encrypt_block(block.into());
        }
    }
    buffer
}

/// The name hash that selects which of the 101 NG keys an archive uses.
///
/// Port of `GTA5Hash.CalculateHash`. This is *not* the Jenkins hash — it is a
/// separate function driven by a 256-byte lookup table that ships inside
/// `magic.dat`. Mixing the two up yields a valid-looking key index that decrypts
/// to noise.
///
/// Every arithmetic step wraps, as it does in unchecked C#.
pub fn gta5_hash(text: &str, lut: &[u8; 256]) -> u32 {
    let mut result: u32 = 0;
    for unit in text.encode_utf16() {
        // C# indexes `LUT[text[index]]` with a UTF-16 code unit and would throw
        // above U+00FF. Archive names are ASCII; mask so a stray byte cannot
        // panic mid-archive.
        let lut_value = lut[(unit & 0xFF) as usize] as u32;
        let temp = 1025u32.wrapping_mul(lut_value.wrapping_add(result));
        result = (temp >> 6) ^ temp;
    }
    let nine = 9u32.wrapping_mul(result);
    32769u32.wrapping_mul((nine >> 11) ^ nine)
}

/// One 16-byte NG block. Port of `GTACrypto.DecryptNGBlock`.
///
/// Seventeen rounds: A for rounds 0, 1 and 16, B for 2 through 15. The two
/// differ only in which table column each input byte feeds.
fn decrypt_ng_block(block: &[u8; 16], key: &[u32; 68], tables: &NgDecryptTables) -> [u8; 16] {
    let sub_key =
        |i: usize| -> [u32; 4] { [key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]] };

    let mut buffer = *block;
    buffer = decrypt_ng_round_a(&buffer, &sub_key(0), &tables.0[0]);
    buffer = decrypt_ng_round_a(&buffer, &sub_key(1), &tables.0[1]);
    for k in 2..=15 {
        buffer = decrypt_ng_round_b(&buffer, &sub_key(k), &tables.0[k]);
    }
    decrypt_ng_round_a(&buffer, &sub_key(16), &tables.0[16])
}

/// Rounds 0, 1 and 16: each output word takes four consecutive input bytes.
fn decrypt_ng_round_a(data: &[u8; 16], key: &[u32; 4], table: &[[u32; 256]; 16]) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (w, chunk) in out.chunks_exact_mut(4).enumerate() {
        let base = w * 4;
        let x = table[base][data[base] as usize]
            ^ table[base + 1][data[base + 1] as usize]
            ^ table[base + 2][data[base + 2] as usize]
            ^ table[base + 3][data[base + 3] as usize]
            ^ key[w];
        chunk.copy_from_slice(&x.to_le_bytes());
    }
    out
}

/// Rounds 2 through 15: the input bytes feeding each output word are permuted.
fn decrypt_ng_round_b(data: &[u8; 16], key: &[u32; 4], table: &[[u32; 256]; 16]) -> [u8; 16] {
    /// Which input byte (and matching table column) feeds each output word.
    const COLUMNS: [[usize; 4]; 4] = [[0, 7, 10, 13], [1, 4, 11, 14], [2, 5, 8, 15], [3, 6, 9, 12]];

    let mut out = [0u8; 16];
    for (w, chunk) in out.chunks_exact_mut(4).enumerate() {
        let mut x = key[w];
        for &c in &COLUMNS[w] {
            x ^= table[c][data[c] as usize];
        }
        chunk.copy_from_slice(&x.to_le_bytes());
    }
    out
}

/// The 17x16x256 table of decryption constants unpacked from `magic.dat`.
///
/// Boxed because it is 1.1 MB — returning it by value would blow the stack.
pub struct NgDecryptTables(pub Box<[[[u32; 256]; 16]; 17]>);

/// Decrypt a buffer with a 272-byte NG key. Port of `GTACrypto.DecryptNG`.
///
/// Data past the last whole 16-byte block is copied through unchanged, matching
/// the reference.
pub fn decrypt_ng(data: &[u8], key: &[u8; 272], tables: &NgDecryptTables) -> Vec<u8> {
    let mut key_uints = [0u32; 68];
    for (i, slot) in key_uints.iter_mut().enumerate() {
        *slot = u32::from_le_bytes([key[i * 4], key[i * 4 + 1], key[i * 4 + 2], key[i * 4 + 3]]);
    }

    let mut out = vec![0u8; data.len()];
    let whole = data.len() / 16;
    for i in 0..whole {
        let mut block = [0u8; 16];
        block.copy_from_slice(&data[i * 16..(i + 1) * 16]);
        let decrypted = decrypt_ng_block(&block, &key_uints, tables);
        out[i * 16..(i + 1) * 16].copy_from_slice(&decrypted);
    }

    let left = data.len() % 16;
    if left != 0 {
        let start = data.len() - left;
        out[start..].copy_from_slice(&data[start..]);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// AES-256 ECB against FIPS-197 / NIST SP 800-38A vector F.1.5, so the
    /// wiring is checked against a published vector rather than against itself.
    #[test]
    fn aes256_ecb_matches_the_nist_vector() {
        let key: [u8; 32] = [
            0x60, 0x3d, 0xeb, 0x10, 0x15, 0xca, 0x71, 0xbe, 0x2b, 0x73, 0xae, 0xf0, 0x85, 0x7d,
            0x77, 0x81, 0x1f, 0x35, 0x2c, 0x07, 0x3b, 0x61, 0x08, 0xd7, 0x2d, 0x98, 0x10, 0xa3,
            0x09, 0x14, 0xdf, 0xf4,
        ];
        let plain: [u8; 16] = [
            0x6b, 0xc1, 0xbe, 0xe2, 0x2e, 0x40, 0x9f, 0x96, 0xe9, 0x3d, 0x7e, 0x11, 0x73, 0x93,
            0x17, 0x2a,
        ];
        let expected: [u8; 16] = [
            0xf3, 0xee, 0xd1, 0xbd, 0xb5, 0xd2, 0xa0, 0x3c, 0x06, 0x4b, 0x5a, 0x7e, 0x3d, 0xb1,
            0x81, 0xf8,
        ];

        assert_eq!(encrypt_aes_data(&plain, &key, 1), expected.to_vec());
        assert_eq!(decrypt_aes_data(&expected, &key, 1), plain.to_vec());
    }

    #[test]
    fn aes_round_trips_over_multiple_rounds() {
        let key = [0x42u8; 32];
        let data: Vec<u8> = (0..64).map(|i| i as u8).collect();
        let encrypted = encrypt_aes_data(&data, &key, 16);
        assert_ne!(encrypted, data);
        assert_eq!(decrypt_aes_data(&encrypted, &key, 16), data);
    }

    /// A trailing partial block is passed through untouched, as the reference's
    /// `length - length % 16` truncation does.
    #[test]
    fn aes_leaves_a_trailing_partial_block_alone() {
        let key = [0x42u8; 32];
        let data: Vec<u8> = (0..20).map(|i| i as u8).collect();
        let encrypted = encrypt_aes_data(&data, &key, 1);
        assert_eq!(&encrypted[16..], &data[16..]);
        assert_eq!(decrypt_aes_data(&encrypted, &key, 1), data);
    }

    #[test]
    fn aes_passes_through_buffers_shorter_than_a_block() {
        let key = [0x42u8; 32];
        let data = [1u8, 2, 3];
        assert_eq!(decrypt_aes_data(&data, &key, 1), data.to_vec());
    }

    fn identity_lut() -> [u8; 256] {
        let mut lut = [0u8; 256];
        for (i, slot) in lut.iter_mut().enumerate() {
            *slot = i as u8;
        }
        lut
    }

    /// `GTA5Hash` is the Jenkins hash with a byte substitution in front of it.
    ///
    /// The reference writes its steps as multiplications, which hides this:
    /// `1025 * x == x + (x << 10)`, `9 * x == x + (x << 3)`, and
    /// `32769 * x == x + (x << 15)` — precisely Jenkins' three mixing steps.
    /// So with an identity LUT the two functions must agree exactly, which
    /// checks this port against `gen_hash`, itself pinned to the reference DLL.
    #[test]
    fn gta5_hash_reduces_to_jenkins_under_an_identity_lut() {
        let lut = identity_lut();
        for name in ["", "x64a.rpf", "common.rpf", "vehicles.meta", "0123456789"] {
            assert_eq!(
                gta5_hash(name, &lut),
                crate::utils::gen_hash(name),
                "identity LUT must reduce to Jenkins for {name:?}"
            );
        }
    }

    /// It is the LUT, and only the LUT, that separates the two hashes. The real
    /// table is a permutation, so in practice they disagree — but a port that
    /// dropped the LUT lookup would still pass the test above.
    #[test]
    fn the_lut_is_what_separates_it_from_jenkins() {
        let mut lut = identity_lut();
        lut.reverse();
        let name = "x64a.rpf";
        assert_ne!(gta5_hash(name, &lut), crate::utils::gen_hash(name));
        assert_ne!(gta5_hash(name, &lut), gta5_hash(name, &identity_lut()));
    }

    #[test]
    fn gta5_hash_is_deterministic_and_name_sensitive() {
        let lut = identity_lut();
        assert_eq!(gta5_hash("", &lut), 0);
        assert_eq!(gta5_hash("x64a.rpf", &lut), gta5_hash("x64a.rpf", &lut));
        assert_ne!(gta5_hash("x64a.rpf", &lut), gta5_hash("x64b.rpf", &lut));
    }
}
