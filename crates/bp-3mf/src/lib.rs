//! Writes a 3MF that Bambu Studio opens already painted: one object, one part
//! per mesh, millimetre vertices and a `paint_color` per triangle, plus a
//! project config with one filament per colour.

use std::fmt::Write as _;
use std::io::{Seek, Write};

use zip::write::SimpleFileOptions;

pub struct Part<'a> {
    pub name: &'a str,
    pub positions: &'a [[f32; 3]],
    pub tris: &'a [[u32; 3]],
    /// Encoded `paint_color` per triangle.
    pub paint: &'a [String],
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Printer {
    A1,
    A1Mini,
    X1Carbon,
    P1S,
}

impl Printer {
    pub fn from_id(id: &str) -> Self {
        match id {
            "A1M" => Printer::A1Mini,
            "X1C" => Printer::X1Carbon,
            "P1S" => Printer::P1S,
            _ => Printer::A1,
        }
    }

    /// (model, printer preset, process preset, filament preset suffix, plate centre)
    fn presets(self) -> (&'static str, &'static str, &'static str, &'static str, (u32, u32)) {
        match self {
            Printer::A1 => {
                ("Bambu Lab A1", "Bambu Lab A1 0.4 nozzle", "0.20mm Standard @BBL A1", "@BBL A1", (128, 128))
            }
            Printer::A1Mini => {
                ("Bambu Lab A1 mini", "Bambu Lab A1 mini 0.4 nozzle", "0.20mm Standard @BBL A1M", "@BBL A1M", (90, 90))
            }
            Printer::X1Carbon => (
                "Bambu Lab X1 Carbon",
                "Bambu Lab X1 Carbon 0.4 nozzle",
                "0.20mm Standard @BBL X1C",
                "@BBL X1C",
                (128, 128),
            ),
            Printer::P1S => {
                ("Bambu Lab P1S", "Bambu Lab P1S 0.4 nozzle", "0.20mm Standard @BBL X1C", "@BBL X1C", (128, 128))
            }
        }
    }

    /// Printable area in millimetres.
    pub fn bed_mm(self) -> (f32, f32) {
        let c = self.presets().4;
        (c.0 as f32 * 2.0, c.1 as f32 * 2.0)
    }
}

pub struct Model<'a> {
    pub name: &'a str,
    pub parts: Vec<Part<'a>>,
    /// Filament colours as `#RRGGBB`, slot 1 first.
    pub filaments: Vec<String>,
    pub printer: Printer,
}

const NS: &str = r#"xmlns="http://schemas.microsoft.com/3dmanufacturing/core/2015/02" xmlns:BambuStudio="http://schemas.bambulab.com/package/2021" xmlns:p="http://schemas.microsoft.com/3dmanufacturing/production/2015/06" requiredextensions="p""#;

/// Shortest decimal for a coordinate, at most four decimals (1/10 µm).
fn num(out: &mut String, v: f32) {
    let s = format!("{:.4}", v);
    let s = s.trim_end_matches('0').trim_end_matches('.');
    out.push_str(if s == "-0" || s.is_empty() { "0" } else { s });
}

