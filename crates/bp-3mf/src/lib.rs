//! Writes a 3MF that Bambu Studio opens already painted: one object per printed
//! piece, one part per mesh, millimetre vertices and a `paint_color` per
//! triangle, plus a project config with one filament per colour. Also writes the
//! colour kit: loose pieces on single-colour plates, STLs and an assembly guide.

use std::fmt::Write as _;
use std::io::{Seek, Write};

use zip::write::SimpleFileOptions;

pub mod filament;
pub mod kit;
pub mod plate;
mod printer;
pub mod project_file;
pub use kit::{kit_bed, write_kit_zip, KitInfo, KitSummary, Lang as KitLang};
pub use plate::{PlateCheck, Rect, Reserve};
pub use printer::{canonical_id, catalog, Printer, PrinterProfile, NOZZLES};

pub struct Part<'a> {
    pub name: &'a str,
    pub positions: &'a [[f32; 3]],
    pub tris: &'a [[u32; 3]],
    /// Encoded `paint_color` per triangle.
    pub paint: &'a [String],
    /// The printed object it belongs to; parts of one object are printed joined.
    pub object: u32,
}

pub struct Model<'a> {
    pub name: &'a str,
    /// The word for a piece cut off the model, in the user's language (see [`object_name`]).
    pub piece: &'a str,
    pub parts: Vec<Part<'a>>,
    /// Filament colours as `#RRGGBB`, slot 1 first.
    pub filaments: Vec<String>,
    /// The filament on each slot, where the user picked one; the rest print as
    /// the printer's PLA. Shorter than `filaments` is fine.
    pub slots: Vec<Option<filament::SlotFilament>>,
    pub printer: Printer,
    pub flush: Flush,
    /// Layer height the user set; `None` keeps the process preset's.
    pub layer_mm: Option<f32>,
    /// Where the model's centre sits from the plate's centre, in millimetres.
    pub offset_mm: [f32; 2],
    /// Pictures of the plate for Bambu Studio's lists and the file browser.
    pub thumbnail: Option<Thumbnail<'a>>,
}

/// The plate as PNG pictures: Bambu Studio shows the large one in its plate
/// list and recent files, the small one where space is short.
#[derive(Clone, Copy)]
pub struct Thumbnail<'a> {
    pub large: &'a [u8],
    pub small: &'a [u8],
}

impl Thumbnail<'_> {
    /// Both are PNG files; anything else would leave Bambu Studio a broken picture.
    pub fn is_png(&self) -> bool {
        const MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";
        self.large.starts_with(MAGIC) && self.small.starts_with(MAGIC)
    }
}

const THUMB: &str = "Metadata/plate_1.png";
const THUMB_SMALL: &str = "Metadata/plate_1_small.png";

/// How the slicer flushes between filaments, so it matches the waste estimate.
#[derive(Clone, Debug, PartialEq)]
pub struct Flush {
    /// mm³ from filament `i` to `j` at `i * n + j`; empty for 280 everywhere.
    pub volumes_mm3: Vec<f32>,
    pub multiplier: f32,
    /// Flush into the model's infill as well as its supports.
    pub into_infill: bool,
    pub prime_mm3: f32,
}

