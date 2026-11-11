//! Which archetypes are ground, told by the shader that draws them.
//!
//! The terrain layer (#5) is the placements the world index keeps resident
//! map-wide. It used to be the ones whose archetype name has a terrain token,
//! which finds 395 placements; the shaders find 5,355 (measured by
//! `ground_census_over_the_base_game`). The rest of the ground lived in ordinary
//! cells and left with them, so flying away from an area took its ground with it.
//!
//! A model is ground when any of its parts binds a `terrain_cb_w_4lyr` layer
//! (`TextureSampler_layer0` to `3`), the signal the reference's own terrain
//! shader keys on. The shaders live in the model file, not in the `.ytyp`, so
//! telling needs every placed model parsed once: 56,166 files in about 20 s on
//! the release build (8 threads, drawable only, no mesh or texture work). The
//! answer is a list of names, a few hundred kilobytes, cached per install.

use std::collections::{BTreeMap, HashSet};

use cw_core::filetype::FileKind;
use cw_core::resources::drawable::{Drawable, TERRAIN_LAYER_SAMPLERS};
use cw_core::resources::read_named_drawables;
use cw_core::rpf::RpfManager;
use cw_core::utils::GtaKeys;
use rayon::prelude::*;

use crate::explorer::Loaded;
use crate::world::find_model_for;

/// Bump when the rule that decides ground changes, so a cached list is rebuilt.
const GROUND_FORMAT: u32 = 1;

/// Whether a drawable binds any ground layer.
pub fn is_ground_drawable(drawable: &Drawable) -> bool {
    drawable.shaders.iter().any(|shader| {
        TERRAIN_LAYER_SAMPLERS
            .iter()
            .any(|param| shader.texture_for(*param).is_some())
    })
}

/// Whether the model file at `path` draws ground. A file that cannot be read is
/// not ground: the name stays on its token and nothing is lost.
fn model_is_ground(path: &str, keys: &GtaKeys, manager: &RpfManager) -> bool {
    let Some(entry_ref) = manager.find(path) else {
        return false;
    };
    let entry = manager.entry(entry_ref);
    let kind = FileKind::from_name(&entry.name_lower);
    if !matches!(kind, FileKind::Model | FileKind::ModelDictionary) {
        return false;
    }
    let Some((system_size, _)) = entry.resource_segments() else {
        return false;
    };
    let Ok(data) = manager.extract(entry_ref, keys) else {
        return false;
    };
    read_named_drawables(kind, &data, system_size)
        .map(|named| named.iter().any(|d| is_ground_drawable(&d.drawable)))
        .unwrap_or(false)
}

/// The archetype names, among `names`, whose model draws ground.
pub fn ground_names(
    names: &[String],
    keys: &GtaKeys,
    manager: &RpfManager,
    pool: Option<&rayon::ThreadPool>,
) -> HashSet<String> {
    let run = || -> HashSet<String> {
        // One file can serve several names; parse each file once.
        let mut by_path: BTreeMap<String, Vec<&String>> = BTreeMap::new();
        for name in names {
            if let Some(path) = find_model_for(manager, name) {
                by_path.entry(path).or_default().push(name);
            }
        }
        by_path
            .par_iter()
            .filter(|(path, _)| model_is_ground(path, keys, manager))
            .flat_map_iter(|(_, names)| names.iter().map(|n| (*n).clone()))
            .collect()
    };
    match pool {
        Some(pool) => pool.install(run),
        None => run(),
    }
}

/// [`ground_names`] through the install's cache, when it has one.
pub fn cached_ground_names(loaded: &Loaded, group: &str, names: &[String]) -> HashSet<String> {
    let compute = || {
        crate::perf::span("world_index.ground_census", || {
            ground_names(
                names,
                &loaded.keys,
                &loaded.manager,
                crate::world_index::model_pool(),
            )
        })
    };
    let Some(cache) = &loaded.cache else {
        return compute();
    };
    let label = format!("ground:{GROUND_FORMAT}:{group}:{}", names.len());
    let key = cw_cache::CacheKey::named(
        cw_cache::Kind::Index,
        crate::cache::install_stamp(std::path::Path::new(&loaded.folder)),
        &label,
    );
    let bytes: Result<Vec<u8>, ()> = cache.get_or_insert_with(key, cw_cache::Kind::Index, || {
        let mut found: Vec<String> = compute().into_iter().collect();
        found.sort();
        Ok(found.join("\n").into_bytes())
    });
    match bytes {
        Ok(bytes) => String::from_utf8_lossy(&bytes)
            .lines()
            .filter(|l| !l.is_empty())
            .map(str::to_owned)
            .collect(),
        Err(()) => HashSet::new(),
    }
}
