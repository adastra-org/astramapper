//! Hexagonal cells over the ground plane, and the adaptive partition of the
//! world into them.
//!
//! This is the Rust half of `src/lib/world/hex.ts` and `hexAdaptive.ts`, and it
//! has to agree with them to the cell: the Hex view draws what the frontend
//! computes, the backend index serves what this computes, and a placement that
//! lands in a different leaf on each side would be a cell that is asked for and
//! comes back empty. The tests below pin the two against the same fixtures.
//!
//! # The maths
//!
//! Axial `(q, r)`, pointy-top, `size` the distance from a cell's centre to a
//! corner in metres. Distances are in the X/Y plane; height never picks a cell.
//! JavaScript's `Math.round` rounds halves towards `+inf` and Rust's `round`
//! rounds them away from zero, so [`js_round`] is used where the two could
//! disagree on a point exactly between cells.
//!
//! # The partition
//!
//! Several sizes at once, coarse to fine. Every placement starts in the coarsest
//! grid; a cell holding more than `threshold` of them is *split*, and its
//! placements move on to the next finer grid, where they are counted again. The
//! finest level never splits. Hexagons do not nest, so a fine cell near a split
//! coarse one can overlap its edge; each placement still lands in exactly one
//! leaf, which is what matters (H3 makes the same trade).
//!
//! Levels are processed over *all* the placements still active, not parent by
//! parent, so two crowded neighbours that share a fine cell count into it once.
//! Leaves come out in first-seen order within a level, as the TypeScript's `Map`
//! iteration does, so the two partitions are the same list and not just the same
//! set.

use std::collections::HashMap;

const SQRT3: f64 = 1.732_050_807_568_877_2;

/// A cell at one level of the partition: a level index and an axial position.
pub type CellId = (u8, i32, i32);

/// `Math.round`: halves go towards positive infinity.
fn js_round(value: f64) -> f64 {
    (value + 0.5).floor()
}

/// The cell holding a ground-plane point, at one cell size.
pub fn world_to_hex(x: f64, y: f64, size: f64) -> (i32, i32) {
    let q = ((SQRT3 / 3.0) * x - y / 3.0) / size;
    let r = ((2.0 / 3.0) * y) / size;
    round_hex(q, r)
}

/// Cube-coordinate rounding: round all three, then repair the worst offender.
fn round_hex(q: f64, r: f64) -> (i32, i32) {
    let s = -q - r;
    let mut rq = js_round(q);
    let mut rr = js_round(r);
    let rs = js_round(s);
    let dq = (rq - q).abs();
    let dr = (rr - r).abs();
    let ds = (rs - s).abs();
    if dq > dr && dq > ds {
        rq = -rr - rs;
    } else if dr > ds {
        rr = -rq - rs;
    }
    (rq as i32, rr as i32)
}

/// A cell's centre in metres.
pub fn hex_center(q: i32, r: i32, size: f64) -> (f64, f64) {
    (
        size * SQRT3 * (f64::from(q) + f64::from(r) / 2.0),
        size * 1.5 * f64::from(r),
    )
}

/// One number for a cell at a size, matching `hexKey` in `hex.ts`.
pub fn hex_key(q: i32, r: i32) -> u64 {
    ((i64::from(q) + 32768) * 65536 + (i64::from(r) + 32768)) as u64
}

/// One end of the partition: a cell that holds placements.
#[derive(Debug, Clone, PartialEq)]
pub struct Leaf {
    pub level: u8,
    pub q: i32,
    pub r: i32,
    /// Centre in metres.
    pub x: f64,
    pub y: f64,
    /// Mean height of the placements in it.
    pub z: f32,
    /// Where its placements start in [`Partition::members`].
    pub start: u32,
    pub count: u32,
}

/// What a partition is built with.
#[derive(Debug, Clone, PartialEq)]
pub struct PartitionParams {
    /// Centre-to-corner sizes, coarse to fine.
    pub sizes: Vec<f64>,
    /// Placements a cell may hold before it is split into the next finer grid.
    pub threshold: usize,
}