impl Default for Flush {
    fn default() -> Self {
        Self { volumes_mm3: vec![], multiplier: 1.0, into_infill: false, prime_mm3: 45.0 }
    }
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
    let origins = origins(model);
    let _ = write!(
        x,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" {NS}>\n <metadata name=\"BambuStudio:3mfVersion\">2</metadata>\n <resources>\n"
    );
    for (i, p) in model.parts.iter().enumerate() {
        let id = i + 1;
        let _ = write!(x, "  <object id=\"{id}\" p:UUID=\"{id:04x}0000-81cb-4c03-9d28-80fed5dfa1dc\" type=\"model\">\n   <mesh>\n    <vertices>\n");
        let o = origins[i];
        for v in p.positions {
            x.push_str("     <vertex x=\"");
            num(&mut x, v[0] - o[0]);
            x.push_str("\" y=\"");
            num(&mut x, v[1] - o[1]);
            x.push_str("\" z=\"");
            num(&mut x, v[2] - o[2]);
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

/// The parts of each printed object, in object order; each is its own item on the plate.
fn objects(model: &Model) -> Vec<Vec<usize>> {
    let mut ids: Vec<u32> = model.parts.iter().map(|p| p.object).collect();
    ids.sort_unstable();
    ids.dedup();
    ids.iter().map(|&o| (0..model.parts.len()).filter(|&i| model.parts[i].object == o).collect()).collect()
}

/// Where each object sits: the centre of its footprint at its lowest point.
/// Its vertices are written from there and its plate item carries the rest,
/// so Bambu Studio turns, arranges and drops each piece about its own centre.
fn object_origin(model: &Model, parts: &[usize]) -> [f32; 3] {
    let mut it = parts.iter().flat_map(|&i| model.parts[i].positions.iter().copied());
    let Some(first) = it.next() else { return [0.0; 3] };
    let (lo, hi) = it.fold((first, first), |(lo, hi), p| {
        (std::array::from_fn(|i| lo[i].min(p[i])), std::array::from_fn::<f32, 3, _>(|i| hi[i].max(p[i])))
    });
    [(lo[0] + hi[0]) / 2.0, (lo[1] + hi[1]) / 2.0, lo[2]]
}

/// The origin of each part's object, by part.
fn origins(model: &Model) -> Vec<[f32; 3]> {
    let mut out = vec![[0.0; 3]; model.parts.len()];
    for parts in objects(model) {
        let o = object_origin(model, &parts);
        for i in parts {
            out[i] = o;
        }
    }
    out
}

/// The id of the `k`-th object: after the parts' own mesh objects.
fn object_id(model: &Model, k: usize) -> usize {
    model.parts.len() + 1 + k
}

/// The name of printed object `object`: the model's for the model itself and,
/// for each piece cut off it, the model's with `piece` and the object's number
/// ("Dragon – piece 2"). It follows the object, not its place in the file, so a
/// piece keeps its name when others are left out.
pub fn object_name(model: &str, piece: &str, object: u32) -> String {
    if object == 0 {
        model.to_string()
    } else {
        format!("{model} – {piece} {}", object + 1)
    }
}

fn main_xml(model: &Model) -> String {
    let (cx, cy) = model.printer.plate_centre();
    let (cx, cy) = (cx + model.offset_mm[0], cy + model.offset_mm[1]);
    let (mut resources, mut items) = (String::new(), String::new());
    for (k, parts) in objects(model).iter().enumerate() {
        let id = object_id(model, k);
        let o = object_origin(model, parts);
        let (x, y) = (cx + o[0], cy + o[1]);
        let mut comps = String::new();
        for &i in parts {
            let _ = writeln!(
                comps,
                "    <component p:path=\"/3D/Objects/object_1.model\" objectid=\"{}\" p:UUID=\"0001{i:04x}-b206-40ff-9872-83e8017abed1\" transform=\"1 0 0 0 1 0 0 0 1 0 0 0\"/>",
                i + 1
            );
        }
        let _ = write!(
            resources,
            "  <object id=\"{id}\" p:UUID=\"{:08x}-61cb-4c03-9d28-80fed5dfa1dc\" type=\"model\">
   <components>
{comps}   </components>
  </object>
",
            k + 1
        );
        let _ = write!(
            items,
            "  <item objectid=\"{id}\" p:UUID=\"{:08x}-b1ec-4553-aec9-835e5b724bb4\" transform=\"1 0 0 0 1 0 0 0 1 ",
            k + 2
        );
        num(&mut items, x);
        items.push(' ');
        num(&mut items, y);
        items.push(' ');
        num(&mut items, o[2]);
        items.push_str("\" printable=\"1\"/>\n");
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<model unit=\"millimeter\" xml:lang=\"en-US\" {NS}>
 <metadata name=\"Application\">BambuStudio-02.03.01.00</metadata>
 <metadata name=\"BambuStudio:3mfVersion\">2</metadata>
 <metadata name=\"BambuStudio:MmPaintingVersion\">1</metadata>
 <metadata name=\"Title\">{title}</metadata>
 <resources>
{resources} </resources>
 <build p:UUID=\"2c7c17d8-22b5-4d84-8835-1976022ea369\">
{items} </build>
</model>
",
        title = escape(model.name)
    )
}

fn settings_xml(model: &Model) -> String {
    let thumbnail = if model.thumbnail.is_some_and(|t| t.is_png()) {
        format!("    <metadata key=\"thumbnail_file\" value=\"{THUMB}\"/>\n")
    } else {
        String::new()
    };
    let (mut objects_cfg, mut instances) = (String::new(), String::new());
    for (k, ids) in objects(model).iter().enumerate() {
        let id = object_id(model, k);
        let total: usize = ids.iter().map(|&i| model.parts[i].tris.len()).sum();
        let mut parts = String::new();
        for &i in ids {
            let p = &model.parts[i];
            let _ = write!(
                parts,
                "    <part id=\"{}\" subtype=\"normal_part\">
      <metadata key=\"name\" value=\"{}\"/>
      <metadata key=\"matrix\" value=\"1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1\"/>
      <mesh_stat face_count=\"{}\" edges_fixed=\"0\" degenerate_facets=\"0\" facets_removed=\"0\" facets_reversed=\"0\" backwards_edges=\"0\"/>
    </part>
",
                i + 1,
                escape(p.name),
                p.tris.len()
            );
        }
        let _ = write!(
            objects_cfg,
            "  <object id=\"{id}\">
    <metadata key=\"name\" value=\"{name}\"/>
    <metadata key=\"extruder\" value=\"1\"/>
    <metadata face_count=\"{total}\"/>
{parts}  </object>
",
            name = escape(&object_name(model.name, model.piece, model.parts[ids[0]].object))
        );
        let _ = write!(
            instances,
            "    <model_instance>
      <metadata key=\"object_id\" value=\"{id}\"/>
      <metadata key=\"instance_id\" value=\"0\"/>
      <metadata key=\"identify_id\" value=\"{}\"/>
    </model_instance>
",
            k + 1
        );
    }
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>
<config>
{objects_cfg}  <plate>
    <metadata key=\"plater_id\" value=\"1\"/>
    <metadata key=\"plater_name\" value=\"\"/>
    <metadata key=\"locked\" value=\"false\"/>
{thumbnail}{instances}  </plate>
  <assemble>
  </assemble>
</config>
"
    )
}

fn project_json(model: &Model) -> String {
    let printer = &model.printer;
    let nozzle = format!("{}", printer.nozzle_mm);
    // Bambu Studio's own presets when it knows the printer; otherwise the
    // volume and nozzle themselves, so any slicer that reads the config sees them.
    let (printer_model, printer_id, process, filament) = match printer.presets() {
        Some(p) => (p.printer_model.to_string(), p.printer_settings_id, p.process.to_string(), p.filament.to_string()),
        None => (String::new(), printer.profile.name.clone(), String::new(), "Generic PLA".to_string()),
    };
    let n = model.filaments.len().max(1);
    let rep = |v: &str| vec![v.to_string(); n];
    let slot = |i: usize| model.slots.get(i).cloned().flatten();
    let settings_ids: Vec<String> =
        (0..n).map(|i| slot(i).and_then(|s| s.preset).unwrap_or_else(|| filament.clone())).collect();
    let types: Vec<String> = (0..n).map(|i| slot(i).map_or_else(|| "PLA".to_string(), |s| s.material)).collect();
    let (tower_x, tower_y) = printer.tower_origin();
    let f = &model.flush;
    let flush: Vec<String> = (0..n * n)
        .map(|i| match f.volumes_mm3.get(i) {
            _ if i / n == i % n => "0".to_string(),
            Some(v) => format!("{}", v.round()),
            None => "280".to_string(),
        })
        .collect();
    let mut cfg = serde_json::json!({
        "filament_colour": model.filaments.iter().map(|c| format!("{}FF", c.to_uppercase())).collect::<Vec<_>>(),
        "filament_settings_id": settings_ids,
        "filament_type": types,
        "filament_minimal_purge_on_wipe_tower": rep("15"),
        "filament_flow_ratio": rep("0.98"),
        "filament_diameter": rep("1.75"),
        "wipe": rep("1"),
        "wipe_distance": rep("2"),
        "retract_length_toolchange": rep("2"),
        "retract_restart_extra_toolchange": rep("0"),
        "standby_temperature_delta": "-5",
        "enable_prime_tower": if n > 1 { "1" } else { "0" },
        "prime_tower_width": "35",
        "prime_tower_brim_width": "3",
        "filament_prime_volume": rep(&format!("{}", f.prime_mm3.round())),
        "flush_into_infill": if f.into_infill { "1" } else { "0" },
        "flush_into_objects": "0",
        "flush_into_support": "1",
        "flush_multiplier": format!("{}", (f.multiplier * 100.0).round() / 100.0),
        "flush_volumes_matrix": flush,
        "flush_volumes_vector": vec!["140"; 2 * n],
        "wipe_tower_x": [format!("{tower_x}")],
        "wipe_tower_y": [format!("{tower_y}")],
        "wipe_tower_rotation_angle": "0",
        "wipe_tower_no_sparse_layers": "0",
        "wipe_speed": "80%",
        "single_extruder_multi_material": "1",
        "print_sequence": "by layer",
        "printer_model": printer_model,
        "printer_settings_id": printer_id,
        "nozzle_diameter": [nozzle],
        "print_settings_id": process,
        "layer_change_gcode": "; layer change\nG92 E0\n",
        "from": "project",
        "name": "project_settings",
        "version": "02.03.01.00"
    });
    if printer.profile.custom {
        let [w, d, h] = printer.profile.volume_mm;
        let obj = cfg.as_object_mut().expect("config is an object");
        obj.insert(
            "printable_area".into(),
            serde_json::json!(["0x0", format!("{w}x0"), format!("{w}x{d}"), format!("0x{d}")]),
        );
        obj.insert("printable_height".into(), serde_json::json!(format!("{h}")));
        obj.insert("layer_height".into(), serde_json::json!(format!("{}", printer.layer_mm())));
    }
    if let Some(l) = model.layer_mm {
        let obj = cfg.as_object_mut().expect("config is an object");
        obj.insert("layer_height".into(), serde_json::json!(format!("{}", (l * 1000.0).round() / 1000.0)));
    }
    serde_json::to_string_pretty(&cfg).unwrap_or_default()
}

const CONTENT_TYPES: &str = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Types xmlns=\"http://schemas.openxmlformats.org/package/2006/content-types\">\n <Default Extension=\"rels\" ContentType=\"application/vnd.openxmlformats-package.relationships+xml\"/>\n <Default Extension=\"model\" ContentType=\"application/vnd.ms-package.3dmanufacturing-3dmodel+xml\"/>\n <Default Extension=\"png\" ContentType=\"image/png\"/>\n</Types>\n";

fn rels(target: &str) -> String {
    rels_with(target, "")
}

/// Relationships to `target` and, with `extra`, more of them.
fn rels_with(target: &str, extra: &str) -> String {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Relationships xmlns=\"http://schemas.openxmlformats.org/package/2006/relationships\">\n <Relationship Target=\"{target}\" Id=\"rel-1\" Type=\"http://schemas.microsoft.com/3dmanufacturing/2013/01/3dmodel\"/>\n{extra}</Relationships>\n"
    )
}

/// The package's pictures as the 3MF core spec and Bambu Studio name them.
fn thumbnail_rels() -> String {
    format!(
        " <Relationship Target=\"/{THUMB}\" Id=\"rel-2\" Type=\"http://schemas.openxmlformats.org/package/2006/relationships/metadata/thumbnail\"/>\n <Relationship Target=\"/{THUMB}\" Id=\"rel-4\" Type=\"http://schemas.bambulab.com/package/2021/cover-thumbnail-middle\"/>\n <Relationship Target=\"/{THUMB_SMALL}\" Id=\"rel-5\" Type=\"http://schemas.bambulab.com/package/2021/cover-thumbnail-small\"/>\n"
    )
}

pub fn write<W: Write + Seek>(out: W, model: &Model) -> zip::result::ZipResult<()> {
    let mut z = zip::ZipWriter::new(out);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let thumb = model.thumbnail.filter(Thumbnail::is_png);
    let files: [(&str, String); 7] = [
        ("[Content_Types].xml", CONTENT_TYPES.to_string()),
        (
            "_rels/.rels",
            rels_with("/3D/3dmodel.model", &if thumb.is_some() { thumbnail_rels() } else { String::new() }),
        ),
        ("3D/3dmodel.model", main_xml(model)),
        ("3D/_rels/3dmodel.model.rels", rels("/3D/Objects/object_1.model")),
        ("3D/Objects/object_1.model", objects_xml(model)),
        ("Metadata/model_settings.config", settings_xml(model)),
        ("Metadata/project_settings.config", project_json(model)),
    ];
    for (name, body) in files {
        z.start_file(name, opts)?;
        z.write_all(body.as_bytes())?;
    }
    if let Some(t) = thumb {
        // PNG is compressed already.
        let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
        for (name, png) in [(THUMB, t.large), (THUMB_SMALL, t.small)] {
            z.start_file(name, stored)?;
            z.write_all(png)?;
        }
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
            piece: "piece",
            parts: vec![Part { name: "tet", positions: &positions, tris: &tris, paint: &paint, object: 0 }],
            filaments: vec!["#4d2720".into(), "#222222".into(), "#E8A461".into()],
            slots: vec![None, Some(filament::for_slot("PETG HF", &Printer::default()))],
            printer: Printer::default(),
            flush: Flush {
                volumes_mm3: (0..9).map(|i| i as f32 * 100.0).collect(),
                multiplier: 1.5,
                ..Default::default()
            },
            layer_mm: Some(0.16),
            offset_mm: [0.0; 2],
            thumbnail: None,
        };
        let mut buf = Cursor::new(Vec::new());
        write(&mut buf, &model).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(buf.into_inner())).unwrap();
        let mut s = String::new();
        zip.by_name("3D/Objects/object_1.model").unwrap().read_to_string(&mut s).unwrap();
        assert!(s.contains("<vertex x=\"-5\" y=\"-5\" z=\"0\"/>"));
        assert!(s.contains("paint_color=\"0C\""));
        let mut cfg = String::new();
        zip.by_name("Metadata/project_settings.config").unwrap().read_to_string(&mut cfg).unwrap();
        let v: serde_json::Value = serde_json::from_str(&cfg).unwrap();
        assert_eq!(v["filament_colour"][0], "#4D2720FF");
        assert_eq!(v["flush_volumes_matrix"].as_array().unwrap().len(), 9);
        assert_eq!(v["flush_volumes_matrix"][0], "0");
        assert_eq!(v["flush_volumes_matrix"][1], "100");
        assert_eq!(v["flush_volumes_matrix"][3], "300");
        assert_eq!(v["flush_multiplier"], "1.5");
        // Primed per filament under the name Bambu Studio 2 reads.
        assert_eq!(v["filament_prime_volume"], serde_json::json!(["45", "45", "45"]));
        assert!(v.get("prime_volume").is_none());
        assert_eq!(v["wipe_tower_x"][0], "15");
        assert_eq!(v["layer_height"], "0.16");
        let mut main = String::new();
        zip.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut main).unwrap();
        assert!(main.contains("test &amp; co") && main.contains("objectid=\"2\""));
        // The object's origin is the centre of its footprint at its base.
        assert!(main.contains("1 0 0 0 1 0 0 0 1 133 133 0"));
        assert!(s.contains("<vertex x=\"5\" y=\"-5\" z=\"0\"/>"), "{s}");
        assert_eq!(v["printer_settings_id"], "Bambu Lab A1 0.4 nozzle");
        // Slot 2 carries its own filament; the others print as the printer's PLA.
        assert_eq!(v["filament_type"], serde_json::json!(["PLA", "PETG", "PLA"]));
        assert_eq!(v["filament_settings_id"][1], "Bambu PETG HF @BBL A1");
        assert_eq!(v["filament_settings_id"][2], "Bambu PLA Basic @BBL A1");
        assert!(v.get("printable_area").is_none());
    }

    #[test]
    fn a_piece_cut_off_is_an_object_of_its_own() {
        let positions = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 10.0]];
        let tris = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]];
        let paint = vec!["4".to_string(); 4];
        let part = |name, object| Part { name, positions: &positions, tris: &tris, paint: &paint, object };
        let model = Model {
            name: "m",
            piece: "pieza",
            parts: vec![part("a", 0), part("b (2)", 1), part("c", 0)],
            filaments: vec!["#FFFFFF".into()],
            slots: vec![],
            printer: Printer::default(),
            flush: Flush::default(),
            layer_mm: None,
            offset_mm: [0.0; 2],
            thumbnail: None,
        };
        let main = main_xml(&model);
        assert_eq!(main.matches("<item ").count(), 2);
        assert!(main.contains("objectid=\"4\"") && main.contains("objectid=\"5\""));
        let cfg = settings_xml(&model);
        assert_eq!(cfg.matches("<model_instance>").count(), 2);
        assert!(cfg.contains("value=\"b (2)\""));
        assert!(cfg.contains("<metadata key=\"name\" value=\"m – pieza 2\"/>"), "{cfg}");
        // The model's object keeps both of its parts.
        let first = &cfg[..cfg.find("<object id=\"5\"").unwrap()];
        assert_eq!(first.matches("<part ").count(), 2);
    }

    #[test]
    fn each_object_sits_on_its_own_origin() {
        let left = [[-30.0, -5.0, 0.0], [-10.0, -5.0, 0.0], [-20.0, 5.0, 0.0], [-20.0, 0.0, 8.0]];
        let right = [[10.0, -2.0, 0.0], [30.0, -2.0, 0.0], [20.0, 2.0, 0.0], [20.0, 0.0, 4.0]];
        let tris = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]];
        let paint = vec!["4".to_string(); 4];
        let model = Model {
            name: "m",
            piece: "piece",
            parts: vec![
                Part { name: "a", positions: &left, tris: &tris, paint: &paint, object: 0 },
                Part { name: "b", positions: &right, tris: &tris, paint: &paint, object: 1 },
            ],
            filaments: vec!["#FFFFFF".into()],
            slots: vec![],
            printer: Printer::default(),
            flush: Flush::default(),
            layer_mm: None,
            offset_mm: [2.0, 0.0],
            thumbnail: None,
        };
        let main = main_xml(&model);
        // The plate's centre, the model's offset and each object's own centre.
        assert!(main.contains("transform=\"1 0 0 0 1 0 0 0 1 110 128 0\""), "{main}");
        assert!(main.contains("transform=\"1 0 0 0 1 0 0 0 1 150 128 0\""), "{main}");
        let objects = objects_xml(&model);
        assert!(objects.contains("<vertex x=\"-10\" y=\"-5\" z=\"0\"/>"), "{objects}");
        assert!(objects.contains("<vertex x=\"10\" y=\"-2\" z=\"0\"/>"), "{objects}");
        assert_eq!(object_name("m", "piece", 1), "m – piece 2");
    }

    #[test]
    fn thumbnails_go_in_when_they_are_png() {
        let positions = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 10.0]];
        let tris = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]];
        let paint = vec!["4".to_string(); 4];
        let png = b"\x89PNG\r\n\x1a\nlarge".to_vec();
        let small = b"\x89PNG\r\n\x1a\nsmall".to_vec();
        let package = |thumbnail| {
            let model = Model {
                name: "m",
                piece: "piece",
                parts: vec![Part { name: "a", positions: &positions, tris: &tris, paint: &paint, object: 0 }],
                filaments: vec!["#FFFFFF".into()],
                slots: vec![],
                printer: Printer::default(),
                flush: Flush::default(),
                layer_mm: None,
                offset_mm: [0.0; 2],
                thumbnail,
            };
            let mut buf = Cursor::new(Vec::new());
            write(&mut buf, &model).unwrap();
            zip::ZipArchive::new(Cursor::new(buf.into_inner())).unwrap()
        };
        let read = |zip: &mut zip::ZipArchive<Cursor<Vec<u8>>>, name: &str| {
            let mut out = Vec::new();
            zip.by_name(name).unwrap().read_to_end(&mut out).unwrap();
            out
        };
        let mut zip = package(Some(Thumbnail { large: &png, small: &small }));
        assert_eq!(read(&mut zip, "Metadata/plate_1.png"), png);
        assert_eq!(read(&mut zip, "Metadata/plate_1_small.png"), small);
        let rels = String::from_utf8(read(&mut zip, "_rels/.rels")).unwrap();
        assert!(rels.contains("cover-thumbnail-middle") && rels.contains("metadata/thumbnail"), "{rels}");
        let cfg = String::from_utf8(read(&mut zip, "Metadata/model_settings.config")).unwrap();
        assert!(cfg.contains("key=\"thumbnail_file\" value=\"Metadata/plate_1.png\""));
        let types = String::from_utf8(read(&mut zip, "[Content_Types].xml")).unwrap();
        assert!(types.contains("image/png"));

        // Not a PNG: the file goes out without pictures rather than with broken ones.
        let mut zip = package(Some(Thumbnail { large: b"nope", small: &small }));
        assert!(zip.by_name("Metadata/plate_1.png").is_err());
        let rels = String::from_utf8(read(&mut zip, "_rels/.rels")).unwrap();
        assert!(!rels.contains("thumbnail"));
        let cfg = String::from_utf8(read(&mut zip, "Metadata/model_settings.config")).unwrap();
        assert!(!cfg.contains("thumbnail_file"));
    }

    fn config(printer: Printer) -> serde_json::Value {
        let model = Model {
            name: "t",
            piece: "piece",
            parts: vec![],
            filaments: vec!["#FFFFFF".into()],
            slots: vec![],
            printer,
            flush: Flush::default(),
            layer_mm: None,
            offset_mm: [0.0; 2],
            thumbnail: None,
        };
        serde_json::from_str(&project_json(&model)).unwrap()
    }

    #[test]
    fn names_the_presets_for_the_nozzle() {
        let x1c = catalog().into_iter().find(|p| p.id == "Bambu Lab X1 Carbon").unwrap();
        let v = config(Printer::new(x1c, 0.6));
        assert_eq!(v["printer_model"], "Bambu Lab X1 Carbon");
        assert_eq!(v["printer_settings_id"], "Bambu Lab X1 Carbon 0.6 nozzle");
        assert_eq!(v["print_settings_id"], "0.30mm Standard @BBL X1C 0.6 nozzle");
        assert_eq!(v["filament_settings_id"][0], "Bambu PLA Basic @BBL X1C 0.6 nozzle");
        assert_eq!(v["nozzle_diameter"][0], "0.6");
    }

    #[test]
    fn custom_printer_carries_its_volume() {
        let p = PrinterProfile {
            id: "custom-1".into(),
            vendor: String::new(),
            name: "Ender 3".into(),
            volume_mm: [220.0, 120.0, 250.0],
            nozzles: vec![0.4, 0.6],
            base: None,
            custom: true,
        };
        let v = config(Printer::new(p, 0.6));
        assert_eq!(v["printer_settings_id"], "Ender 3");
        assert_eq!(v["printable_area"][2], "220x120");
        assert_eq!(v["printable_height"], "250");
        assert_eq!(v["layer_height"], "0.3");
        assert_eq!(v["wipe_tower_y"][0], "80");
    }
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

