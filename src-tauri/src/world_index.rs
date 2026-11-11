//! The world's spatial index, and the commands that serve it by cell.
//!
//! # Why this exists
//!
//! The frontend used to read every placed map and hold every placement of every
//! one of them: 763,716 of them over the base game, ~830 MB of JS heap and 300 ms
//! collection pauses (`docs/streaming.md`). It also fetched each model with one
//! IPC call of its own, at a cost per call that no amount of backend speed
//! removes.
//!
//! So the placements live here instead, in a [`WorldIndex`]: every placed map
//! read once (from the same cached envelopes `world_map` serves) into one set of
//! columns, cut into hexagonal cells by [`crate::cells::Partition`]. The frontend
//! asks for the cells around the camera and holds only those.
//!
//! # The three commands
//!
//! - [`world_index_build`] reads a content group into the index and returns its
//!   *partition*: one row per cell (level, position, how many placements), and
//!   nothing else. That is all the frontend needs to choose a working set, and
//!   it is the same partition the Hex view draws.
//! - [`world_index_cells`] returns, for a set of cells, their placements as a
//!   columnar envelope **and** the models those cells need, in one reply. A
//!   model rides as a nested `Kind::Model` envelope (the very bytes
//!   `world_model` would have returned), so there is no second round trip and no
//!   second encoding to keep in step.
//! - The terrain layer is a pseudo-cell, [`TERRAIN_LEVEL`]. Terrain is wanted
//!   map-wide whatever the camera does (#5), so it is excluded from the
//!   partition and fetched as one cell the frontend never evicts.
//!
//! # What a cell reply holds
//!
//! The first eleven sections are the columns of `crate::placements`, in the same
//! order and widths, over the concatenation of the cells' placements; section 11
//! is `mapSlot` (`u32`), an index into the metadata's `maps` table. The metadata
//! says which rows are which cell (`cells[].start/count`). Everything after
//! section 11 is a model, named by `models[].section`.
//!
//! Names the frontend already holds (`known`) are left out of `assets`, and
//! their models are not sent again. A name the backend cannot resolve comes back
//! with a `null` path, so the frontend can stop asking about it.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};

use cw_core::wire::{Envelope, Kind};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};
use tauri::State;

use crate::cells::{CellId, Partition, PartitionParams, DEFAULT_FINE, DEFAULT_LEVELS};
use crate::clusters::{Cluster, Clusters, Regions};
use crate::explorer::GameState;
use crate::placements::DecodedMap;
use crate::world::{blocking, find_model_for, map_envelope, ymaps_in_group, GpuState};

/// The level number that names the terrain layer rather than a partition level.
pub const TERRAIN_LEVEL: u8 = 255;

/// One map in the index.
#[derive(Debug, Clone)]
pub struct MapInfo {
    pub path: String,
    pub name: String,
    pub count: usize,
    pub extents_min: [f32; 3],
    pub extents_max: [f32; 3],
}

/// Every placement, one column per field, with names and LOD labels interned
/// across the whole index.
#[derive(Debug, Default)]
pub struct PlacementStore {
    pub names: Vec<String>,
    pub name_hashes: Vec<u32>,
    /// Per name, the archetype class the map envelopes carried (0 for none).
    pub name_classes: Vec<u8>,
    pub lod_levels: Vec<String>,
    name_slot: HashMap<(String, u32), u32>,
    lod_slot: HashMap<String, u16>,

    pub map: Vec<u32>,
    pub index: Vec<u32>,
    pub name_of: Vec<u32>,
    pub flags: Vec<u8>,
    pub position: Vec<[f32; 3]>,
    pub rotation: Vec<[f32; 4]>,
    pub scale_xy: Vec<f32>,
    pub scale_z: Vec<f32>,
    pub parent: Vec<i32>,
    pub lod_of: Vec<u16>,
    pub lod_dist: Vec<f32>,
    pub children: Vec<u32>,
}

impl PlacementStore {
    pub fn len(&self) -> usize {
        self.index.len()
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    /// Append one map's placements. Fails only past 65,535 distinct LOD labels,
    /// which a corrupt file can reach and a wrapped index would hide.
    pub fn append(&mut self, map_slot: u32, decoded: &DecodedMap) -> Result<(), String> {
        // Translate this map's slots into the store's.
        let mut names = Vec::with_capacity(decoded.names.len());
        for (i, (name, hash)) in decoded.names.iter().zip(&decoded.name_hashes).enumerate() {
            let class = decoded.name_classes.get(i).copied().unwrap_or(0);
            let next = self.names.len() as u32;
            let slot = *self
                .name_slot
                .entry((name.clone(), *hash))
                .or_insert_with(|| {
                    self.names.push(name.clone());
                    self.name_hashes.push(*hash);
                    self.name_classes.push(class);
                    next
                });
            // The class is the archetype's, so every map agrees; a map that
            // could not index it says none, which must not erase a class.
            if class != 0 && self.name_classes[slot as usize] == 0 {
                self.name_classes[slot as usize] = class;
            }
            names.push(slot);
        }
        let mut lods = Vec::with_capacity(decoded.lod_levels.len());
        for label in &decoded.lod_levels {
            let slot = match self.lod_slot.get(label) {
                Some(slot) => *slot,
                None => {
                    let slot = u16::try_from(self.lod_levels.len())
                        .map_err(|_| "more than 65,535 distinct LOD levels".to_string())?;
                    self.lod_levels.push(label.clone());
                    self.lod_slot.insert(label.clone(), slot);
                    slot
                }
            };
            lods.push(slot);
        }

        let count = decoded.len();
        self.map.extend(std::iter::repeat_n(map_slot, count));
        self.index.extend_from_slice(&decoded.index);
        for &slot in &decoded.name_slot {
            let mapped = names
                .get(slot as usize)
                .copied()
                .ok_or_else(|| format!("{} names a missing archetype", decoded.path))?;
            self.name_of.push(mapped);
        }
        self.flags.extend_from_slice(&decoded.flags);
        self.position.extend_from_slice(&decoded.position);
        self.rotation.extend_from_slice(&decoded.rotation);
        self.scale_xy.extend_from_slice(&decoded.scale_xy);
        self.scale_z.extend_from_slice(&decoded.scale_z);
        self.parent.extend_from_slice(&decoded.parent);
        for &slot in &decoded.lod_slot {
            let mapped = lods
                .get(slot as usize)
                .copied()
                .ok_or_else(|| format!("{} names a missing LOD level", decoded.path))?;
            self.lod_of.push(mapped);
        }
        self.lod_dist.extend_from_slice(&decoded.lod_dist);
        self.children.extend_from_slice(&decoded.children);
        Ok(())
    }
}

/// Whether an archetype name is ground: any whole-word token of `terrain`,
/// `ter`, `terr` or `grnd`. The same rule as `classifyName` in `world.ts`, which
/// is the only thing deciding it until the backend has real flags (#9); the two
/// are pinned by tests against the same names.
pub fn is_terrain_name(name: &str) -> bool {
    name.to_ascii_lowercase()
        .split(|c: char| !c.is_ascii_lowercase())
        .any(|token| matches!(token, "terrain" | "ter" | "terr" | "grnd"))
}

/// A cell as the frontend names it. `level` is [`TERRAIN_LEVEL`] for the ground
/// layer, whose `q` and `r` mean nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
pub struct CellRef {
    pub level: u8,
    pub q: i32,
    pub r: i32,
}

/// Where a cell's rows sit in a reply.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CellSpan {
    pub level: u8,
    pub q: i32,
    pub r: i32,
    pub start: usize,
    pub count: usize,
}

