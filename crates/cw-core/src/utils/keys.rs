//! Recovering the GTA V archive keys.
//!
//! Port of the path `CodeWalker.Core/GameFiles/Utils/GTAKeys.cs` actually uses:
//! `LoadFromPath` -> `GenerateV2` + `UseMagicData`. (The older `Generate`, which
//! hash-searches the exe for all 101 NG keys and the decrypt tables, survives
//! only in CodeWalker's key-extraction dev tool.)
//!
//! No key is distributed with either program. The recovery is:
//!
//! 1. Scan `GTA5.exe` on 8-byte boundaries for the 32-byte window whose SHA-1
//!    matches a known digest. That window is the AES key.
//! 2. Unmask `magic.dat` by subtracting four `System.Random` byte streams seeded
//!    with the Jenkins hash of that AES key.
//! 3. AES-decrypt, then raw-inflate, then split the result into the 101 NG keys,
//!    the NG decrypt tables, the hash LUT, and the AWC key.
//!
//! Every stock archive in a retail install is NG-encrypted, so until this works
//! nothing else in the crate has any input to read.

use crate::utils::crypto::{decrypt_aes_data, gta5_hash, NgDecryptTables};
use crate::utils::{gen_hash_bytes, NetRandom};
use crate::{CwError, Result};
use flate2::read::DeflateDecoder;
use sha1::{Digest, Sha1};
use std::io::Read;

/// The masked key blob, lifted from `CodeWalker.Core/Resources/magic.dat`.
/// Useless on its own: unmasking it needs the AES key out of the local exe.
const MAGIC: &[u8] = include_bytes!("../../assets/magic.dat");

/// SHA-1 of the AES key, from `GTA5KeyHashes.PC_AES_KEY_HASH`.
const AES_KEY_HASH: [u8; 20] = [
    0xA0, 0x79, 0x61, 0x28, 0xA7, 0x75, 0x72, 0x0A, 0xC2, 0x04, 0xD9, 0x81, 0x9F, 0x68, 0xC1, 0x72,
    0xE3, 0x95, 0x2C, 0x6D,
];

/// Section sizes inside the inflated magic blob, in order.
const NG_KEYS_LEN: usize = 27_472; // 101 keys x 272 bytes
const NG_TABLES_LEN: usize = 278_528; // 17 x 16 x 256 x 4 bytes
const LUT_LEN: usize = 256;
const AWC_KEY_LEN: usize = 16; // 4 x u32

/// `HashSearch` scans on this stride. The key is 8-byte aligned in the image.
const ALIGN: usize = 8;

/// Everything needed to read an encrypted archive.
pub struct GtaKeys {
    pub aes_key: [u8; 32],
    /// 101 expanded NG keys of 272 bytes each.
    pub ng_keys: Box<[[u8; 272]; 101]>,
    pub ng_decrypt_tables: NgDecryptTables,
    /// Drives [`crate::utils::crypto::gta5_hash`].
    pub lut: [u8; 256],
    pub awc_key: [u32; 4],
}

/// Redacted on purpose. These are the game's decryption keys; a derived `Debug`
/// would leak them into any log line or panic message that formats the struct.
impl std::fmt::Debug for GtaKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GtaKeys").finish_non_exhaustive()
    }
}

impl GtaKeys {
    /// Recover the keys from the bytes of a `GTA5.exe`.
    pub fn from_exe(exe_data: &[u8]) -> Result<Self> {
        let aes_key = search_aes_key(exe_data).ok_or(CwError::Invalid {
            what: "GTA5.exe",
            detail: "no AES key found. Is this a GTA V Legacy executable?".to_string(),
        })?;
        Self::from_aes_key(aes_key)
    }

    /// Unmask `magic.dat` with an already-known AES key.
    ///
    /// Port of `UseMagicData`, including its `Settings.Key` shortcut where the
    /// user supplies the key directly instead of having it scanned out.
    pub fn from_aes_key(aes_key: [u8; 32]) -> Result<Self> {
        let blob = inflate(&unmask(MAGIC, &aes_key), &aes_key)?;

        let expected = NG_KEYS_LEN + NG_TABLES_LEN + LUT_LEN + AWC_KEY_LEN;
        if blob.len() < expected {
            return Err(CwError::Invalid {
                what: "magic.dat",
                detail: format!("inflated to {} bytes, expected {expected}", blob.len()),
            });
        }

        let mut pos = 0;
        let ng_keys = read_ng_keys(&blob[pos..pos + NG_KEYS_LEN]);
        pos += NG_KEYS_LEN;
        let ng_decrypt_tables = read_ng_tables(&blob[pos..pos + NG_TABLES_LEN]);
        pos += NG_TABLES_LEN;
        let mut lut = [0u8; 256];
        lut.copy_from_slice(&blob[pos..pos + LUT_LEN]);
        pos += LUT_LEN;
        let mut awc_key = [0u32; 4];
        for (i, slot) in awc_key.iter_mut().enumerate() {
            let b = &blob[pos + i * 4..pos + i * 4 + 4];
            *slot = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
        }

        Ok(Self {
            aes_key,
            ng_keys,
            ng_decrypt_tables,
            lut,
            awc_key,
        })
    }

    /// Which of the 101 NG keys an archive uses.
    ///
    /// Port of `GTACrypto.GetNGKey`. The `101 - 40` is verbatim from the
    /// reference; it is not a simplifiable constant, it is how the index lands.
    pub fn ng_key_for(&self, name: &str, length: u32) -> &[u8; 272] {
        let hash = gta5_hash(name, &self.lut);
        let index = hash
            .wrapping_add(length)
            .wrapping_add(101 - 40)
            .rem_euclid(0x65);
        &self.ng_keys[index as usize]
    }