/// The words of the colour list inside the ZIP, in the user's language.
pub struct ZipText {
    pub index_file: &'static str,
    /// Column heads: slot, colour, file.
    pub columns: [&'static str; 3],
    pub solid: &'static str,
    pub footer: &'static str,
    /// A piece cut off the model, as its 3MF object is named.
    pub piece: &'static str,
}

impl ZipText {
    pub const ES: ZipText = ZipText {
        index_file: "colores.txt",
        columns: ["Ranura", "Color", "Archivo"],
        solid: "(sólido completo)",
        footer: "Cada color es una pieza cerrada. Las cáscaras siguen la superficie del modelo hacia dentro;\n\
                 el sólido completo, si lo hay, es el modelo entero en el color principal.\n",
        piece: "pieza",
    };
    pub const EN: ZipText = ZipText {
        index_file: "colors.txt",
        columns: ["Slot", "Colour", "File"],
        solid: "(full solid)",
        footer: "Each colour is a closed piece. Shells follow the model's surface inwards;\n\
                 the full solid, if there is one, is the whole model in the main colour.\n",
        piece: "piece",
    };

    /// The text for a BCP 47 language tag; Spanish when there is none for it.
    pub fn for_lang(lang: &str) -> &'static ZipText {
        if lang.to_ascii_lowercase().starts_with("en") {
            &Self::EN
        } else {
            &Self::ES
        }
    }
}

