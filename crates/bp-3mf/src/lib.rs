//! Writes a 3MF that Bambu Studio opens already painted: one object per printed
//! piece, one part per mesh, millimetre vertices and a `paint_color` per
//! triangle, plus a project config with one filament per colour.

use std::fmt::Write as _;
use std::io::{Seek, Write};

use zip::write::SimpleFileOptions;

pub mod filament;
pub mod plate;
mod printer;
pub mod project_file;
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
        Self { volumes_mm3: vec![], multiplier: 1.0, into_infill: false, prime_mm3: 20.0 }
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
        "prime_tower_width": "20",
        "prime_tower_brim_width": "3",
        "prime_volume": format!("{}", f.prime_mm3.round()),
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

/// A ZIP with one STL per colour plus a list of slot, colour and kind.
pub fn write_color_zip<W: Write + Seek>(
    out: W,
    model_name: &str,
    files: &[ColorFile],
    text: &ZipText,
) -> zip::result::ZipResult<()> {
    let mut z = zip::ZipWriter::new(out);
    let opts = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);
    let [slot, colour, file_head] = text.columns;
    let mut index = format!("{model_name}\n\n{slot:<7} {colour:<8} {file_head}\n");
    for (i, f) in files.iter().enumerate() {
        let file = format!("{:02}_{}_{}.stl", i + 1, model_name, f.hex.trim_start_matches('#'));
        let solid = if f.solid { format!("  {}", text.solid) } else { String::new() };
        let _ = writeln!(index, "{:<7} {}  {}{}", i + 1, f.hex, file, solid);
        z.start_file(&file, opts)?;
        let mut stl = Vec::new();
        write_stl(&mut stl, &file, f.positions, f.tris)?;
        z.write_all(&stl)?;
    }
    index.push('\n');
    index.push_str(text.footer);
    z.start_file(text.index_file, opts)?;
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
        write_color_zip(&mut buf, "spider", &files, ZipText::for_lang("en-GB")).unwrap();
        let mut zip = zip::ZipArchive::new(Cursor::new(buf.into_inner())).unwrap();
        assert_eq!(zip.len(), 3);
        let mut index = String::new();
        zip.by_name("colors.txt").unwrap().read_to_string(&mut index).unwrap();
        assert!(index.contains("Slot") && index.contains("(full solid)"), "{index}");
        let mut stl = Vec::new();
        zip.by_name("02_spider_E9A462.stl").unwrap().read_to_end(&mut stl).unwrap();
        assert_eq!(stl.len(), 84 + 2 * 50);
        assert_eq!(u32::from_le_bytes(stl[80..84].try_into().unwrap()), 2);
    }
}