fn escape(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn objects_xml(model: &Model) -> String {
    let tris: usize = model.parts.iter().map(|p| p.tris.len()).sum();
    let mut x = String::with_capacity(tris * 64);
    let _ = write!(
        x,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" {NS}>\n <metadata name=\"BambuStudio:3mfVersion\">2</metadata>\n <resources>\n"
    );
    for (i, p) in model.parts.iter().enumerate() {
        let id = i + 1;
        let _ = write!(x, "  <object id=\"{id}\" p:UUID=\"{id:04x}0000-81cb-4c03-9d28-80fed5dfa1dc\" type=\"model\">\n   <mesh>\n    <vertices>\n");
        for v in p.positions {
            x.push_str("     <vertex x=\"");
            num(&mut x, v[0]);
            x.push_str("\" y=\"");
            num(&mut x, v[1]);
            x.push_str("\" z=\"");
            num(&mut x, v[2]);
            x.push_str("\"/>\n");
        }
        x.push_str("    </vertices>\n    <triangles>\n");
        for (t, paint) in p.tris.iter().zip(p.paint) {
            let _ = writeln!(
                x,
                "     <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\" paint_color=\"{}\"/>",
                t[0], t[1], t[2], paint
            );
        }
        x.push_str("    </triangles>\n   </mesh>\n  </object>\n");
    }
    x.push_str(" </resources>\n <build/>\n</model>\n");
    x
}

fn main_xml(model: &Model, object_id: usize) -> String {
    let (_, _, _, _, (cx, cy)) = model.printer.presets();
    let mut comps = String::new();
    for i in 0..model.parts.len() {
        let _ = writeln!(
            comps,
            "    <component p:path=\"/3D/Objects/object_1.model\" objectid=\"{}\" p:UUID=\"0001{i:04x}-b206-40ff-9872-83e8017abed1\" transform=\"1 0 0 0 1 0 0 0 1 0 0 0\"/>",
            i + 1
        );
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" {NS}>\n \
<metadata name=\"Application\">BambuStudio-02.03.01.00</metadata>\n \
<metadata name=\"BambuStudio:3mfVersion\">2</metadata>\n \
<metadata name=\"BambuStudio:MmPaintingVersion\">1</metadata>\n \
<metadata name=\"Title\">{title}</metadata>\n <resources>\n  \
<object id=\"{object_id}\" p:UUID=\"00000001-61cb-4c03-9d28-80fed5dfa1dc\" type=\"model\">\n   <components>\n{comps}   </components>\n  </object>\n \
</resources>\n <build p:UUID=\"2c7c17d8-22b5-4d84-8835-1976022ea369\">\n  \
<item objectid=\"{object_id}\" p:UUID=\"00000002-b1ec-4553-aec9-835e5b724bb4\" transform=\"1 0 0 0 1 0 0 0 1 {cx} {cy} 0\" printable=\"1\"/>\n \
</build>\n</model>\n",
        title = escape(model.name)
    )
}

fn settings_xml(model: &Model, object_id: usize) -> String {
    let total: usize = model.parts.iter().map(|p| p.tris.len()).sum();
    let mut parts = String::new();
    for (i, p) in model.parts.iter().enumerate() {
        let _ = write!(
            parts,
            "    <part id=\"{}\" subtype=\"normal_part\">\n      <metadata key=\"name\" value=\"{}\"/>\n      \
<metadata key=\"matrix\" value=\"1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1\"/>\n      \
<mesh_stat face_count=\"{}\" edges_fixed=\"0\" degenerate_facets=\"0\" facets_removed=\"0\" facets_reversed=\"0\" backwards_edges=\"0\"/>\n    </part>\n",
            i + 1,
            escape(p.name),
            p.tris.len()
        );
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<config>\n  <object id=\"{object_id}\">\n    \
<metadata key=\"name\" value=\"{name}\"/>\n    <metadata key=\"extruder\" value=\"1\"/>\n    \
<metadata face_count=\"{total}\"/>\n{parts}  </object>\n  <plate>\n    \
<metadata key=\"plater_id\" value=\"1\"/>\n    <metadata key=\"plater_name\" value=\"\"/>\n    \
<metadata key=\"locked\" value=\"false\"/>\n    <model_instance>\n      \
<metadata key=\"object_id\" value=\"{object_id}\"/>\n      <metadata key=\"instance_id\" value=\"0\"/>\n      \
<metadata key=\"identify_id\" value=\"1\"/>\n    </model_instance>\n  </plate>\n  <assemble>\n  </assemble>\n</config>\n",
        name = escape(model.name)
    )
}

fn project_json(model: &Model) -> String {
    let (printer_model, printer_id, process, suffix, _) = model.printer.presets();
    let n = model.filaments.len().max(1);
    let rep = |v: &str| vec![v.to_string(); n];
    let flush: Vec<String> = (0..n * n).map(|i| if i / n == i % n { "0" } else { "280" }.to_string()).collect();
    serde_json::to_string_pretty(&serde_json::json!({
        "filament_colour": model.filaments.iter().map(|c| format!("{}FF", c.to_uppercase())).collect::<Vec<_>>(),
        "filament_settings_id": rep(&format!("Bambu PLA Basic {suffix}")),
        "filament_type": rep("PLA"),
        "filament_minimal_purge_on_wipe_tower": rep("15"),
        "filament_flow_ratio": rep("0.98"),
        "filament_diameter": rep("1.75"),
        "wipe": rep("1"),
        "wipe_distance": rep("2"),
        "retract_length_toolchange": rep("2"),
        "retract_restart_extra_toolchange": rep("0"),
        "standby_temperature_delta": "-5",
        "enable_prime_tower": if n > 1 { "1" } else { "0" },
        "prime_tower_width": "20",
        "prime_tower_brim_width": "3",
        "prime_volume": "20",
        "flush_into_infill": "0",
        "flush_into_objects": "0",
        "flush_into_support": "1",
        "flush_multiplier": "1",
        "flush_volumes_matrix": flush,
        "flush_volumes_vector": vec!["140"; 2 * n],
        "wipe_tower_x": ["15"],
        "wipe_tower_y": ["145"],
        "wipe_tower_rotation_angle": "0",
        "wipe_tower_no_sparse_layers": "0",
        "wipe_speed": "80%",
        "single_extruder_multi_material": "1",
        "print_sequence": "by layer",
        "printer_model": printer_model,
        "printer_settings_id": printer_id,
        "nozzle_diameter": ["0.4"],
        "print_settings_id": process,
        "layer_change_gcode": "; layer change\nG92 E0\n",
        "from": "project",
        "name": "project_settings",
        "version": "02.03.01.00"
    }))
    .unwrap_or_default()
}

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\n</Types>\n";

fn rels(target: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n <Relationship Target=\"{target}\" Id=\"rel-1\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/>\n</Relationships>\n"
    )
}