/// One object of a multi-plate project, already turned for printing.
pub struct PlateObject<'a> {
    pub name: &'a str,
    /// Millimetres, centred on XY with the base on Z = 0.
    pub positions: &'a [[f32; 3]],
    pub tris: &'a [[u32; 3]],
    /// 1-based filament.
    pub filament: usize,
    /// 0-based plate.
    pub plate: usize,
    /// Centre on the plate, in plate millimetres.
    pub at: [f32; 2],
}

pub struct PlateProject<'a> {
    pub name: &'a str,
    /// Flushes between filaments, for plates where colours mix.
    pub flush: Flush,
    pub objects: Vec<PlateObject<'a>>,
    /// Name shown on each plate.
    pub plates: Vec<String>,
    pub filaments: Vec<String>,
    /// The filament on each slot, where the user picked one (see [`Model::slots`]).
    pub slots: Vec<Option<filament::SlotFilament>>,
    pub printer: Printer,
    /// Each plate's picture, large and small PNG, in plate order; may be empty.
    pub thumbnails: Vec<(Vec<u8>, Vec<u8>)>,
}

/// Where Bambu Studio puts plate `i` of `count`: a grid `ceil(sqrt(count))`
/// wide, a fifth of a plate apart, later rows toward -Y.
pub fn plate_origin(printer: &Printer, i: usize, count: usize) -> (f32, f32) {
    let cols = (count.max(1) as f32).sqrt().ceil() as usize;
    let (w, d) = printer.bed_mm();
    ((i % cols) as f32 * w * 1.2, -((i / cols) as f32) * d * 1.2)
}

