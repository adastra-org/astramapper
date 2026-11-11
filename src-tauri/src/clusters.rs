//! Zone clusters: named sets of leaf cells.
//!
//! A cluster is a layer *above* the leaves of [`crate::cells::Partition`], not a
//! second way of loading. The regions are drawn by hand in `data/zones.json`
//! (named boxes and polygons in game coordinates); each leaf belongs to the first
//! region, in file order, that contains its centre, and the leaves in none fall
//! into a built-in *countryside* cluster. So every leaf is in exactly one cluster,
//! and a cluster is just a list of leaf indexes the frontend already knows how to
//! ask for.
//!
//! What this buys: memory and model cost can be read per zone (placements,
//! distinct models, and measured model bytes via
//! [`crate::world_index::world_index_cluster_measure`]), and a whole zone can be
//! loaded at once by asking the cell streamer for all of its leaves.
//!
//! The terrain pseudo-cell is map-wide and belongs to no cluster.
//!
//! Regions are assigned by the leaf's *centre*. A leaf that straddles a region's
//! edge goes wholly to one side; leaves are 100 m to 1.6 km across, so a border
//! is accurate to about that.

use std::collections::HashSet;
use std::sync::OnceLock;

use serde::{Deserialize, Serialize};

use crate::cells::Partition;

/// The key of the cluster that takes every leaf no region contains.
pub const COUNTRYSIDE_KEY: &str = "countryside";
/// Its display name.
pub const COUNTRYSIDE_NAME: &str = "Countryside";

/// The hand-drawn regions that ship with the app.
const BUILTIN: &str = include_str!("../data/zones.json");

/// An area in game coordinates (X east, Y north).
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Shape {
    Box { min: [f64; 2], max: [f64; 2] },
    Polygon { points: Vec<[f64; 2]> },
}

impl Shape {
    /// Whether a ground-plane point is inside. A point on a box's edge counts as in.
    pub fn contains(&self, x: f64, y: f64) -> bool {
        match self {
            Shape::Box { min, max } => x >= min[0] && x <= max[0] && y >= min[1] && y <= max[1],
            Shape::Polygon { points } => polygon_contains(points, x, y),
        }
    }

    fn validate(&self, key: &str) -> Result<(), String> {
        match self {
            Shape::Box { min, max } => {
                if min[0] >= max[0] || min[1] >= max[1] {
                    return Err(format!("region '{key}': box min must be below max"));
                }
            }
            Shape::Polygon { points } => {
                if points.len() < 3 {
                    return Err(format!("region '{key}': a polygon needs 3 points or more"));
                }
            }
        }
        Ok(())
    }
}

/// Even-odd ray casting.
fn polygon_contains(points: &[[f64; 2]], x: f64, y: f64) -> bool {
    let mut inside = false;
    let mut j = points.len() - 1;
    for i in 0..points.len() {
        let (xi, yi) = (points[i][0], points[i][1]);
        let (xj, yj) = (points[j][0], points[j][1]);
        if (yi > y) != (yj > y) && x < (xj - xi) * (y - yi) / (yj - yi) + xi {
            inside = !inside;
        }
        j = i;
    }
    inside
}

/// One named zone.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct Region {
    /// Stable id, lower-case, used by the commands.
    pub key: String,
    pub name: String,
    pub shape: Shape,
}

#[derive(Deserialize)]
struct RegionFile {
    regions: Vec<Region>,
}

/// The regions, in priority order.
#[derive(Debug, Clone, PartialEq)]
pub struct Regions(pub Vec<Region>);

impl Regions {
    /// Parse and validate a regions file: unique non-empty keys (none of them the
    /// countryside's), shapes that enclose an area.
    pub fn parse(json: &str) -> Result<Self, String> {
        let file: RegionFile =
            serde_json::from_str(json).map_err(|e| format!("zones file: {e}"))?;
        let mut seen = HashSet::new();
        for region in &file.regions {
            if region.key.is_empty() || region.key == COUNTRYSIDE_KEY {
                return Err(format!("region key '{}' is not allowed", region.key));
            }
            if !seen.insert(region.key.as_str()) {
                return Err(format!("region key '{}' appears twice", region.key));
            }
            region.shape.validate(&region.key)?;
        }
        Ok(Self(file.regions))
    }

    /// The regions that ship in `data/zones.json`.
    pub fn builtin() -> &'static Regions {
        static REGIONS: OnceLock<Regions> = OnceLock::new();
        REGIONS.get_or_init(|| {
            // Checked by a test, so a bad file fails the tests and never reaches
            // a user.
            Regions::parse(BUILTIN).unwrap_or_else(|e| panic!("data/zones.json: {e}"))
        })
    }

    /// Index of the first region containing a point.
    pub fn region_at(&self, x: f64, y: f64) -> Option<usize> {
        self.0.iter().position(|r| r.shape.contains(x, y))
    }
}

