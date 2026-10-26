//! The `.bpproj` file: a ZIP with the document's settings as JSON and the
//! model as the core's bytes, deflated (textures make up most of it).

use std::io::{Read, Seek, Write};

use serde::{Deserialize, Serialize};
use zip::write::SimpleFileOptions;

use crate::PrinterProfile;

/// The layout this build writes; a newer one is refused.
pub const FORMAT: u32 = 1;

/// A notice from the import, kept with the project: a key and the name it is about.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Warning {
    pub code: String,
    pub detail: String,
}

/// The document's own settings, next to the model.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Settings {
    pub format: u32,
    /// The BlendParser that saved it, for reference.
    pub app_version: String,
    /// The model the project came from.
    pub file_name: String,
    pub file_size: u64,
    pub import_warnings: Vec<Warning>,
    /// The whole profile, so a custom printer that has since gone still opens.
    pub printer: PrinterProfile,
    pub nozzle_mm: f32,
    /// Flush the user typed per swap: old colour, new colour, mm³.
    pub flush: Vec<(String, String, f32)>,
    /// Where the model's centre sits from the plate's centre.
    pub offset_mm: [f32; 2],
    /// The filament on each slot that has one, by the slot's colour (`#RRGGBB`).
    #[serde(default)]
    pub filaments: Vec<(String, crate::filament::Filament)>,
    /// How the colour kit was last cut, if it was.
    #[serde(default)]
    pub kit: Option<KitSettings>,
}

/// How the colour kit is cut (see `bp_core::SplitParams`).
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KitSettings {
    pub voxel_mm: f32,
    pub depth_mm: f32,
    pub core_slot: Option<u8>,
    pub min_piece_mm3: f32,
    /// Let colours share a plate (see `bp_core::kit::Bed::mix`).
    #[serde(default)]
    pub mix: bool,
    /// What the ZIP holds.
    #[serde(default)]
    pub contents: KitContents,
}

/// What goes in the kit's ZIP besides its summary, and how sharp its pictures are.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct KitContents {
    pub three_mf: bool,
    pub stl: StlFiles,
    /// The assembly guide with pictures.
    pub guide: bool,
    /// The same steps as plain text.
    pub text: bool,
    pub quality: Quality,
}

impl Default for KitContents {
    fn default() -> Self {
        Self { three_mf: true, stl: StlFiles::Piece, guide: true, text: true, quality: Quality::Medium }
    }
}

impl KitContents {
    /// Anything besides the summary.
    pub fn any(&self) -> bool {
        self.three_mf || self.stl != StlFiles::None || self.guide || self.text
    }
}

/// STL files: none, one per piece, or one per colour with its pieces assembled.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StlFiles {
    None,
    Piece,
    Colour,
}

/// How large and smooth the guide's pictures are, and so the ZIP.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Quality {
    Low,
    Medium,
    High,
}

impl Quality {
    /// Picture size against medium, and supersampling.
    pub fn scale(self) -> (f32, usize) {
        match self {
            Self::Low => (0.7, 1),
            Self::Medium => (1.0, 2),
            Self::High => (1.5, 2),
        }
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum ProjectError {
    /// Not a project, or damaged.
    Corrupt,
    /// Saved by a newer BlendParser.
    Version,
}

/// The settings in `json`, refusing a newer format before reading the rest.
pub fn parse_settings(json: &str) -> Result<Settings, ProjectError> {
    let value: serde_json::Value = serde_json::from_str(json).map_err(|_| ProjectError::Corrupt)?;
    if value["format"].as_u64().is_some_and(|f| f > FORMAT as u64) {
        return Err(ProjectError::Version);
    }
    serde_json::from_value(value).map_err(|_| ProjectError::Corrupt)
}

const SETTINGS: &str = "project.json";
const MODEL: &str = "model.bin";

/// Writes the settings and the model.
pub fn write<W: Write + Seek>(out: W, settings: &str, model: &[u8]) -> zip::result::ZipResult<()> {
    let mut z = zip::ZipWriter::new(out);
    let opts = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .large_file(model.len() > u32::MAX as usize / 2);
    z.start_file(SETTINGS, opts)?;
    z.write_all(settings.as_bytes())?;
    z.start_file(MODEL, opts)?;
    z.write_all(model)?;
    z.finish()?;
    Ok(())
}

/// The settings and the model, as written by [`write`].
pub fn read<R: Read + Seek>(input: R) -> zip::result::ZipResult<(String, Vec<u8>)> {
    let mut z = zip::ZipArchive::new(input)?;
    let mut settings = String::new();
    z.by_name(SETTINGS)?.read_to_string(&mut settings)?;
    let mut model = Vec::new();
    z.by_name(MODEL)?.read_to_end(&mut model)?;
    Ok((settings, model))
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    #[test]
    fn round_trip() {
        let mut buf = Cursor::new(Vec::new());
        super::write(&mut buf, "{\"a\":1}", &[1, 2, 3]).unwrap();
        let (s, m) = super::read(Cursor::new(buf.into_inner())).unwrap();
        assert_eq!((s.as_str(), m.as_slice()), ("{\"a\":1}", &[1u8, 2, 3][..]));
        assert!(super::read(Cursor::new(b"not a zip".to_vec())).is_err());
    }
}