/// The rows a set of cells holds, in request order.
#[derive(Debug, Default)]
pub struct Gathered {
    pub spans: Vec<CellSpan>,
    /// Store ids, cell after cell.
    pub ids: Vec<u32>,
}

/// A model name resolved to a file, or to nothing.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Asset {
    pub name: String,
    pub path: Option<String>,
}

/// What building the index cost and found, for the log and the gate.
#[derive(Debug, Clone, Default)]
pub struct BuildReport {
    pub maps: usize,
    pub failed: usize,
    pub placements: usize,
    pub terrain: usize,
    pub leaves: usize,
}

pub struct WorldIndex {
    /// The install this was built from. A different folder means a stale index.
    pub folder: String,
    pub group: String,
    pub maps: Vec<MapInfo>,
    pub store: PlacementStore,
    /// Ids of the ground placements, the layer outside the partition.
    pub terrain: Vec<u32>,
    pub partition: Partition,
    /// The partition's leaves grouped into named zones.
    pub clusters: Clusters,
    pub report: BuildReport,
}

impl WorldIndex {
    /// Index decoded maps: intern their placements, set the ground aside, and
    /// partition the rest.
    pub fn build(
        folder: &str,
        group: &str,
        decoded: &[DecodedMap],
        failed: usize,
        params: PartitionParams,
    ) -> Result<Self, String> {
        let mut store = PlacementStore::default();
        let mut maps = Vec::with_capacity(decoded.len());
        for (slot, map) in decoded.iter().enumerate() {
            store.append(slot as u32, map)?;
            maps.push(MapInfo {
                path: map.path.clone(),
                name: map.name.clone(),
                count: map.len(),
                extents_min: map.extents_min,
                extents_max: map.extents_max,
            });
        }

        // Classified once per distinct name, not per placement.
        let terrain_name: Vec<bool> = store.names.iter().map(|n| is_terrain_name(n)).collect();
        let mut terrain = Vec::new();
        let mut rest = Vec::with_capacity(store.len());
        for (id, &name) in store.name_of.iter().enumerate() {
            if terrain_name[name as usize] {
                terrain.push(id as u32);
            } else {
                rest.push(id as u32);
            }
        }

        let xs: Vec<f32> = store.position.iter().map(|p| p[0]).collect();
        let ys: Vec<f32> = store.position.iter().map(|p| p[1]).collect();
        let zs: Vec<f32> = store.position.iter().map(|p| p[2]).collect();
        let partition = Partition::build(&xs, &ys, &zs, &rest, params);
        let clusters = Clusters::build(Regions::builtin(), &partition, &store.name_of);

        let report = BuildReport {
            maps: maps.len(),
            failed,
            placements: store.len(),
            terrain: terrain.len(),
            leaves: partition.leaves.len(),
        };
        Ok(Self {
            folder: folder.to_string(),
            group: group.to_string(),
            maps,
            store,
            terrain,
            partition,
            clusters,
            report,
        })
    }