/// One cluster: a region and the leaves that fell in it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Cluster {
    pub key: String,
    pub name: String,
    /// Indexes into [`Partition::leaves`], in leaf order. Not sent: the frontend
    /// has the per-leaf cluster column.
    #[serde(skip)]
    pub leaves: Vec<u32>,
    /// How many leaves, for the wire.
    #[serde(rename = "leaves")]
    pub leaf_count: usize,
    pub placements: u64,
    /// Distinct archetype names across its placements.
    pub models: usize,
}

/// Every leaf's cluster.
#[derive(Debug, Clone, Default)]
pub struct Clusters {
    /// The regions in file order, then the countryside last (always present, so
    /// cluster ids are stable whether or not it holds anything).
    pub list: Vec<Cluster>,
    /// Cluster index of each leaf, parallel to [`Partition::leaves`].
    pub of_leaf: Vec<u16>,
}

fn empty_cluster(key: &str, name: &str) -> Cluster {
    Cluster {
        key: key.to_string(),
        name: name.to_string(),
        leaves: Vec::new(),
        leaf_count: 0,
        placements: 0,
        models: 0,
    }
}

impl Clusters {
    /// Assign every leaf to a cluster by its centre.
    ///
    /// `name_of` is the placement store's name slot per placement id, so the
    /// distinct models of a cluster are counted from its real members.
    pub fn build(regions: &Regions, partition: &Partition, name_of: &[u32]) -> Self {
        let mut list: Vec<Cluster> = regions
            .0
            .iter()
            .map(|r| empty_cluster(&r.key, &r.name))
            .collect();
        list.push(empty_cluster(COUNTRYSIDE_KEY, COUNTRYSIDE_NAME));
        let countryside = list.len() - 1;

        let mut of_leaf = Vec::with_capacity(partition.leaves.len());
        for (i, leaf) in partition.leaves.iter().enumerate() {
            let at = regions.region_at(leaf.x, leaf.y).unwrap_or(countryside);
            of_leaf.push(at as u16);
            list[at].leaves.push(i as u32);
            list[at].placements += u64::from(leaf.count);
        }
        for cluster in &mut list {
            cluster.leaf_count = cluster.leaves.len();
            let mut names = HashSet::new();
            for &leaf in &cluster.leaves {
                let leaf = &partition.leaves[leaf as usize];
                for &id in partition.members_of(leaf) {
                    names.insert(name_of[id as usize]);
                }
            }
            cluster.models = names.len();
        }
        Self { list, of_leaf }
    }

