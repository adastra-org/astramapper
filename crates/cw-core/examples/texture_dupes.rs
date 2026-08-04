//! Which dictionaries hold a given texture, and at what size.
//!
//! ```text
//! cargo run --release --example texture_dupes -- im_freeway_barrier001_n dt1_00_3
//! ```

use std::path::PathBuf;

use cw_core::filetype::FileKind;
use cw_core::filetypes::index_archetypes;
use cw_core::resources::read_textures;
use cw_core::rpf::RpfManager;
use cw_core::utils::{gen_hash, GtaKeys};

const DEFAULT_GTA_PATH: &str = r"E:\SteamLibrary\steamapps\common\Grand Theft Auto V";

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let texture = args.first().expect("a texture name").to_lowercase();
    let model = args.get(1).map(|s| s.to_lowercase());

    let folder =
        PathBuf::from(std::env::var("ASTRAMAPPER_GTA_PATH").unwrap_or(DEFAULT_GTA_PATH.into()));
    let keys = GtaKeys::from_exe(&std::fs::read(folder.join("GTA5.exe")).expect("exe")).expect("keys");
    let manager = RpfManager::scan(&folder, &keys).expect("scan");
    let wanted = gen_hash(&texture);

    if let Some(model) = &model {
        let archetypes = index_archetypes(&manager, &keys);
        match archetypes.all.get(&gen_hash(model)) {
            Some(a) => {
                let named = manager.ytd_by_hash().get(&a.texture_dictionary);
                println!(
                    "archetype {model}: textureDictionary {:08x} -> {}",
                    a.texture_dictionary,
                    named.map(String::as_str).unwrap_or("(not found)")
                );
            }
            None => println!("archetype {model}: not in the index"),
        }
    }

    println!("\ndictionaries holding {texture} ({wanted:08x}):");
    for archive in &manager.archives {
        for entry in &archive.entries {
            if !entry.name_lower.ends_with(".ytd") {
                continue;
            }
            let Some(r) = manager.find(&entry.path) else {
                continue;
            };
            let Some((size, _)) = entry.resource_segments() else {
                continue;
            };
            let Ok(data) = manager.extract(r, &keys) else {
                continue;
            };
            let Ok(dict) = read_textures(FileKind::Texture, &data, size) else {
                continue;
            };
            if let Some(t) = dict.lookup(wanted) {
                println!(
                    "  {}x{} {:?} levels={}  {}",
                    t.width,
                    t.height,
                    t.format.map(|f| f.label()).unwrap_or("?"),
                    t.levels,
                    entry.path
                );
            }
        }
    }
}