/// The project config of a plates 3MF: its filaments, printer and flushes,
/// Bambu's defaults for the rest. A plate of one filament flushes nothing.
fn plates_model<'a>(project: &'a PlateProject) -> Model<'a> {
    Model {
        name: project.name,
        piece: "",
        parts: vec![],
        filaments: project.filaments.clone(),
        slots: project.slots.clone(),
        printer: project.printer.clone(),
        flush: project.flush.clone(),
        layer_mm: None,
        offset_mm: [0.0; 2],
        thumbnail: None,
    }
}

/// A 3MF with every object on its plate and its filament, nothing painted.
pub fn write_plates<W: Write + Seek>(out: W, project: &PlateProject) -> zip::result::ZipResult<()> {
    let n = project.objects.len();
    let tris: usize = project.objects.iter().map(|o| o.tris.len()).sum();

    let mut meshes = String::with_capacity(tris * 48);
    let _ = write!(
        meshes,
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" {NS}>\n <metadata name=\"BambuStudio:3mfVersion\">2</metadata>\n <resources>\n"
    );
    for (i, o) in project.objects.iter().enumerate() {
        let id = i + 1;
        let _ = write!(
            meshes,
            "  <object id=\"{id}\" p:UUID=\"{id:08x}-81cb-4c03-9d28-80fed5dfa1dc\" type=\"model\">\n   <mesh>\n    <vertices>\n"
        );
        for v in o.positions {
            meshes.push_str("     <vertex x=\"");
            num(&mut meshes, v[0]);
            meshes.push_str("\" y=\"");
            num(&mut meshes, v[1]);
            meshes.push_str("\" z=\"");
            num(&mut meshes, v[2]);
            meshes.push_str("\"/>\n");
        }
        meshes.push_str("    </vertices>\n    <triangles>\n");
        for t in o.tris {
            let _ = writeln!(meshes, "     <triangle v1=\"{}\" v2=\"{}\" v3=\"{}\"/>", t[0], t[1], t[2]);
        }
        meshes.push_str("    </triangles>\n   </mesh>\n  </object>\n");
    }
    meshes.push_str(" </resources>\n <build/>\n</model>\n");

    let (mut objects, mut items) = (String::new(), String::new());
    for (i, o) in project.objects.iter().enumerate() {
        let (mesh, wrapper) = (i + 1, n + i + 1);
        let (ox, oy) = plate_origin(&project.printer, o.plate, project.plates.len());
        let _ = write!(
            objects,
            "  <object id=\"{wrapper}\" p:UUID=\"{wrapper:08x}-61cb-4c03-9d28-80fed5dfa1dc\" type=\"model\">\n   <components>\n    \
<component p:path=\"/3D/Objects/object_1.model\" objectid=\"{mesh}\" p:UUID=\"{mesh:08x}-b206-40ff-9872-83e8017abed1\" transform=\"1 0 0 0 1 0 0 0 1 0 0 0\"/>\n   </components>\n  </object>\n"
        );
        let mut t = String::new();
        num(&mut t, ox + o.at[0]);
        t.push(' ');
        num(&mut t, oy + o.at[1]);
        let _ = writeln!(
            items,
            "  <item objectid=\"{wrapper}\" p:UUID=\"{wrapper:08x}-b1ec-4553-aec9-835e5b724bb4\" transform=\"1 0 0 0 1 0 0 0 1 {t} 0\" printable=\"1\"/>"
        );
    }
    let main = format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<model unit=\"millimeter\" xml:lang=\"en-US\" {NS}>\n \
<metadata name=\"Application\">BambuStudio-02.03.01.00</metadata>\n \
<metadata name=\"BambuStudio:3mfVersion\">2</metadata>\n \
<metadata name=\"Title\">{title}</metadata>\n <resources>\n{objects} </resources>\n \
<build p:UUID=\"2c7c17d8-22b5-4d84-8835-1976022ea369\">\n{items} </build>\n</model>\n",
        title = escape(project.name)
    );

    // Pictures that are PNG files; a plate without one has none.
    let thumbs: Vec<&(Vec<u8>, Vec<u8>)> =
        project.thumbnails.iter().take_while(|t| Thumbnail { large: &t.0, small: &t.1 }.is_png()).collect();
    let mut settings = String::from("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<config>\n");
    for (i, o) in project.objects.iter().enumerate() {
        let (mesh, wrapper) = (i + 1, n + i + 1);
        let _ = write!(
            settings,
            "  <object id=\"{wrapper}\">\n    <metadata key=\"name\" value=\"{name}\"/>\n    \
<metadata key=\"extruder\" value=\"{f}\"/>\n    <metadata face_count=\"{faces}\"/>\n    \
<part id=\"{mesh}\" subtype=\"normal_part\">\n      <metadata key=\"name\" value=\"{name}\"/>\n      \
<metadata key=\"matrix\" value=\"1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1\"/>\n      \
<metadata key=\"extruder\" value=\"{f}\"/>\n      \
<mesh_stat face_count=\"{faces}\" edges_fixed=\"0\" degenerate_facets=\"0\" facets_removed=\"0\" facets_reversed=\"0\" backwards_edges=\"0\"/>\n    </part>\n  </object>\n",
            name = escape(o.name),
            f = o.filament,
            faces = o.tris.len()
        );
    }
    for (k, plate_name) in project.plates.iter().enumerate() {
        let _ = write!(
            settings,
            "  <plate>\n    <metadata key=\"plater_id\" value=\"{}\"/>\n    <metadata key=\"plater_name\" value=\"{}\"/>\n    \
<metadata key=\"locked\" value=\"false\"/>\n",
            k + 1,
            escape(plate_name)
        );
        if thumbs.get(k).is_some() {
            let _ = writeln!(settings, "    <metadata key=\"thumbnail_file\" value=\"Metadata/plate_{}.png\"/>", k + 1);
        }
        for (i, _) in project.objects.iter().enumerate().filter(|(_, o)| o.plate == k) {
            let _ = write!(
                settings,
                "    <model_instance>\n      <metadata key=\"object_id\" value=\"{}\"/>\n      \
<metadata key=\"instance_id\" value=\"0\"/>\n      <metadata key=\"identify_id\" value=\"{}\"/>\n    </model_instance>\n",
                n + i + 1,
                i + 1
            );
        }
        settings.push_str("  </plate>\n");
    }
    settings.push_str("  <assemble>\n  </assemble>\n</config>\n");

    let mut z = zip::ZipWriter::new(out);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let files: [(&str, String); 7] = [
        ("[Content_Types].xml", CONTENT_TYPES.to_string()),
        (
            "_rels/.rels",
            rels_with("/3D/3dmodel.model", &if thumbs.is_empty() { String::new() } else { thumbnail_rels() }),
        ),
        ("3D/3dmodel.model", main),
        ("3D/_rels/3dmodel.model.rels", rels("/3D/Objects/object_1.model")),
        ("3D/Objects/object_1.model", meshes),
        ("Metadata/model_settings.config", settings),
        ("Metadata/project_settings.config", project_json(&plates_model(project))),
    ];
    for (name, body) in files {
        z.start_file(name, opts)?;
        z.write_all(body.as_bytes())?;
    }
    // Plate 1's picture is the package's cover, as in a painted 3MF.
    let stored = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored);
    for (k, t) in thumbs.iter().enumerate() {
        for (name, png) in
            [(format!("Metadata/plate_{}.png", k + 1), &t.0), (format!("Metadata/plate_{}_small.png", k + 1), &t.1)]
        {
            z.start_file(name, stored)?;
            z.write_all(png)?;
        }
    }
    z.finish()?;
    Ok(())
}

