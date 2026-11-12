//! Pull plain files out of the game's archives by file name.
//!
//! ```text
//! rpf_extract <out_dir> <file name> [<file name> ...]
//! ASTRAMAPPER_GTA_PATH="..." rpf_extract out materials.dat materialfx.dat
//! ```
//!
//! Every archive that holds a match is written, as
//! `<out_dir>/<archive path with / as _>__<file name>`, because the base game
//! and `update.rpf` each carry their own copy and the update one is the one the
//! game uses. Read-only on the install.

use std::path::PathBuf;

use cw_core::rpf::{EntryRef, RpfManager};
use cw_core::utils::GtaKeys;

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

fn main() {
    let mut args = std::env::args().skip(1);
    let Some(out_dir) = args.next().map(PathBuf::from) else {
        eprintln!("usage: rpf_extract <out_dir> <file name> [...]");
        return;
    };
    let wanted: Vec<String> = args.map(|a| a.to_lowercase()).collect();
    if wanted.is_empty() {
        eprintln!("no file names given");
        return;
    }
    let folder =
        PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.into()));
    let Ok(exe_data) = std::fs::read(folder.join("GTA5.exe")) else {
        eprintln!("no install at {}", folder.display());
        return;
    };
    let keys = GtaKeys::from_exe(&exe_data).expect("keys");
    let manager = RpfManager::scan(&folder, &keys).expect("scan");
    std::fs::create_dir_all(&out_dir).expect("create out dir");

    let mut found = 0;
    for (a, archive) in manager.archives.iter().enumerate() {
        for (e, entry) in archive.entries.iter().enumerate() {
            if !entry.is_file() || !wanted.contains(&entry.name_lower) {
                continue;
            }
            let reference = EntryRef {
                archive: a,
                entry: e,
            };
            match manager.extract(reference, &keys) {
                Ok(bytes) => {
                    let tag = archive.path.replace(['/', '\\'], "_");
                    let target = out_dir.join(format!("{tag}__{}", entry.name_lower));
                    std::fs::write(&target, &bytes).expect("write");
                    println!(
                        "{}  {} bytes  <- {}",
                        target.display(),
                        bytes.len(),
                        entry.path
                    );
                    found += 1;
                }
                Err(err) => println!("FAILED {}: {err}", entry.path),
            }
        }
    }
    println!("{found} file(s) written");
}
