//! What a file in the archives is, from its name.
//!
//! There is no reference file for this. `GameFileCache.cs` switches on the
//! extension in a dozen places and `GameFileType` enumerates the parsers; this
//! collects that knowledge in one table so the Explorer can label and route
//! every entry, including the many it cannot parse yet.
//!
//! The extension is a *claim*, not a fact. It picks the parser to try and the
//! icon to draw; whether the bytes actually are what the name says is settled by
//! [`crate::text::sniff`] and by the parser failing.

use serde::Serialize;

/// The family a file belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum FileKind {
    /// An `.rpf` — browsable as a directory.
    Archive,
    /// A single drawable: `.ydr`.
    Model,
    /// A dictionary of drawables: `.ydd`.
    ModelDictionary,
    /// A fragment — a drawable plus physics: `.yft`.
    Fragment,
    /// A particle effect dictionary, which embeds drawables: `.ypt`.
    Particle,
    /// A texture dictionary: `.ytd`.
    Texture,
    /// A standalone image: `.dds`, `.png`.
    Image,
    /// Collision geometry: `.ybn`.
    Bounds,
    /// Metadata in one of the RSC/PSO/RBF encodings: `.ymap`, `.ytyp`, `.ymt`.
    Meta,
    /// Animation: `.ycd`, `.yed`, `.yfd`.
    Clip,
    /// Paths and navigation: `.ynd`, `.ynv`.
    Nav,
    /// An audio wave container: `.awc`.
    Audio,
    /// Audio metadata that names and sequences the waves: `.rel`, `.oac`.
    AudioMetadata,
    /// Compiled script: `.ysc`.
    Script,
    /// Text, or something conventionally stored as text: `.xml`, `.meta`,
    /// `.txt`, `.ini`, `.sps`, `.nametable`.
    Text,
    /// A known binary blob with no viewer: `.dat`, `.gfx`, `.fxc`, `.gxt2`.
    Data,
    /// Anything else.
    Unknown,
}

impl FileKind {
    /// Classify by the last extension of a file name.
    pub fn from_name(name: &str) -> Self {
        let lower = name.to_lowercase();
        let extension = match lower.rfind('.') {
            Some(i) if i > 0 => &lower[i + 1..],
            _ => return Self::Unknown,
        };
        Self::from_extension(extension)
    }

    pub fn from_extension(extension: &str) -> Self {
        use FileKind::*;
        match extension {
            "rpf" => Archive,

            "ydr" => Model,
            "ydd" => ModelDictionary,
            "yft" => Fragment,
            "ypt" => Particle,

            "ytd" => Texture,
            "dds" | "png" | "jpg" | "jpeg" | "bmp" | "tga" => Image,

            "ybn" => Bounds,

            "ymap" | "ytyp" | "ymt" | "ymf" | "pso" => Meta,

            "ycd" | "yed" | "yfd" | "yld" | "ypdb" => Clip,
            "ynd" | "ynv" | "yvr" | "ywr" => Nav,

            "awc" => Audio,
            "rel" | "oac" => AudioMetadata,
            "ysc" => Script,

            "xml" | "meta" | "txt" | "ini" | "cfg" | "sps" | "nametable" | "log" | "csv"
            | "shop" | "list" | "html" | "json" | "lua" | "js" | "vsh" | "fsh" => Text,

            "dat" | "gfx" | "fxc" | "gxt2" | "cut" | "bik" | "wav" | "asi" | "dll" | "exe"
            | "bin" | "cache" | "zip" | "oiv" => Data,

            _ => Unknown,
        }
    }