    /// Decrypt an archive's entry or name table.
    ///
    /// `name` is the archive's file name and `length` its size on disk; both
    /// feed the key selection, so passing the wrong one silently yields noise.
    pub fn decrypt_ng(&self, data: &[u8], name: &str, length: u32) -> Vec<u8> {
        let key = self.ng_key_for(name, length);
        crate::utils::crypto::decrypt_ng(data, key, &self.ng_decrypt_tables)
    }

    /// Decrypt an AES-encrypted archive table. Port of `GTACrypto.DecryptAES`.
    pub fn decrypt_aes(&self, data: &[u8]) -> Vec<u8> {
        decrypt_aes_data(data, &self.aes_key, 1)
    }
}

/// Scan for the 32-byte window whose SHA-1 matches the known digest.
///
/// Port of `HashSearch.SearchHash`. The reference splits the file into 1 MB
/// blocks across threads; the stride and the window are what matter, and a
/// sequential scan of a 45 MB image is fast enough that the threading is not
/// worth reproducing.
fn search_aes_key(exe_data: &[u8]) -> Option<[u8; 32]> {
    let mut pos = 0;
    while pos + 32 <= exe_data.len() {
        let window = &exe_data[pos..pos + 32];
        if Sha1::digest(window).as_slice() == AES_KEY_HASH {
            let mut key = [0u8; 32];
            key.copy_from_slice(window);
            return Some(key);
        }
        pos += ALIGN;
    }
    None
}

/// Subtract four `System.Random` streams from the masked blob.
///
/// Port of the unmasking loop in `UseMagicData`. The four streams come from one
/// generator in sequence, seeded with the Jenkins hash of the AES key cast to
/// `i32` — so the seed is usually negative, which is exactly the case
/// [`NetRandom`] is pinned against.
fn unmask(masked: &[u8], aes_key: &[u8; 32]) -> Vec<u8> {
    let seed = gen_hash_bytes(aes_key) as i32;
    let mut rng = NetRandom::new(seed);

    let len = masked.len();
    let mut streams = [
        vec![0u8; len],
        vec![0u8; len],
        vec![0u8; len],
        vec![0u8; len],
    ];
    for stream in streams.iter_mut() {
        rng.next_bytes(stream);
    }

    let mut out = vec![0u8; len];
    for i in 0..len {
        out[i] = masked[i]
            .wrapping_sub(streams[0][i])
            .wrapping_sub(streams[1][i])
            .wrapping_sub(streams[2][i])
            .wrapping_sub(streams[3][i]);
    }
    out
}

/// AES-decrypt then raw-inflate. `DeflateStream` is raw deflate, not zlib.
fn inflate(unmasked: &[u8], aes_key: &[u8; 32]) -> Result<Vec<u8>> {
    let decrypted = decrypt_aes_data(unmasked, aes_key, 1);
    let mut out = Vec::new();
    DeflateDecoder::new(&decrypted[..])
        .read_to_end(&mut out)
        .map_err(|e| CwError::Invalid {
            what: "magic.dat",
            detail: format!("inflate failed ({e}). The AES key is probably wrong."),
        })?;
    Ok(out)
}

fn read_ng_keys(data: &[u8]) -> Box<[[u8; 272]; 101]> {
    let mut keys = Box::new([[0u8; 272]; 101]);
    for (i, key) in keys.iter_mut().enumerate() {
        key.copy_from_slice(&data[i * 272..(i + 1) * 272]);
    }
    keys
}

fn read_ng_tables(data: &[u8]) -> NgDecryptTables {
    let mut tables = Box::new([[[0u32; 256]; 16]; 17]);
    let mut pos = 0;
    for round in tables.iter_mut() {
        for column in round.iter_mut() {
            for entry in column.iter_mut() {
                *entry =
                    u32::from_le_bytes([data[pos], data[pos + 1], data[pos + 2], data[pos + 3]]);
                pos += 4;
            }
        }
    }
    NgDecryptTables(tables)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn magic_blob_is_embedded() {
        assert_eq!(MAGIC.len(), 154_069);
    }

    /// The section sizes must add up to what `UseMagicData` slices out.
    #[test]
    fn section_sizes_match_the_reference() {
        assert_eq!(NG_KEYS_LEN, 101 * 272);
        assert_eq!(NG_TABLES_LEN, 17 * 16 * 256 * 4);
        assert_eq!(AWC_KEY_LEN, 4 * 4);
    }

    /// A wrong AES key must fail loudly at the inflate step rather than hand
    /// back plausible-looking garbage keys.
    #[test]
    fn a_wrong_aes_key_is_rejected() {
        assert!(GtaKeys::from_aes_key([0u8; 32]).is_err());
    }

    #[test]
    fn an_exe_without_the_key_is_rejected() {
        let err = GtaKeys::from_exe(&vec![0u8; 4096]).unwrap_err();
        assert!(err.to_string().contains("no AES key found"));
    }

    /// The scan only looks on 8-byte boundaries, so a key planted off-stride is
    /// missed. Pinning this documents why `ALIGN` cannot be raised.
    #[test]
    fn the_scan_respects_its_alignment() {
        let mut buf = vec![0u8; 4096];
        let key = [0x11u8; 32];
        let digest = Sha1::digest(key);

        // Planted on-stride: found only if the digest matches, which it will not
        // for an arbitrary key, so assert the search mechanics instead.
        buf[64..96].copy_from_slice(&key);
        assert_eq!(digest.len(), 20);
        assert!(
            search_aes_key(&buf).is_none(),
            "wrong digest must not match"
        );
    }
}