    /// The partition as an envelope: one row per leaf, one per split cell.
    ///
    /// Sections: 0 level `u8`, 1 q `i32`, 2 r `i32`, 3 count `u32`, 4 mean z
    /// `f32` — per leaf — then 5 level `u8`, 6 q `i32`, 7 r `i32` per split
    /// cell, then 8 cluster `u16` per leaf: an index into the metadata's
    /// `clusters` (zone clusters, `crate::clusters`). Centres are not sent:
    /// `hexCenter` makes them from `(q, r)` and the level's size, and both sides
    /// compute the same number.
    pub fn partition_envelope(&self) -> Vec<u8> {
        let leaves = &self.partition.leaves;
        let mut level = Vec::with_capacity(leaves.len());
        let mut q = Vec::with_capacity(leaves.len() * 4);
        let mut r = Vec::with_capacity(leaves.len() * 4);
        let mut count = Vec::with_capacity(leaves.len() * 4);
        let mut z = Vec::with_capacity(leaves.len() * 4);
        for leaf in leaves {
            level.push(leaf.level);
            q.extend_from_slice(&leaf.q.to_le_bytes());
            r.extend_from_slice(&leaf.r.to_le_bytes());
            count.extend_from_slice(&leaf.count.to_le_bytes());
            z.extend_from_slice(&leaf.z.to_le_bytes());
        }
        let splits = &self.partition.splits;
        let mut split_level = Vec::with_capacity(splits.len());
        let mut split_q = Vec::with_capacity(splits.len() * 4);
        let mut split_r = Vec::with_capacity(splits.len() * 4);
        for &(l, sq, sr) in splits {
            split_level.push(l);
            split_q.extend_from_slice(&sq.to_le_bytes());
            split_r.extend_from_slice(&sr.to_le_bytes());
        }

        let mut cluster = Vec::with_capacity(leaves.len() * 2);
        for &at in &self.clusters.of_leaf {
            cluster.extend_from_slice(&at.to_le_bytes());
        }

        #[derive(Serialize)]
        #[serde(rename_all = "camelCase")]
        struct Meta<'a> {
            group: &'a str,
            sizes: &'a [f64],
            threshold: usize,
            maps: usize,
            failed: usize,
            placements: usize,
            /// Ground placements: held outside the partition, as one layer.
            terrain: usize,
            leaves: usize,
            splits: usize,
            /// Zone clusters; a leaf's section-8 value indexes this list.
            clusters: &'a [Cluster],
        }
        let meta = Meta {
            group: &self.group,
            sizes: &self.partition.params.sizes,
            threshold: self.partition.params.threshold,
            maps: self.report.maps,
            failed: self.report.failed,
            placements: self.report.placements,
            terrain: self.report.terrain,
            leaves: leaves.len(),
            splits: splits.len(),
            clusters: &self.clusters.list,
        };
        let meta = serde_json::to_string(&meta).unwrap_or_else(|_| "{}".to_string());

        let mut envelope = Envelope::new();
        for section in [
            level,
            q,
            r,
            count,
            z,
            split_level,
            split_q,
            split_r,
            cluster,
        ] {
            envelope.push(section);
        }
        envelope.finish(Kind::Cells, &meta)
    }

    /// The rows of a set of cells, in request order. A cell the partition has no
    /// leaf for is an empty span rather than an error: it was asked for because
    /// the frontend saw it in an older partition, and nothing is the right answer.
    pub fn gather(&self, cells: &[CellRef]) -> Gathered {
        let mut out = Gathered::default();
        for cell in cells {
            let ids: &[u32] = if cell.level == TERRAIN_LEVEL {
                &self.terrain
            } else {
                let id: CellId = (cell.level, cell.q, cell.r);
                self.partition
                    .leaf(id)
                    .map_or(&[][..], |leaf| self.partition.members_of(leaf))
            };
            out.spans.push(CellSpan {
                level: cell.level,
                q: cell.q,
                r: cell.r,
                start: out.ids.len(),
                count: ids.len(),
            });
            out.ids.extend_from_slice(ids);
        }
        out
    }

    /// The distinct archetype names a gather holds, first-seen order.
    pub fn names_in(&self, gathered: &Gathered) -> Vec<&str> {
        let mut seen = HashSet::new();
        let mut out = Vec::new();
        for &id in &gathered.ids {
            let slot = self.store.name_of[id as usize];
            if seen.insert(slot) {
                out.push(self.store.names[slot as usize].as_str());
            }
        }
        out
    }

    /// Encode a gather as the batched reply: columns, then `models`.
    ///
    /// `assets` says which file each unknown name is, and `models` carries the
    /// compiled bytes of the files worth sending, keyed by path. A path in
    /// `assets` with no entry in `models` is one the frontend fetches itself.
    pub fn encode_batch(
        &self,
        gathered: &Gathered,
        assets: Vec<Asset>,
        models: Vec<(String, Vec<u8>)>,
    ) -> Result<Vec<u8>, String> {
        let store = &self.store;
        let count = gathered.ids.len();

        // Local tables: only what these rows use, in first-seen order.
        let mut names: Vec<&str> = Vec::new();
        let mut name_hashes: Vec<u32> = Vec::new();
        let mut name_classes: Vec<u8> = Vec::new();
        let mut name_local: HashMap<u32, u32> = HashMap::new();
        let mut lod_levels: Vec<&str> = Vec::new();
        let mut lod_local: HashMap<u16, u16> = HashMap::new();
        let mut map_refs: Vec<MapRef> = Vec::new();
        let mut map_local: HashMap<u32, u32> = HashMap::new();

        let mut col_index = Vec::with_capacity(count * 4);
        let mut col_name = Vec::with_capacity(count * 4);
        let mut col_flags = Vec::with_capacity(count);
        let mut col_position = Vec::with_capacity(count * 12);
        let mut col_rotation = Vec::with_capacity(count * 16);
        let mut col_scale_xy = Vec::with_capacity(count * 4);
        let mut col_scale_z = Vec::with_capacity(count * 4);
        let mut col_parent = Vec::with_capacity(count * 4);
        let mut col_lod = Vec::with_capacity(count * 2);
        let mut col_lod_dist = Vec::with_capacity(count * 4);
        let mut col_children = Vec::with_capacity(count * 4);
        let mut col_map = Vec::with_capacity(count * 4);

        for &id in &gathered.ids {
            let i = id as usize;
            let name = *name_local.entry(store.name_of[i]).or_insert_with(|| {
                names.push(store.names[store.name_of[i] as usize].as_str());
                name_hashes.push(store.name_hashes[store.name_of[i] as usize]);
                name_classes.push(store.name_classes[store.name_of[i] as usize]);
                (names.len() - 1) as u32
            });
            let lod = *lod_local.entry(store.lod_of[i]).or_insert_with(|| {
                lod_levels.push(store.lod_levels[store.lod_of[i] as usize].as_str());
                (lod_levels.len() - 1) as u16
            });
            let map = *map_local.entry(store.map[i]).or_insert_with(|| {
                let info = &self.maps[store.map[i] as usize];
                map_refs.push(MapRef {
                    path: &info.path,
                    name: &info.name,
                });
                (map_refs.len() - 1) as u32
            });

            col_index.extend_from_slice(&store.index[i].to_le_bytes());
            col_name.extend_from_slice(&name.to_le_bytes());
            col_flags.push(store.flags[i]);
            for value in store.position[i] {
                col_position.extend_from_slice(&value.to_le_bytes());
            }
            for value in store.rotation[i] {
                col_rotation.extend_from_slice(&value.to_le_bytes());
            }
            col_scale_xy.extend_from_slice(&store.scale_xy[i].to_le_bytes());
            col_scale_z.extend_from_slice(&store.scale_z[i].to_le_bytes());
            col_parent.extend_from_slice(&store.parent[i].to_le_bytes());
            col_lod.extend_from_slice(&lod.to_le_bytes());
            col_lod_dist.extend_from_slice(&store.lod_dist[i].to_le_bytes());
            col_children.extend_from_slice(&store.children[i].to_le_bytes());
            col_map.extend_from_slice(&map.to_le_bytes());
        }

        let mut envelope = Envelope::new();
        // Push order is the contract: the placement columns, then `mapSlot`.
        for column in [
            col_index,
            col_name,
            col_flags,
            col_position,
            col_rotation,
            col_scale_xy,
            col_scale_z,
            col_parent,
            col_lod,
            col_lod_dist,
            col_children,
            col_map,
        ] {
            envelope.push(column);
        }
        let model_refs: Vec<ModelRef> = models
            .into_iter()
            .map(|(path, bytes)| ModelRef {
                section: envelope.push(bytes),
                path,
            })
            .collect();

        let meta = BatchMeta {
            cells: &gathered.spans,
            count,
            maps: map_refs,
            names,
            name_hashes,
            name_classes,
            lod_levels,
            assets,
            models: model_refs,
        };
        let meta = serde_json::to_string(&meta)
            .map_err(|e| format!("could not write the cell metadata: {e}"))?;
        Ok(envelope.finish(Kind::CellData, &meta))
    }
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct MapRef<'a> {
    path: &'a str,
    name: &'a str,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct ModelRef {
    path: String,
    /// The section holding the model's own `Kind::Model` envelope.
    section: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct BatchMeta<'a> {
    cells: &'a [CellSpan],
    count: usize,
    maps: Vec<MapRef<'a>>,
    names: Vec<&'a str>,
    name_hashes: Vec<u32>,
    /// Parallel to `names`: an archetype class, as `nameClasses` in a map envelope.
    name_classes: Vec<u8>,
    lod_levels: Vec<&'a str>,
    assets: Vec<Asset>,
    models: Vec<ModelRef>,
}

