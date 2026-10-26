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
