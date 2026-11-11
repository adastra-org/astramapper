//! Which load phase an archetype belongs to, decided from its `.ytyp` data.
//!
//! The streamer loads terrain, then structures, then decals, then vegetation
//! (`docs/hex-streaming.md`). It used to tell them apart by whole-word tokens in
//! the archetype *name*, which misses what the name does not say: puddles
//! (`ap1_01_c_puds_06`), runway paint (`ap1_01_d_runw131`), kelp, weeds, cattails.
//! The archetype record carries better signals, and the index of every
//! archetype (`ArchetypeIndexes::all`) is already built for the placement read,
//! so reading them here costs nothing extra.
//!
//! # What the data can and cannot say
//!
//! Measured over the base game (159,375 archetypes, 83,966 with a resolvable
//! name; `examples/archetype_flags.rs`):
//!
//! - **Vegetation**: the *Double-sided rendering* flag (`1 << 16`) is set on 13%
//!   of archetypes whose name says vegetation and on 0.07% of the rest, and the
//!   rest it does name are kelp, weeds and cattails the name tokens missed.
//! - **Decal**: no flag alone separates them (*Dont Cast Shadows* is on 68% of
//!   decals and 19% of everything else), but *Dont Cast Shadows* on a drawable
//!   whose thinnest bounding-box axis is a few centimetres and whose other two
//!   axes are real extents is a flat quad: a puddle, a road marking, dirt.
//! - **Terrain**: nothing. Flat and large (a bounding box over 100 m, flat) hits
//!   444 of 932 terrain-named archetypes and 9,558 that are not (road splines,
//!   walls, wires). The ground layer is never unloaded, so a false positive is
//!   resident for the session; terrain stays on its name until a real signal
//!   (the drawable's shader names, which need a read of the model) exists.
//!
//! The class is `None` when the data says nothing, and the frontend then falls
//! back to the name tokens.

use super::mlo::ArchetypeInfo;
use crate::meta::typed::AssetType;

/// `rage__fwArchetypeDef` flag: *Dont Cast Shadows*.
pub const FLAG_DONT_CAST_SHADOWS: u32 = 1 << 13;
/// `rage__fwArchetypeDef` flag: *Double-sided rendering*.
pub const FLAG_DOUBLE_SIDED: u32 = 1 << 16;

/// A decal's thinnest bounding-box axis is below this, in metres.
const DECAL_MAX_THICKNESS: f32 = 0.1;
/// ...and its second-thinnest is at least this, which is what makes it a plane
/// and not a wire or a rod.
const DECAL_MIN_SPAN: f32 = 1.0;

/// The load phase the archetype data decides. The numbers are the wire contract
/// with `src/lib/world/placements.ts`; append, never renumber.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[repr(u8)]
pub enum ArchetypeClass {
    /// The data says nothing; the name decides.
    #[default]
    None = 0,
    Decal = 1,
    Vegetation = 2,
}