/// How many sizes the partition runs through by default, coarse to fine.
pub const DEFAULT_LEVELS: usize = 5;
/// The finest cell size, in metres.
pub const DEFAULT_FINE: f64 = 100.0;
/// Placements a cell holds before it splits. Chosen from the base game's real
/// density (`docs/hex-streaming.md`): 1,000 gives ~1,900 leaves with a median of
/// 331 placements and the city's reach unchanged from 500, where 2,000 doubles
/// the median leaf (and so a cell reply's models) for no gain in reach.
pub const DEFAULT_THRESHOLD: usize = 1000;
/// Placements the working set may hold: a 0.7 km radius in Legion Square and
/// 1.1-1.7 km in the open country, over the base game.
pub const DEFAULT_BUDGET: usize = 50_000;

/// The sizes for a finest cell: each coarser level double the one below.
pub fn sizes_for(fine: f64, levels: usize) -> Vec<f64> {
    (0..levels)
        .map(|i| fine * f64::from(1u32 << (levels - 1 - i)))
        .collect()
}

impl Default for PartitionParams {
    fn default() -> Self {
        Self {
            sizes: sizes_for(DEFAULT_FINE, DEFAULT_LEVELS),
            threshold: DEFAULT_THRESHOLD,
        }
    }
}

/// The world cut into leaves. Each member placement is in exactly one.
#[derive(Debug, Clone)]
pub struct Partition {
    pub params: PartitionParams,
    pub leaves: Vec<Leaf>,
    /// Cells that were split finer, so a reader can tell "split" from "empty".
    pub splits: Vec<CellId>,
    /// Placement ids, grouped by leaf; a leaf's are `start..start + count`.
    pub members: Vec<u32>,
    lookup: HashMap<CellId, u32>,
}

impl Partition {
    /// Partition the `ids` of a placement store by position.
    ///
    /// `xs`, `ys` and `zs` are indexed by placement id; `ids` is which ones take
    /// part (the store holds terrain too, which is a layer of its own).
    pub fn build(xs: &[f32], ys: &[f32], zs: &[f32], ids: &[u32], params: PartitionParams) -> Self {
        let mut leaves: Vec<Leaf> = Vec::new();
        let mut splits: Vec<CellId> = Vec::new();
        let mut members: Vec<u32> = Vec::with_capacity(ids.len());
        let mut active: Vec<u32> = ids.to_vec();

        let levels = params.sizes.len();
        for (level, &size) in params.sizes.iter().enumerate() {
            if active.is_empty() {
                break;
            }
            let last = level + 1 == levels;

            // First-seen order, like a JS `Map`: `slot` finds a key's group.
            let mut slot: HashMap<u64, usize> = HashMap::new();
            let mut groups: Vec<(i32, i32, Vec<u32>)> = Vec::new();
            for &id in &active {
                let (q, r) =
                    world_to_hex(f64::from(xs[id as usize]), f64::from(ys[id as usize]), size);
                let at = *slot.entry(hex_key(q, r)).or_insert_with(|| {
                    groups.push((q, r, Vec::new()));
                    groups.len() - 1
                });
                groups[at].2.push(id);
            }

            let mut next: Vec<u32> = Vec::new();
            for (q, r, group) in groups {
                if !last && group.len() > params.threshold {
                    splits.push((level as u8, q, r));
                    next.extend_from_slice(&group);
                    continue;
                }
                // Ascending ids, so a leaf's rows come out map by map whichever
                // order the splits shuffled them into.
                let mut group = group;
                group.sort_unstable();
                let (x, y) = hex_center(q, r, size);
                let z_sum: f64 = group.iter().map(|&id| f64::from(zs[id as usize])).sum();
                leaves.push(Leaf {
                    level: level as u8,
                    q,
                    r,
                    x,
                    y,
                    z: (z_sum / group.len() as f64) as f32,
                    start: members.len() as u32,
                    count: group.len() as u32,
                });
                members.extend_from_slice(&group);
            }
            active = next;
        }

        let lookup = leaves
            .iter()
            .enumerate()
            .map(|(i, leaf)| ((leaf.level, leaf.q, leaf.r), i as u32))
            .collect();
        Self {
            params,
            leaves,
            splits,
            members,
            lookup,
        }
    }

    /// The leaf at a cell, if one exists there.
    pub fn leaf(&self, cell: CellId) -> Option<&Leaf> {
        self.lookup.get(&cell).map(|&i| &self.leaves[i as usize])
    }

    /// A leaf's placement ids.
    pub fn members_of(&self, leaf: &Leaf) -> &[u32] {
        &self.members[leaf.start as usize..(leaf.start + leaf.count) as usize]
    }

