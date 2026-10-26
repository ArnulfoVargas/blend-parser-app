//! The `.bpproj` file: a ZIP with the document's settings as JSON and the
//! model as the core's bytes, deflated (textures make up most of it).

use std::io::{Read, Seek, Write};

use zip::write::SimpleFileOptions;

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