/// Classify one archetype. See the module note for what each rule rests on.
pub fn classify_archetype(info: &ArchetypeInfo) -> ArchetypeClass {
    // An interior is not a model, and only a drawable or a fragment draws.
    if info.is_mlo {
        return ArchetypeClass::None;
    }
    let drawable = match info.asset_type {
        AssetType::AssetDrawable => true,
        AssetType::AssetFragment => false,
        _ => return ArchetypeClass::None,
    };

    // Checked first: it is the strongest single signal, and a foliage card is a
    // plane too.
    if info.flags & FLAG_DOUBLE_SIDED != 0 {
        return ArchetypeClass::Vegetation;
    }

    if drawable && info.flags & FLAG_DONT_CAST_SHADOWS != 0 {
        let mut extent = [
            info.bb_max[0] - info.bb_min[0],
            info.bb_max[1] - info.bb_min[1],
            info.bb_max[2] - info.bb_min[2],
        ];
        // A NaN or inverted box says nothing.
        if extent.iter().all(|e| e.is_finite() && *e >= 0.0) {
            extent.sort_by(f32::total_cmp);
            if extent[0] < DECAL_MAX_THICKNESS && extent[1] >= DECAL_MIN_SPAN {
                return ArchetypeClass::Decal;
            }
        }
    }
    ArchetypeClass::None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(flags: u32, asset_type: AssetType, extent: [f32; 3]) -> ArchetypeInfo {
        ArchetypeInfo {
            name: 1,
            asset_name: 0,
            asset_type,
            drawable_dictionary: 0,
            texture_dictionary: 0,
            physics_dictionary: 0,
            bb_min: [-extent[0] / 2.0, -extent[1] / 2.0, 0.0],
            bb_max: [extent[0] / 2.0, extent[1] / 2.0, extent[2]],
            lod_dist: 100.0,
            flags,
            is_mlo: false,
        }
    }

    #[test]
    fn a_double_sided_drawable_is_vegetation() {
        let class =
            classify_archetype(&info(FLAG_DOUBLE_SIDED, AssetType::AssetDrawable, [3.0; 3]));
        assert_eq!(class, ArchetypeClass::Vegetation);
    }

    #[test]
    fn a_double_sided_fragment_is_vegetation_too() {
        let class =
            classify_archetype(&info(FLAG_DOUBLE_SIDED, AssetType::AssetFragment, [3.0; 3]));
        assert_eq!(class, ArchetypeClass::Vegetation);
    }

    #[test]
    fn a_thin_plane_that_casts_no_shadow_is_a_decal() {
        let class = classify_archetype(&info(
            FLAG_DONT_CAST_SHADOWS,
            AssetType::AssetDrawable,
            [8.0, 12.0, 0.02],
        ));
        assert_eq!(class, ArchetypeClass::Decal);
    }

    #[test]
    fn a_thin_plane_is_not_a_decal_without_the_no_shadow_flag() {
        let class = classify_archetype(&info(0, AssetType::AssetDrawable, [8.0, 12.0, 0.02]));
        assert_eq!(class, ArchetypeClass::None);
    }

    #[test]
    fn a_wire_is_not_a_plane() {
        // Thin in two axes: a wire or a rod, not a decal.
        let class = classify_archetype(&info(
            FLAG_DONT_CAST_SHADOWS,
            AssetType::AssetDrawable,
            [10.0, 0.02, 0.02],
        ));
        assert_eq!(class, ArchetypeClass::None);
    }

    #[test]
    fn a_thick_no_shadow_prop_is_not_a_decal() {
        let class = classify_archetype(&info(
            FLAG_DONT_CAST_SHADOWS,
            AssetType::AssetDrawable,
            [8.0, 12.0, 3.0],
        ));
        assert_eq!(class, ArchetypeClass::None);
    }

    #[test]
    fn a_fragment_is_never_a_decal() {
        let class = classify_archetype(&info(
            FLAG_DONT_CAST_SHADOWS,
            AssetType::AssetFragment,
            [8.0, 12.0, 0.02],
        ));
        assert_eq!(class, ArchetypeClass::None);
    }

    #[test]
    fn vegetation_wins_over_decal_for_a_double_sided_plane() {
        let class = classify_archetype(&info(
            FLAG_DOUBLE_SIDED | FLAG_DONT_CAST_SHADOWS,
            AssetType::AssetDrawable,
            [8.0, 12.0, 0.02],
        ));
        assert_eq!(class, ArchetypeClass::Vegetation);
    }

    #[test]
    fn assets_that_do_not_draw_say_nothing() {
        for asset in [
            AssetType::AssetDrawableDictionary,
            AssetType::AssetTxd,
            AssetType::Uninitialised,
            AssetType::Unknown(-1),
        ] {
            assert_eq!(
                classify_archetype(&info(FLAG_DOUBLE_SIDED, asset, [3.0; 3])),
                ArchetypeClass::None,
                "{asset:?}"
            );
        }
    }

    #[test]
    fn an_interior_says_nothing() {
        let mut interior = info(FLAG_DOUBLE_SIDED, AssetType::AssetDrawable, [3.0; 3]);
        interior.is_mlo = true;
        assert_eq!(classify_archetype(&interior), ArchetypeClass::None);
    }

    #[test]
    fn a_degenerate_box_says_nothing() {
        let mut broken = info(
            FLAG_DONT_CAST_SHADOWS,
            AssetType::AssetDrawable,
            [8.0, 12.0, 0.02],
        );
        broken.bb_max[0] = f32::NAN;
        assert_eq!(classify_archetype(&broken), ArchetypeClass::None);
        let mut inverted = info(
            FLAG_DONT_CAST_SHADOWS,
            AssetType::AssetDrawable,
            [8.0, 12.0, 0.02],
        );
        inverted.bb_min = [5.0; 3];
        inverted.bb_max = [-5.0; 3];
        assert_eq!(classify_archetype(&inverted), ArchetypeClass::None);
    }

    #[test]
    fn plain_structures_say_nothing() {
        let class = classify_archetype(&info(0, AssetType::AssetDrawable, [10.0, 10.0, 10.0]));
        assert_eq!(class, ArchetypeClass::None);
    }
}