    /// The leaves a loader would hold for a camera, nearest first, until the
    /// next one would pass `budget` placements.
    ///
    /// The cell under the camera is always taken, however crowded: a working set
    /// with nothing in it is no use. Past that the budget is a hard stop. The
    /// same rule as `adaptiveWorkingSet` in `hexAdaptive.ts`.
    pub fn working_set(&self, cam_x: f64, cam_y: f64, budget: usize) -> WorkingSet {
        let mut by_distance: Vec<(u32, f64)> = self
            .leaves
            .iter()
            .enumerate()
            .map(|(i, leaf)| (i as u32, (leaf.x - cam_x).hypot(leaf.y - cam_y)))
            .collect();
        // Stable, so ties keep leaf order as the TypeScript sort does.
        by_distance.sort_by(|a, b| a.1.total_cmp(&b.1));

        let mut taken = Vec::new();
        let mut placements = 0usize;
        let mut reach = 0.0;
        for (i, distance) in by_distance {
            let count = self.leaves[i as usize].count as usize;
            if !taken.is_empty() && placements + count > budget {
                break;
            }
            taken.push(i);
            placements += count;
            reach = distance;
        }
        WorkingSet {
            leaves: taken,
            placements,
            reach,
        }
    }
}

/// See [`Partition::working_set`].
#[derive(Debug, Clone, PartialEq)]
pub struct WorkingSet {
    /// Indexes into [`Partition::leaves`], nearest first.
    pub leaves: Vec<u32>,
    pub placements: usize,
    /// Distance to the farthest leaf taken, in metres.
    pub reach: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_cells_centre_maps_back_to_the_same_cell() {
        for (q, r) in [(0, 0), (3, -2), (-5, 7)] {
            let (x, y) = hex_center(q, r, 250.0);
            assert_eq!(world_to_hex(x, y, 250.0), (q, r));
        }
    }

    /// The same numbers `hex.test.ts` pins, so the two sides stay one function.
    #[test]
    fn the_known_points_land_in_the_cells_the_frontend_puts_them_in() {
        let (x, y) = hex_center(1, 0, 100.0);
        assert_eq!(world_to_hex(x + 40.0, y, 100.0), (1, 0));
        assert_eq!(world_to_hex(x - 40.0, y, 100.0), (1, 0));
        // Values printed by `worldToHex` in `hex.ts`.
        assert_eq!(world_to_hex(0.0, 0.0, 100.0), (0, 0));
        assert_eq!(world_to_hex(1000.0, 1000.0, 250.0), (1, 3));
        assert_eq!(world_to_hex(-2000.0, 500.0, 250.0), (-5, 1));
        assert_eq!(world_to_hex(-0.5, -0.5, 1.0), (0, 0));
        assert_eq!(world_to_hex(123.4, -987.6, 100.0), (4, -7));
    }

    #[test]
    fn the_key_matches_the_frontends() {
        assert_eq!(hex_key(0, 0), 32768 * 65536 + 32768);
        assert_eq!(hex_key(-1, 2), 32767 * 65536 + 32770);
    }

    #[test]
    fn the_sizes_double_from_the_finest() {
        assert_eq!(
            sizes_for(100.0, 5),
            vec![1600.0, 800.0, 400.0, 200.0, 100.0]
        );
    }

    /// A dense knot at the origin and a few strays far away, as in
    /// `hexAdaptive.test.ts`.
    fn world() -> (Vec<f32>, Vec<f32>, Vec<f32>) {
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        for i in 0..400 {
            xs.push(((i % 20) * 8 - 80) as f32);
            ys.push(((i / 20) * 8 - 80) as f32);
        }
        for (x, y) in [(5000.0, 5000.0), (5100.0, 5050.0), (-6000.0, 2000.0)] {
            xs.push(x);
            ys.push(y);
        }
        let zs = vec![10.0; xs.len()];
        (xs, ys, zs)
    }

    fn params(threshold: usize) -> PartitionParams {
        PartitionParams {
            sizes: vec![2000.0, 500.0, 125.0],
            threshold,
        }
    }

    fn build(threshold: usize) -> (Partition, Vec<f32>, Vec<f32>) {
        let (xs, ys, zs) = world();
        let ids: Vec<u32> = (0..xs.len() as u32).collect();
        (
            Partition::build(&xs, &ys, &zs, &ids, params(threshold)),
            xs,
            ys,
        )
    }