pub fn write<W: Write + Seek>(out: W, model: &Model) -> zip::result::ZipResult<()> {
    let object_id = model.parts.len() + 1;
    let mut z = zip::ZipWriter::new(out);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let files: [(&str, String); 7] = [
        ("[Content_Types].xml", CONTENT_TYPES.to_string()),
        ("_rels/.rels", rels("/3D/3dmodel.model")),
        ("3D/3dmodel.model", main_xml(model, object_id)),
        ("3D/_rels/3dmodel.model.rels", rels("/3D/Objects/object_1.model")),
        ("3D/Objects/object_1.model", objects_xml(model)),
        ("Metadata/model_settings.config", settings_xml(model, object_id)),
        ("Metadata/project_settings.config", project_json(model)),
    ];
    for (name, body) in files {
        z.start_file(name, opts)?;
        z.write_all(body.as_bytes())?;
    }
    z.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Read};

    #[test]
    fn writes_a_bambu_package() {
        let positions = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 10.0]];
        let tris = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]];
        let paint = vec!["4".to_string(), "8".into(), "0C".into(), "40C0C0C30C0C0C30C0C0C3".into()];
        let model = Model {
            name: "test & co",
            parts: vec![Part { name: "tet", positions: &positions, tris: &tris, paint: &paint }],
            filaments: vec!["#4d2720".into(), "#222222".into(), "#E8A461".into()],
            printer: Printer::A1,
        };
        let mut buf = Cursor::new(Vec::new());
        write(&mut buf, &model).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(buf.into_inner())).unwrap();
        let mut s = String::new();
        zip.by_name("3D/Objects/object_1.model").unwrap().read_to_string(&mut s).unwrap();
        assert!(s.contains("<vertex x=\"10\" y=\"0\" z=\"0\"/>"));
        assert!(s.contains("paint_color=\"0C\""));
        let mut cfg = String::new();
        zip.by_name("Metadata/project_settings.config").unwrap().read_to_string(&mut cfg).unwrap();
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert_eq!(v["filament_colour"][0], "#4D2720FF");
        assert_eq!(v["flush_volumes_matrix"].as_array().unwrap().len(), 9);
        let mut main = String::new();
        zip.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut main).unwrap();
        assert!(main.contains("test &amp; co") && main.contains("objectid=\"2\""));
    }
}