    /// A cluster by its key.
    pub fn find(&self, key: &str) -> Option<&Cluster> {
        self.list.iter().find(|c| c.key == key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cells::PartitionParams;

    #[test]
    fn the_shipped_regions_file_is_valid() {
        let regions = Regions::builtin();
        assert!(regions.0.len() >= 4);
        let city = regions.region_at(195.0, -933.0).expect("Legion Square");
        assert_eq!(regions.0[city].key, "city");
    }

    #[test]
    fn the_first_region_wins_where_two_overlap() {
        let regions = Regions::builtin();
        // The airport sits inside the box the south region also covers.
        let at = regions.region_at(-1337.0, -3044.0).expect("LS airport");
        assert_eq!(regions.0[at].key, "airport");
    }

    #[test]
    fn a_box_and_a_polygon_contain_what_they_should() {
        let b = Shape::Box {
            min: [0.0, 0.0],
            max: [10.0, 10.0],
        };
        assert!(b.contains(5.0, 5.0));
        assert!(b.contains(10.0, 10.0), "an edge is in");
        assert!(!b.contains(10.1, 5.0));
        let t = Shape::Polygon {
            points: vec![[0.0, 0.0], [10.0, 0.0], [0.0, 10.0]],
        };
        assert!(t.contains(2.0, 2.0));
        assert!(!t.contains(8.0, 8.0));
        assert!(!t.contains(-1.0, 1.0));
    }

    #[test]
    fn bad_region_files_are_refused() {
        let bad = |json: &str| Regions::parse(json).expect_err(json);
        bad("not json");
        bad(
            r#"{"regions":[{"key":"countryside","name":"x","shape":{"type":"box","min":[0,0],"max":[1,1]}}]}"#,
        );
        bad(
            r#"{"regions":[{"key":"","name":"x","shape":{"type":"box","min":[0,0],"max":[1,1]}}]}"#,
        );
        bad(
            r#"{"regions":[{"key":"a","name":"x","shape":{"type":"box","min":[1,1],"max":[0,0]}}]}"#,
        );
        bad(
            r#"{"regions":[{"key":"a","name":"x","shape":{"type":"polygon","points":[[0,0],[1,1]]}}]}"#,
        );
        bad(r#"{"regions":[
            {"key":"a","name":"x","shape":{"type":"box","min":[0,0],"max":[1,1]}},
            {"key":"a","name":"y","shape":{"type":"box","min":[0,0],"max":[1,1]}}]}"#);
    }

    /// Placements in two knots (a city one and a far one) and a stray.
    fn world() -> (Partition, Vec<u32>) {
        let mut xs = Vec::new();
        let mut ys = Vec::new();
        let mut names = Vec::new();
        for i in 0..40u32 {
            xs.push(100.0 + (i % 5) as f32 * 10.0);
            ys.push(-900.0 + (i / 5) as f32 * 10.0);
            names.push(i % 4); // four distinct models in the city
        }
        for i in 0..10u32 {
            xs.push(1900.0 + i as f32);
            ys.push(3500.0);
            names.push(10 + i % 2); // two in the north-east
        }
        xs.push(-4000.0);
        ys.push(-100.0);
        names.push(99);
        let zs = vec![0.0; xs.len()];
        let ids: Vec<u32> = (0..xs.len() as u32).collect();
        let partition = Partition::build(
            &xs,
            &ys,
            &zs,
            &ids,
            PartitionParams {
                sizes: vec![1000.0, 250.0],
                threshold: 30,
            },
        );
        (partition, names)
    }

    fn regions() -> Regions {
        Regions::parse(
            r#"{"regions":[
                {"key":"city","name":"City","shape":{"type":"box","min":[-500,-1500],"max":[500,0]}},
                {"key":"east","name":"East","shape":{"type":"box","min":[1000,3000],"max":[3000,4000]}}]}"#,
        )
        .expect("regions")
    }

    #[test]
    fn every_leaf_is_in_exactly_one_cluster_and_the_counts_add_up() {
        let (partition, names) = world();
        let clusters = Clusters::build(&regions(), &partition, &names);
        assert_eq!(clusters.of_leaf.len(), partition.leaves.len());
        let leaves: usize = clusters.list.iter().map(|c| c.leaf_count).sum();
        assert_eq!(leaves, partition.leaves.len());
        let placements: u64 = clusters.list.iter().map(|c| c.placements).sum();
        assert_eq!(placements, 51);
        let mut seen: Vec<u32> = clusters
            .list
            .iter()
            .flat_map(|c| c.leaves.clone())
            .collect();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), partition.leaves.len());
        for (i, &at) in clusters.of_leaf.iter().enumerate() {
            assert!(clusters.list[at as usize].leaves.contains(&(i as u32)));
        }
    }

    #[test]
    fn leaves_are_assigned_by_centre_and_the_rest_are_countryside() {
        let (partition, names) = world();
        let clusters = Clusters::build(&regions(), &partition, &names);
        let city = clusters.find("city").expect("city");
        let east = clusters.find("east").expect("east");
        let rest = clusters.find(COUNTRYSIDE_KEY).expect("countryside");
        assert_eq!(city.placements, 40);
        assert_eq!(east.placements, 10);
        assert_eq!(rest.placements, 1, "the stray is nobody's");
        assert_eq!(
            clusters.list.last().map(|c| c.key.as_str()),
            Some(COUNTRYSIDE_KEY)
        );
    }

    #[test]
    fn a_cluster_counts_its_distinct_models() {
        let (partition, names) = world();
        let clusters = Clusters::build(&regions(), &partition, &names);
        assert_eq!(clusters.find("city").map(|c| c.models), Some(4));
        assert_eq!(clusters.find("east").map(|c| c.models), Some(2));
        assert_eq!(clusters.find(COUNTRYSIDE_KEY).map(|c| c.models), Some(1));
    }

    #[test]
    fn countryside_exists_even_when_empty() {
        let (partition, names) = world();
        let everywhere = Regions::parse(
            r#"{"regions":[{"key":"all","name":"All","shape":{"type":"box","min":[-9000,-9000],"max":[9000,9000]}}]}"#,
        )
        .expect("regions");
        let clusters = Clusters::build(&everywhere, &partition, &names);
        assert_eq!(clusters.list.len(), 2);
        assert_eq!(
            clusters.find(COUNTRYSIDE_KEY).map(|c| c.leaf_count),
            Some(0)
        );
    }
}