    #[test]
    fn every_placement_lands_in_exactly_one_leaf() {
        let (partition, xs, _) = build(50);
        let total: u32 = partition.leaves.iter().map(|l| l.count).sum();
        assert_eq!(total as usize, xs.len());
        let mut seen = partition.members.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), xs.len(), "no placement is in two leaves");
    }

    #[test]
    fn the_dense_knot_is_split_finer_than_the_empty_country() {
        let (partition, _, _) = build(50);
        let level_at = |x: f64, y: f64| {
            partition
                .leaves
                .iter()
                .filter(|l| {
                    let size = partition.params.sizes[l.level as usize];
                    world_to_hex(x, y, size) == (l.q, l.r)
                })
                .map(|l| l.level)
                .next()
        };
        let city = level_at(0.0, 0.0).expect("a leaf at the knot");
        let country = level_at(5000.0, 5000.0).expect("a leaf in the country");
        assert!(city > country, "city {city}, country {country}");
        assert_eq!(country, 0);
        assert!(!partition.splits.is_empty());
    }

    #[test]
    fn a_leaf_over_the_threshold_only_survives_at_the_finest_level() {
        let (partition, _, _) = build(20);
        let finest = (partition.params.sizes.len() - 1) as u8;
        for leaf in &partition.leaves {
            if leaf.count > 20 {
                assert_eq!(leaf.level, finest);
            }
        }
    }

    #[test]
    fn a_leafs_members_are_ascending() {
        let (partition, _, _) = build(50);
        for leaf in &partition.leaves {
            let members = partition.members_of(leaf);
            assert!(members.windows(2).all(|w| w[0] < w[1]), "{leaf:?}");
        }
    }

    #[test]
    fn a_leaf_reports_its_mean_height_and_its_members() {
        let (xs, ys, _) = world();
        let zs: Vec<f32> = (0..xs.len()).map(|i| i as f32).collect();
        let ids: Vec<u32> = (0..xs.len() as u32).collect();
        let partition = Partition::build(&xs, &ys, &zs, &ids, params(50));
        for leaf in &partition.leaves {
            let members = partition.members_of(leaf);
            assert_eq!(members.len(), leaf.count as usize);
            let mean: f64 = members
                .iter()
                .map(|&id| f64::from(zs[id as usize]))
                .sum::<f64>()
                / members.len() as f64;
            assert!((f64::from(leaf.z) - mean).abs() < 1e-3);
        }
    }

    #[test]
    fn only_the_ids_asked_for_take_part() {
        let (xs, ys, zs) = world();
        let ids: Vec<u32> = (400..403).collect();
        let partition = Partition::build(&xs, &ys, &zs, &ids, params(50));
        let total: u32 = partition.leaves.iter().map(|l| l.count).sum();
        assert_eq!(total, 3);
    }

    #[test]
    fn a_leaf_is_found_by_its_cell() {
        let (partition, _, _) = build(50);
        let first = partition.leaves[0].clone();
        assert_eq!(
            partition.leaf((first.level, first.q, first.r)),
            Some(&first)
        );
        assert_eq!(partition.leaf((0, 9999, 9999)), None);
    }

    #[test]
    fn the_working_set_is_the_nearest_leaves_under_the_budget() {
        let (partition, _, _) = build(50);
        let set = partition.working_set(0.0, 0.0, 100);
        assert!(set.placements <= 100 || set.leaves.len() == 1);
        // Nearest first.
        let distances: Vec<f64> = set
            .leaves
            .iter()
            .map(|&i| {
                let leaf = &partition.leaves[i as usize];
                leaf.x.hypot(leaf.y)
            })
            .collect();
        assert!(distances.windows(2).all(|w| w[0] <= w[1]));
        // A budget of nothing still takes the cell under the camera.
        assert_eq!(partition.working_set(0.0, 0.0, 0).leaves.len(), 1);
        // A budget of everything takes every leaf.
        let all = partition.working_set(0.0, 0.0, usize::MAX);
        assert_eq!(all.leaves.len(), partition.leaves.len());
    }

    #[test]
    fn dense_zones_reach_metres_and_sparse_ones_reach_kilometres() {
        let (partition, _, _) = build(50);
        let city = partition.working_set(0.0, 0.0, 300);
        let country = partition.working_set(5000.0, 5000.0, 300);
        assert!(
            country.reach > city.reach,
            "{} vs {}",
            country.reach,
            city.reach
        );
    }
}