/// One colour of a model, as a closed mesh in millimetres.
pub struct ColorFile<'a> {
    /// `#RRGGBB`.
    pub hex: &'a str,
    pub positions: &'a [[f32; 3]],
    pub tris: &'a [[u32; 3]],
    /// Whole part rather than a shell.
    pub solid: bool,
}

/// Binary STL with facet normals.
pub fn write_stl<W: Write>(out: &mut W, name: &str, positions: &[[f32; 3]], tris: &[[u32; 3]]) -> std::io::Result<()> {
    let mut header = [b' '; 80];
    for (h, c) in header.iter_mut().zip(name.bytes().filter(u8::is_ascii)) {
        *h = c;
    }
    out.write_all(&header)?;
    out.write_all(&(tris.len() as u32).to_le_bytes())?;
    let mut buf = Vec::with_capacity(tris.len() * 50);
    for t in tris {
        let [a, b, c] = t.map(|v| positions[v as usize]);
        let u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
        let v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
        let n = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
        let l = (n[0] * n[0] + n[1] * n[1] + n[2] * n[2]).sqrt().max(f32::MIN_POSITIVE);
        for f in [n[0] / l, n[1] / l, n[2] / l].into_iter().chain(a).chain(b).chain(c) {
            buf.extend(f.to_le_bytes());
        }
        buf.extend([0u8, 0u8]);
    }
    out.write_all(&buf)
}

/// A ZIP with one STL per colour plus `colores.txt` listing slot, colour and kind.
pub fn write_color_zip<W: Write + Seek>(out: W, model_name: &str, files: &[ColorFile]) -> zip::result::ZipResult<()> {
    let mut z = zip::ZipWriter::new(out);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let mut index = format!("{model_name}\n\nRanura  Color    Archivo\n");
    for (i, f) in files.iter().enumerate() {
        let file = format!("{:02}_{}_{}.stl", i + 1, model_name, f.hex.trim_start_matches('#'));
        let _ = writeln!(index, "{:<7} {}  {}{}", i + 1, f.hex, file, if f.solid { "  (sólido completo)" } else { "" });
        z.start_file(&file, opts)?;
        let mut stl = Vec::new();
        write_stl(&mut stl, &file, f.positions, f.tris)?;
        z.write_all(&stl)?;
    }
    index.push_str(
        "\nCada color es una pieza cerrada. Las cáscaras siguen la superficie del modelo hacia dentro;\n\
         el sólido completo, si lo hay, es el modelo entero en el color principal.\n",
    );
    z.start_file("colores.txt", opts)?;
    z.write_all(index.as_bytes())?;
    z.finish()?;
    Ok(())
}

#[cfg(test)]
mod color_zip_tests {
    use super::*;
    use std::io::{Cursor, Read};

    #[test]
    fn zip_has_one_stl_per_colour() {
        let positions = [[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let tris = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]];
        let files = [
            ColorFile { hex: "#4D2720", positions: &positions, tris: &tris, solid: true },
            ColorFile { hex: "#E9A462", positions: &positions, tris: &tris[..2], solid: false },
        ];
        let mut buf = Cursor::new(Vec::new());
        write_color_zip(&mut buf, "spider", &files).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(buf.into_inner())).unwrap();
        assert_eq!(zip.len(), 3);
        let mut stl = Vec::new();
        zip.by_name("02_spider_E9A462.stl").unwrap().read_to_end(&mut stl).unwrap();
        assert_eq!(stl.len(), 84 + 2 * 50);
        assert_eq!(u32::from_le_bytes(stl[80..84].try_into().unwrap()), 2);
    }
}