    /// A short human label for the UI.
    pub fn label(self) -> &'static str {
        use FileKind::*;
        match self {
            Archive => "archive",
            Model => "drawable",
            ModelDictionary => "drawable dictionary",
            Fragment => "fragment",
            Particle => "particle dictionary",
            Texture => "texture dictionary",
            Image => "image",
            Bounds => "collision bounds",
            Meta => "metadata",
            Clip => "animation",
            Nav => "navigation",
            Audio => "audio container",
            AudioMetadata => "audio metadata",
            Script => "script",
            Text => "text",
            Data => "data",
            Unknown => "unknown",
        }
    }

    /// Whether a 3D preview is possible today. `Fragment` and `Particle` embed
    /// drawables too, but reaching them needs `Frag.cs` and `Particle.cs`, which
    /// are not ported — see `docs/formats.md`.
    pub fn has_model(self) -> bool {
        matches!(self, FileKind::Model | FileKind::ModelDictionary)
    }

    /// Whether a texture preview is possible today. Only `.ytd` — a `.ydr` and
    /// a `.yft` each embed a dictionary too, but behind pointers the drawable
    /// reader steps past, and `.dds` needs a DDS *reader*, which is not ported.
    pub fn has_textures(self) -> bool {
        matches!(self, FileKind::Texture)
    }

    /// Whether audio can be played out of this. Only `.awc` — `.rel` names and
    /// sequences the waves but holds none, and `RelFile.cs` is not ported.
    pub fn has_audio(self) -> bool {
        matches!(self, FileKind::Audio)
    }

    /// Whether the file is *expected* to be readable as text. A false here does
    /// not forbid the text viewer: sniffing decides, and plenty of `.dat` files
    /// in the archives are plain ASCII.
    pub fn is_text(self) -> bool {
        matches!(self, FileKind::Text)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_the_resource_extensions() {
        assert_eq!(FileKind::from_name("prop_bench_01a.ydr"), FileKind::Model);
        assert_eq!(
            FileKind::from_name("mp_m_freemode_01.ydd"),
            FileKind::ModelDictionary
        );
        assert_eq!(FileKind::from_name("adder.yft"), FileKind::Fragment);
        assert_eq!(FileKind::from_name("v_res_d_bskt.ytd"), FileKind::Texture);
        assert_eq!(FileKind::from_name("dt1_01.ymap"), FileKind::Meta);
        assert_eq!(FileKind::from_name("x64a.rpf"), FileKind::Archive);
    }

    #[test]
    fn classifies_by_the_last_extension_only() {
        assert_eq!(FileKind::from_name("vehicles.meta.xml"), FileKind::Text);
        assert_eq!(FileKind::from_name("handling.meta"), FileKind::Text);
    }

    /// A leading dot is not an extension separator, matching
    /// `RpfEntry::short_name_lower`.
    #[test]
    fn treats_a_leading_dot_as_part_of_the_name() {
        assert_eq!(FileKind::from_name(".hidden"), FileKind::Unknown);
        assert_eq!(FileKind::from_name("noextension"), FileKind::Unknown);
    }

    #[test]
    fn is_case_insensitive() {
        assert_eq!(FileKind::from_name("PROP_BENCH.YDR"), FileKind::Model);
    }

    /// `.rel` is audio, but it is metadata about waves rather than waves.
    #[test]
    fn only_the_wave_container_claims_audio() {
        assert_eq!(FileKind::from_name("resident.awc"), FileKind::Audio);
        assert!(FileKind::Audio.has_audio());
        assert_eq!(
            FileKind::from_name("dlcbusiness_game.dat151.rel"),
            FileKind::AudioMetadata
        );
        assert!(
            !FileKind::AudioMetadata.has_audio(),
            "RelFile.cs is not ported"
        );
    }

    #[test]
    fn only_the_two_ported_formats_claim_a_model() {
        assert!(FileKind::Model.has_model());
        assert!(FileKind::ModelDictionary.has_model());
        assert!(!FileKind::Fragment.has_model(), "Frag.cs is not ported");
        assert!(!FileKind::Texture.has_model());
    }

    /// `.dds` is an image but there is no DDS reader, and the drawables embed
    /// dictionaries the resource layer walks past.
    #[test]
    fn only_the_texture_dictionary_claims_textures() {
        assert!(FileKind::Texture.has_textures());
        assert!(!FileKind::Image.has_textures());
        assert!(!FileKind::Model.has_textures());
        assert!(!FileKind::Fragment.has_textures());
    }
}