/// Resolve the names the frontend does not hold to model files.
///
/// Distinct paths are listed once, in first-seen order, which is the order the
/// models are compiled and sent in.
pub fn resolve_assets(
    names: &[&str],
    known: &HashSet<String>,
    find: impl Fn(&str) -> Option<String>,
) -> (Vec<Asset>, Vec<String>) {
    let mut assets = Vec::new();
    let mut paths: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for &name in names {
        if known.contains(name) {
            continue;
        }
        let path = find(name);
        if let Some(path) = &path {
            if seen.insert(path.clone()) {
                paths.push(path.clone());
            }
        }
        assets.push(Asset {
            name: name.to_string(),
            path,
        });
    }
    (assets, paths)
}

/// The built index, shared by the commands. `None` until the first build.
#[derive(Default)]
pub struct WorldIndexState(pub RwLock<Option<Arc<WorldIndex>>>);

impl WorldIndexState {
    fn current(&self, folder: &str) -> Result<Arc<WorldIndex>, String> {
        let index = self
            .0
            .read()
            .map_err(|_| "the world index is poisoned")?
            .clone()
            .ok_or("the world index has not been built")?;
        if index.folder != folder {
            return Err("the world index was built for another game folder".into());
        }
        Ok(index)
    }
}

/// Build the index over a content group and return its partition.
///
/// `fine` and `threshold` default to the numbers chosen from the base game's
/// real density (see `docs/hex-streaming.md`).
#[tauri::command]
pub async fn world_index_build(
    group: String,
    fine: Option<f64>,
    threshold: Option<usize>,
    game: State<'_, GameState>,
    state: State<'_, WorldIndexState>,
) -> Result<tauri::ipc::Response, String> {
    let loaded = game.loaded()?;
    let mut params = PartitionParams {
        sizes: crate::cells::sizes_for(fine.unwrap_or(DEFAULT_FINE), DEFAULT_LEVELS),
        ..PartitionParams::default()
    };
    if let Some(threshold) = threshold {
        params.threshold = threshold;
    }
    let index = blocking(move || {
        crate::perf::span("world_index.build", || {
            let listed = ymaps_in_group(&loaded.manager, &group);
            // Every map comes out of the same cache `world_map` fills, so a
            // second launch is hits all the way down.
            let read: Vec<Result<DecodedMap, String>> = listed
                .par_iter()
                .map(|(path, _)| {
                    map_envelope(&loaded, path).and_then(|bytes| crate::placements::decode(&bytes))
                })
                .collect();
            let failed = read.iter().filter(|r| r.is_err()).count();
            let decoded: Vec<DecodedMap> = read.into_iter().flatten().collect();
            WorldIndex::build(&loaded.folder, &group, &decoded, failed, params)
        })
    })
    .await?;
    let reply = index.partition_envelope();
    *state.0.write().map_err(|_| "the world index is poisoned")? = Some(Arc::new(index));
    Ok(tauri::ipc::Response::new(reply))
}

/// Most model bytes one `world_index_cells` reply carries. A reply is one IPC
/// payload held whole in the backend and again in the WebView, so it is bounded
/// by what a reply can safely cost, not by what the cells need.
pub const REPLY_MODEL_BYTES: usize = 48 * 1024 * 1024;

/// Stack for the model workers: compiling a drawable's textures recurses deeper
/// than the 2 MB a default worker gets.
const MODEL_WORKER_STACK: usize = 16 * 1024 * 1024;