#[cfg(test)]
mod plate_tests {
    use super::*;
    use std::io::{Cursor, Read};

    #[test]
    fn objects_land_on_their_plates_with_their_filament() {
        let positions = [[0.0, 0.0, 0.0], [10.0, 0.0, 0.0], [0.0, 10.0, 0.0], [0.0, 0.0, 10.0]];
        let tris = [[0, 2, 1], [0, 1, 3], [1, 2, 3], [0, 3, 2]];
        let object = |name, filament, plate| PlateObject {
            name,
            positions: &positions,
            tris: &tris,
            filament,
            plate,
            at: [50.0, 60.0],
        };
        let project = PlateProject {
            flush: Flush::default(),
            name: "kit",
            objects: vec![object("P01", 1, 0), object("P02", 2, 1)],
            plates: vec!["Negro".into(), "Blanco".into()],
            filaments: vec!["#202020".into(), "#F5F4F0".into()],
            slots: vec![],
            printer: Printer::default(),
            thumbnails: vec![(b"\x89PNG\r\n\x1a\n1".to_vec(), b"\x89PNG\r\n\x1a\n1s".to_vec())],
        };
        let mut buf = Cursor::new(Vec::new());
        write_plates(&mut buf, &project).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(buf.into_inner())).unwrap();
        let mut main = String::new();
        zip.by_name("3D/3dmodel.model").unwrap().read_to_string(&mut main).unwrap();
        // Plate 2 of 2 sits one plate width and a fifth to the right.
        assert!(main.contains("transform=\"1 0 0 0 1 0 0 0 1 50 60 0\""));
        assert!(main.contains("transform=\"1 0 0 0 1 0 0 0 1 357.2 60 0\""), "{main}");
        let mut cfg = String::new();
        zip.by_name("Metadata/model_settings.config").unwrap().read_to_string(&mut cfg).unwrap();
        assert!(cfg.contains("<metadata key=\"plater_id\" value=\"2\"/>"));
        assert!(cfg.contains("<metadata key=\"extruder\" value=\"2\"/>"));
        // Plate 1 has its picture, plate 2 none.
        assert_eq!(cfg.matches("thumbnail_file").count(), 1);
        assert!(zip.by_name("Metadata/plate_1.png").is_ok() && zip.by_name("Metadata/plate_2.png").is_err());
    }
}