/// A pool of its own, one core short of the machine's, so a cell fetch cannot
/// starve the UI thread or the bake, and its workers have room to compile.
fn model_pool() -> Option<&'static rayon::ThreadPool> {
    static POOL: std::sync::OnceLock<Option<rayon::ThreadPool>> = std::sync::OnceLock::new();
    POOL.get_or_init(|| {
        let threads = std::thread::available_parallelism()
            .map(|n| (n.get() - 1).max(1))
            .unwrap_or(1);
        rayon::ThreadPoolBuilder::new()
            .num_threads(threads)
            .stack_size(MODEL_WORKER_STACK)
            .thread_name(|i| format!("cell-model-{i}"))
            .build()
            .ok()
    })
    .as_ref()
}

/// Compile `paths` in order, a worker-count of them at a time, and stop once the
/// results hold `cap` bytes. The last wave may overshoot by up to a wave's worth;
/// a model that fails is skipped. Order is the order of `paths`.
pub fn compile_capped(
    paths: &[String],
    cap: usize,
    compile: impl Fn(&str) -> Option<Vec<u8>> + Sync,
) -> Vec<(String, Vec<u8>)> {
    let wave = model_pool().map_or(1, |p| p.current_num_threads()).max(1);
    let mut out: Vec<(String, Vec<u8>)> = Vec::new();
    let mut bytes = 0usize;
    for chunk in paths.chunks(wave) {
        if bytes >= cap {
            break;
        }
        let run = || -> Vec<Option<Vec<u8>>> { chunk.par_iter().map(|p| compile(p)).collect() };
        let built = match model_pool() {
            Some(pool) => pool.install(run),
            None => chunk.iter().map(|p| compile(p)).collect(),
        };
        for (path, built) in chunk.iter().zip(built) {
            if let Some(model) = built {
                bytes += model.len();
                out.push((path.clone(), model));
            }
        }
    }
    out
}

/// Placements and models for a set of cells, in one reply.
///
/// `known` is every archetype name the frontend already holds a model (or a
/// known absence) for: they are left out of the reply, which is what keeps the
/// second cell of a street from paying for the lamp-post the first one sent.
#[tauri::command]
pub async fn world_index_cells(
    cells: Vec<CellRef>,
    known: Vec<String>,
    game: State<'_, GameState>,
    gpu: State<'_, GpuState>,
    state: State<'_, WorldIndexState>,
) -> Result<tauri::ipc::Response, String> {
    let loaded = game.loaded()?;
    let index = state.current(&loaded.folder)?;
    let caps = gpu.get();
    blocking(move || {
        crate::perf::span("world_index.cells", || {
            let gathered = index.gather(&cells);
            let known: HashSet<String> = known.into_iter().collect();
            let names = index.names_in(&gathered);
            let (assets, paths) =
                resolve_assets(&names, &known, |name| find_model_for(&loaded.manager, name));
            // A cold cell can name hundreds of models and gigabytes of
            // geometry (a measured terrain cell is 1.8 GB). Only the first
            // `REPLY_MODEL_BYTES` ride along; the rest are left for the
            // frontend's per-model loader, which paces itself against the model
            // cache budget.
            let models = compile_capped(&paths, REPLY_MODEL_BYTES, |path| {
                crate::world::model_envelope(&loaded, caps, path).ok()
            });
            index.encode_batch(&gathered, assets, models)
        })
    })
    .await
    .map(tauri::ipc::Response::new)
}

/// What a cluster's models really weigh once compiled.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ClusterMeasure {
    pub key: String,
    /// Distinct archetype names in the cluster.
    pub models: usize,
    /// Distinct model files those names resolve to (several names can share one).
    pub files: usize,
    /// Names no model file was found for.
    pub unresolved: usize,
    /// Files that did not compile.
    pub failed: usize,
    /// Compiled bytes of every file that did, each counted once.
    pub bytes: u64,
    /// The largest single compiled file.
    pub largest: u64,
    pub millis: u64,
}

/// Size every path with `measure`, on the model pool, keeping only the lengths so
/// the models themselves are dropped as they are counted. Returns
/// `(total bytes, largest, failed)`.
pub fn measure_models(
    paths: &[String],
    measure: impl Fn(&str) -> Option<usize> + Sync,
) -> (u64, u64, usize) {
    let run = || -> Vec<Option<usize>> { paths.par_iter().map(|p| measure(p)).collect() };
    let sizes = match model_pool() {
        Some(pool) => pool.install(run),
        None => paths.iter().map(|p| measure(p)).collect(),
    };
    let mut bytes = 0u64;
    let mut largest = 0u64;
    let mut failed = 0usize;
    for size in sizes {
        match size {
            Some(n) => {
                bytes += n as u64;
                largest = largest.max(n as u64);
            }
            None => failed += 1,
        }
    }
    (bytes, largest, failed)
}

impl WorldIndex {
    /// The cells of a cluster, in leaf order, as the frontend would name them.
    pub fn cluster_cells(&self, cluster: &Cluster) -> Vec<CellRef> {
        cluster
            .leaves
            .iter()
            .map(|&i| {
                let leaf = &self.partition.leaves[i as usize];
                CellRef {
                    level: leaf.level,
                    q: leaf.q,
                    r: leaf.r,
                }
            })
            .collect()
    }

    /// Measure a cluster: resolve its models to files and size each compiled one.
    pub fn measure_cluster(
        &self,
        cluster: &Cluster,
        find: impl Fn(&str) -> Option<String>,
        measure: impl Fn(&str) -> Option<usize> + Sync,
    ) -> ClusterMeasure {
        let started = std::time::Instant::now();
        let gathered = self.gather(&self.cluster_cells(cluster));
        let names = self.names_in(&gathered);
        let (assets, paths) = resolve_assets(&names, &HashSet::new(), find);
        let (bytes, largest, failed) = measure_models(&paths, measure);
        ClusterMeasure {
            key: cluster.key.clone(),
            models: names.len(),
            files: paths.len(),
            unresolved: assets.iter().filter(|a| a.path.is_none()).count(),
            failed,
            bytes,
            largest,
            millis: started.elapsed().as_millis() as u64,
        }
    }
}

/// Compile (or read from the cache) every model of a zone cluster and report
/// their total size, without keeping any of them.
///
/// On demand and slow on a cold cache: a city-sized zone is gigabytes of
/// geometry. Models land in the persistent cache as a side effect, so a second
/// measure, or loading the zone afterwards, reads them back.
#[tauri::command]
pub async fn world_index_cluster_measure(
    key: String,
    game: State<'_, GameState>,
    gpu: State<'_, GpuState>,
    state: State<'_, WorldIndexState>,
) -> Result<ClusterMeasure, String> {
    let loaded = game.loaded()?;
    let index = state.current(&loaded.folder)?;
    let caps = gpu.get();
    blocking(move || {
        let cluster = index
            .clusters
            .find(&key)
            .ok_or_else(|| format!("no zone cluster '{key}'"))?;
        Ok(crate::perf::span("world_index.cluster_measure", || {
            index.measure_cluster(
                cluster,
                |name| find_model_for(&loaded.manager, name),
                |path| {
                    crate::world::model_envelope(&loaded, caps, path)
                        .ok()
                        .map(|b| b.len())
                },
            )
        }))
    })
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use cw_core::wire::EnvelopeView;

    fn decoded(path: &str, rows: &[(&str, &str, f32, f32)]) -> DecodedMap {
        // `rows`: name, lod label, x, y.
        let mut map = DecodedMap {
            path: path.to_string(),
            name: path.trim_end_matches(".ymap").to_string(),
            entity_count: rows.len(),
            extents_min: [0.0; 3],
            extents_max: [1.0; 3],
            ..DecodedMap::default()
        };
        for (i, (name, lod, x, y)) in rows.iter().enumerate() {
            let name_slot = map.names.iter().position(|n| n == name).unwrap_or_else(|| {
                map.names.push(name.to_string());
                map.name_hashes.push(name.len() as u32);
                map.names.len() - 1
            });
            let lod_slot = map
                .lod_levels
                .iter()
                .position(|l| l == lod)
                .unwrap_or_else(|| {
                    map.lod_levels.push(lod.to_string());
                    map.lod_levels.len() - 1
                });
            map.index.push(i as u32 * 10);
            map.name_slot.push(name_slot as u32);
            map.flags.push(0);
            map.position.push([*x, *y, 5.0]);
            map.rotation.push([0.0, 0.0, 0.0, 1.0]);
            map.scale_xy.push(1.0);
            map.scale_z.push(1.0);
            map.parent.push(-1);
            map.lod_slot.push(lod_slot as u16);
            map.lod_dist.push(100.0);
            map.children.push(0);
        }
        map
    }

    fn small_params() -> PartitionParams {
        PartitionParams {
            sizes: vec![1000.0, 250.0],
            threshold: 3,
        }
    }

    fn world() -> WorldIndex {
        let a = decoded(
            "a.ymap",
            &[
                ("prop_lamp", "HD", 10.0, 10.0),
                ("prop_lamp", "HD", 20.0, 20.0),
                ("prop_bin", "HD", 30.0, 30.0),
                ("terrain_01", "HD", 40.0, 40.0),
                ("prop_far", "LOD", 5000.0, 5000.0),
            ],
        );
        let b = decoded(
            "b.ymap",
            &[
                ("prop_lamp", "HD", 15.0, 15.0),
                ("prop_tree", "LOD", 25.0, 25.0),
            ],
        );
        WorldIndex::build("C:\\gta", "base", &[a, b], 0, small_params()).expect("build")
    }

    #[test]
    fn the_ground_is_set_aside_and_everything_else_is_partitioned() {
        let index = world();
        assert_eq!(index.report.placements, 7);
        assert_eq!(index.report.terrain, 1);
        let partitioned: u32 = index.partition.leaves.iter().map(|l| l.count).sum();
        assert_eq!(partitioned, 6);
        assert_eq!(
            index.store.names[index.store.name_of[index.terrain[0] as usize] as usize],
            "terrain_01"
        );
    }

    #[test]
    fn names_and_lod_labels_are_shared_across_maps() {
        let index = world();
        let lamps = index
            .store
            .names
            .iter()
            .filter(|n| *n == "prop_lamp")
            .count();
        assert_eq!(lamps, 1, "one row for a name two maps both place");
        assert_eq!(index.store.lod_levels, vec!["HD", "LOD"]);
    }

    #[test]
    fn terrain_is_told_by_whole_word_tokens() {
        for name in [
            "terrain_01",
            "Ter_Road",
            "grnd_a",
            "prop_terr_x",
            "x_TERRAIN",
        ] {
            assert!(is_terrain_name(name), "{name}");
        }
        // `terrace` and `interior` contain `ter` but are not the word.
        for name in ["prop_terrace", "interior_a", "prop_lamp", "hash_1234abcd"] {
            assert!(!is_terrain_name(name), "{name}");
        }
    }

    #[test]
    fn a_leafs_cell_returns_its_rows_and_the_ground_is_its_own_cell() {
        let index = world();
        let leaf = index
            .partition
            .leaves
            .iter()
            .max_by_key(|l| l.count)
            .expect("a leaf")
            .clone();
        let gathered = index.gather(&[
            CellRef {
                level: leaf.level,
                q: leaf.q,
                r: leaf.r,
            },
            CellRef {
                level: TERRAIN_LEVEL,
                q: 0,
                r: 0,
            },
            CellRef {
                level: 0,
                q: 999,
                r: 999,
            },
        ]);
        assert_eq!(gathered.spans.len(), 3);
        assert_eq!(gathered.spans[0].count, leaf.count as usize);
        assert_eq!(gathered.spans[1].start, leaf.count as usize);
        assert_eq!(gathered.spans[1].count, 1);
        assert_eq!(
            gathered.spans[2].count, 0,
            "an unknown cell is empty, not an error"
        );
        assert_eq!(gathered.ids.len(), leaf.count as usize + 1);
    }

    #[test]
    fn the_partition_envelope_has_one_row_per_leaf() {
        let index = world();
        let bytes = index.partition_envelope();
        let view = EnvelopeView::read(&bytes).expect("read");
        assert_eq!(view.kind, Kind::Cells);
        let leaves = index.partition.leaves.len();
        assert_eq!(view.section(0).map(<[u8]>::len), Some(leaves));
        for section in 1..=4 {
            assert_eq!(view.section(section).map(<[u8]>::len), Some(leaves * 4));
        }
        let meta: serde_json::Value = serde_json::from_str(view.meta).expect("json");
        assert_eq!(meta["leaves"], leaves);
        assert_eq!(meta["terrain"], 1);
        assert_eq!(meta["placements"], 7);
        assert_eq!(meta["sizes"], serde_json::json!([1000.0, 250.0]));
        let counts: Vec<u32> = view
            .section(3)
            .expect("counts")
            .chunks_exact(4)
            .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
            .collect();
        assert_eq!(counts.iter().sum::<u32>(), 6);
    }

    #[test]
    fn the_partition_envelope_carries_each_leafs_cluster() {
        let index = world();
        let bytes = index.partition_envelope();
        let view = EnvelopeView::read(&bytes).expect("read");
        let leaves = index.partition.leaves.len();
        let section = view.section(8).expect("cluster column");
        assert_eq!(section.len(), leaves * 2);
        let meta: serde_json::Value = serde_json::from_str(view.meta).expect("json");
        let clusters = meta["clusters"].as_array().expect("clusters");
        assert_eq!(clusters.last().expect("last")["key"], "countryside");
        let total: u64 = clusters
            .iter()
            .map(|c| c["placements"].as_u64().expect("placements"))
            .sum();
        assert_eq!(total, 6, "every partitioned placement is in a cluster");
        for chunk in section.chunks_exact(2) {
            let at = u16::from_le_bytes([chunk[0], chunk[1]]) as usize;
            assert!(at < clusters.len());
        }
    }

    #[test]
    fn a_clusters_models_are_measured_once_each_and_failures_counted() {
        let index = world();
        let all = index
            .clusters
            .list
            .iter()
            .max_by_key(|c| c.placements)
            .expect("a cluster")
            .clone();
        let measured = index.measure_cluster(
            &all,
            |name| (name != "prop_far").then(|| format!("models\\{name}.ydr")),
            |path| (!path.contains("prop_bin")).then_some(100),
        );
        assert_eq!(measured.key, all.key);
        assert_eq!(measured.models, all.models);
        // `prop_far` has no file; the others each have one.
        assert_eq!(measured.files + measured.unresolved, measured.models);
        assert_eq!(
            measured.bytes,
            100 * (measured.files - measured.failed) as u64
        );
        assert!(measured.largest <= 100);
    }

    #[test]
    fn measuring_sizes_every_path_and_counts_failures() {
        let paths: Vec<String> = ["a", "bb", "fail", "dddd"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        let (bytes, largest, failed) = measure_models(&paths, |p| (p != "fail").then_some(p.len()));
        assert_eq!((bytes, largest, failed), (7, 4, 1));
    }

    fn batch(index: &WorldIndex, cells: &[CellRef], known: &[&str]) -> Vec<u8> {
        let gathered = index.gather(cells);
        let names = index.names_in(&gathered);
        let known: HashSet<String> = known.iter().map(|s| s.to_string()).collect();
        let (assets, paths) = resolve_assets(&names, &known, |name| {
            (name != "prop_far").then(|| format!("models\\{name}.ydr"))
        });
        // A stand-in for a compiled model: a real, tiny, model envelope.
        let models = paths
            .into_iter()
            .map(|path| {
                let mut inner = Envelope::new();
                inner.push(vec![1u8, 2, 3, 4]);
                let bytes = inner.finish(Kind::Model, &format!("{{\"path\":\"{path}\"}}"));
                (path, bytes)
            })
            .collect();
        index
            .encode_batch(&gathered, assets, models)
            .expect("encode")
    }

    fn everything(index: &WorldIndex) -> Vec<CellRef> {
        let mut cells: Vec<CellRef> = index
            .partition
            .leaves
            .iter()
            .map(|l| CellRef {
                level: l.level,
                q: l.q,
                r: l.r,
            })
            .collect();
        cells.push(CellRef {
            level: TERRAIN_LEVEL,
            q: 0,
            r: 0,
        });
        cells
    }

    #[test]
    fn a_batch_holds_the_placement_columns_then_the_models() {
        let index = world();
        let bytes = batch(&index, &everything(&index), &[]);
        let view = EnvelopeView::read(&bytes).expect("read");
        assert_eq!(view.kind, Kind::CellData);
        let meta: serde_json::Value = serde_json::from_str(view.meta).expect("json");
        assert_eq!(meta["count"], 7);

        let widths = [4, 4, 1, 12, 16, 4, 4, 4, 2, 4, 4, 4];
        for (section, width) in widths.iter().enumerate() {
            assert_eq!(
                view.section(section).map(<[u8]>::len),
                Some(7 * width),
                "section {section}"
            );
        }

        // Every model rides as its own envelope, named by section.
        let models = meta["models"].as_array().expect("models");
        assert!(!models.is_empty());
        for model in models {
            let section = model["section"].as_u64().expect("section") as usize;
            assert!(section >= 12);
            let inner = EnvelopeView::read(view.section(section).expect("model bytes"))
                .expect("a nested envelope reads");
            assert_eq!(inner.kind, Kind::Model);
        }
    }

    #[test]
    fn rows_point_at_the_right_maps_names_and_entities() {
        let index = world();
        let bytes = batch(&index, &everything(&index), &[]);
        let view = EnvelopeView::read(&bytes).expect("read");
        let meta: serde_json::Value = serde_json::from_str(view.meta).expect("json");

        let u32s = |section: usize| -> Vec<u32> {
            view.section(section)
                .expect("section")
                .chunks_exact(4)
                .map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]]))
                .collect()
        };
        let entity = u32s(0);
        let name = u32s(1);
        let map = u32s(11);
        let names = meta["names"].as_array().expect("names");
        let maps = meta["maps"].as_array().expect("maps");

        // Every row's (map, entity index, name) is one of the inputs.
        let mut seen = Vec::new();
        for row in 0..7 {
            seen.push((
                maps[map[row] as usize]["path"]
                    .as_str()
                    .expect("path")
                    .to_string(),
                entity[row],
                names[name[row] as usize]
                    .as_str()
                    .expect("name")
                    .to_string(),
            ));
        }
        seen.sort();
        let mut want = vec![
            ("a.ymap", 0, "prop_lamp"),
            ("a.ymap", 10, "prop_lamp"),
            ("a.ymap", 20, "prop_bin"),
            ("a.ymap", 30, "terrain_01"),
            ("a.ymap", 40, "prop_far"),
            ("b.ymap", 0, "prop_lamp"),
            ("b.ymap", 10, "prop_tree"),
        ]
        .into_iter()
        .map(|(p, e, n)| (p.to_string(), e, n.to_string()))
        .collect::<Vec<_>>();
        want.sort();
        assert_eq!(seen, want);
    }

    #[test]
    fn a_batch_names_each_model_once_and_skips_what_the_frontend_holds() {
        let index = world();
        let all = everything(&index);

        let bytes = batch(&index, &all, &[]);
        let view = EnvelopeView::read(&bytes).expect("read");
        let meta: serde_json::Value = serde_json::from_str(view.meta).expect("json");
        let assets = meta["assets"].as_array().expect("assets");
        // Five distinct names, the lamp placed three times counted once.
        assert_eq!(assets.len(), 5);
        let far = assets
            .iter()
            .find(|a| a["name"] == "prop_far")
            .expect("far");
        assert!(far["path"].is_null(), "an unresolved name says so");
        assert_eq!(meta["models"].as_array().expect("models").len(), 4);

        let bytes = batch(&index, &all, &["prop_lamp", "prop_bin"]);
        let view = EnvelopeView::read(&bytes).expect("read");
        let meta: serde_json::Value = serde_json::from_str(view.meta).expect("json");
        let names: Vec<&str> = meta["assets"]
            .as_array()
            .expect("assets")
            .iter()
            .map(|a| a["name"].as_str().expect("name"))
            .collect();
        assert!(!names.contains(&"prop_lamp") && !names.contains(&"prop_bin"));
        assert_eq!(meta["models"].as_array().expect("models").len(), 2);
        // The placements still arrive: known names only skip the model.
        assert_eq!(meta["count"], 7);
    }

    #[test]
    fn two_names_sharing_a_file_send_it_once() {
        let known = HashSet::new();
        let (assets, paths) = resolve_assets(&["a", "b", "c"], &known, |name| {
            Some(if name == "c" { "other.ydr" } else { "same.ydr" }.to_string())
        });
        assert_eq!(assets.len(), 3);
        assert_eq!(paths, vec!["same.ydr", "other.ydr"]);
    }

    #[test]
    fn an_empty_gather_still_encodes() {
        let index = world();
        let bytes = batch(&index, &[], &[]);
        let view = EnvelopeView::read(&bytes).expect("read");
        let meta: serde_json::Value = serde_json::from_str(view.meta).expect("json");
        assert_eq!(meta["count"], 0);
        assert_eq!(view.section(0).map(<[u8]>::len), Some(0));
    }

    #[test]
    fn the_state_refuses_an_index_from_another_folder() {
        let state = WorldIndexState::default();
        assert!(state.current("C:\\gta").is_err(), "nothing built yet");
        *state.0.write().expect("lock") = Some(Arc::new(world()));
        assert!(state.current("C:\\gta").is_ok());
        assert!(state.current("D:\\other").is_err());
    }

    #[test]
    fn a_reply_stops_taking_models_once_it_is_full() {
        let paths: Vec<String> = (0..200).map(|i| format!("m{i}.ydr")).collect();
        let sent = compile_capped(&paths, 1000, |_| Some(vec![0u8; 100]));
        let bytes: usize = sent.iter().map(|(_, b)| b.len()).sum();
        assert!(bytes >= 1000, "filled up to the cap");
        assert!(sent.len() < paths.len(), "and then stopped");
        // At most one wave past the cap.
        assert!(bytes <= 1000 + 100 * 64);
        // What was sent is a prefix, in order.
        for (i, (path, _)) in sent.iter().enumerate() {
            assert_eq!(path, &paths[i]);
        }
    }

    #[test]
    fn a_model_that_fails_is_skipped_and_costs_no_bytes() {
        let paths: Vec<String> = (0..6).map(|i| format!("m{i}.ydr")).collect();
        let sent = compile_capped(&paths, usize::MAX, |p| {
            (p != "m2.ydr").then(|| vec![1u8; 10])
        });
        assert_eq!(
            sent.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
            ["m0.ydr", "m1.ydr", "m3.ydr", "m4.ydr", "m5.ydr"]
        );
        assert!(compile_capped(&[], 10, |_| Some(vec![])).is_empty());
    }

    #[test]
    fn a_name_keeps_its_archetype_class_through_the_store_and_the_batch() {
        let mut a = decoded(
            "a.ymap",
            &[("prop_bush", "HD", 0.0, 0.0), ("prop_wall", "HD", 1.0, 1.0)],
        );
        a.name_classes = vec![2, 0];
        // Another map that could not index the bush says none: it must not erase it.
        let mut b = decoded("b.ymap", &[("prop_bush", "HD", 2.0, 2.0)]);
        b.name_classes = vec![0];
        let mut store = PlacementStore::default();
        store.append(0, &a).expect("a");
        store.append(1, &b).expect("b");
        let bush = store
            .names
            .iter()
            .position(|n| n == "prop_bush")
            .expect("bush");
        let wall = store
            .names
            .iter()
            .position(|n| n == "prop_wall")
            .expect("wall");
        assert_eq!(store.name_classes[bush], 2);
        assert_eq!(store.name_classes[wall], 0);
    }
}
