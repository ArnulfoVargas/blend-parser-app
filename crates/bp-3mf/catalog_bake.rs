//! Offline build-time conversion of the versioned catalogue into Rust constants.
use std::collections::HashSet;
use std::fmt::Write;

use serde::Deserialize;

#[derive(Deserialize)]
struct Catalog {
    source: String,
    printers: Vec<Printer>,
}

#[derive(Deserialize)]
struct Printer {
    id: String,
    vendor: String,
    name: String,
    area: [f32; 4],
    height: f32,
    nozzles: Vec<Nozzle>,
}

#[derive(Deserialize)]
struct Nozzle {
    diameter: f32,
    printer: String,
    process: Option<String>,
    layer: Option<f32>,
    filament: Option<String>,
}

pub fn bake(input: &str) -> Result<String, String> {
    let catalog: Catalog = serde_json::from_str(input).map_err(|e| format!("catalog.json: {e}"))?;
    let Some(commit) = catalog.source.strip_prefix("bambulab/BambuStudio@") else {
        return Err("catalogue source must identify a Bambu Studio commit".into());
    };
    if commit.len() != 40 || !commit.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err("catalogue source must contain a full commit SHA".into());
    }
    let mut ids = HashSet::new();
    let mut output =
        format!("// Generated offline from {}. Do not edit.\nconst PRINTERS: &[CatalogPrinter] = &[\n", catalog.source);
    for p in catalog.printers {
        if p.id.trim().is_empty() || p.vendor.trim().is_empty() || p.name.trim().is_empty() || !ids.insert(p.id.clone())
        {
            return Err(format!("empty or duplicate printer identity: {:?}", p.id));
        }
        if !p.area.iter().all(|v| v.is_finite())
            || !p.height.is_finite()
            || p.height <= 0.0
            || !(p.area[2] - p.area[0]).is_finite()
            || p.area[2] <= p.area[0]
            || !(p.area[3] - p.area[1]).is_finite()
            || p.area[3] <= p.area[1]
            || p.nozzles.is_empty()
        {
            return Err(format!("{}: invalid print volume or missing nozzles", p.id));
        }
        writeln!(
            output,
            "CatalogPrinter {{ id: {:?}, vendor: {:?}, name: {:?}, area: {:?}, height: {:?}, nozzles: &[",
            p.id, p.vendor, p.name, p.area, p.height
        )
        .unwrap();
        let mut diameters = HashSet::new();
        for n in p.nozzles {
            if !n.diameter.is_finite()
                || n.diameter <= 0.0
                || !diameters.insert(n.diameter.to_bits())
                || n.printer.trim().is_empty()
                || n.layer.is_some_and(|v| !v.is_finite() || v <= 0.0 || v > n.diameter)
                || n.process.as_ref().is_some_and(|s| s.trim().is_empty())
                || n.filament.as_ref().is_some_and(|s| s.trim().is_empty())
            {
                return Err(format!("{}: invalid nozzle or preset", p.id));
            }
            // Rust's Debug escapes strings and emits valid Rust float/Option literals.
            writeln!(
                output,
                "CatalogNozzle {{ diameter: {:?}, printer: {:?}, process: {:?}, layer: {:?}, filament: {:?} }},",
                n.diameter, n.printer, n.process, n.layer, n.filament
            )
            .unwrap();
        }
        output.push_str("] },\n");
    }
    if !ids.contains("Bambu Lab A1") {
        return Err("catalogue is missing the default printer Bambu Lab A1".into());
    }
    output.push_str("];\n");
    Ok(output)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn sample() -> Value {
        json!({
            "source": "bambulab/BambuStudio@79df7ae309be398ed30c9f397c13c9911c2edc57",
            "printers": [{
                "id": "Bambu Lab A1", "vendor": "Bambu Lab", "name": "Bambu Lab A1",
                "area": [0, 0, 256, 256], "height": 256,
                "nozzles": [{"diameter": 0.4, "printer": "A1 0.4 nozzle", "process": null, "layer": null, "filament": null}]
            }]
        })
    }

    #[test]
    fn rejects_duplicate_printers_and_nozzles() {
        let mut input = sample();
        let copy = input["printers"][0].clone();
        input["printers"].as_array_mut().unwrap().push(copy);
        assert!(bake(&input.to_string()).unwrap_err().contains("duplicate"));
        let mut input = sample();
        let copy = input["printers"][0]["nozzles"][0].clone();
        input["printers"][0]["nozzles"].as_array_mut().unwrap().push(copy);
        assert!(bake(&input.to_string()).is_err());
    }

    #[test]
    fn rejects_invalid_geometry_and_layer_heights() {
        let mut input = sample();
        input["printers"][0]["area"] = json!([0, 0, 0, 256]);
        assert!(bake(&input.to_string()).is_err());
        let mut input = sample();
        input["printers"][0]["nozzles"][0]["layer"] = json!(0.8);
        assert!(bake(&input.to_string()).is_err());
    }

    #[test]
    fn requires_a_pinned_source_and_default_printer() {
        let mut input = sample();
        input["source"] = json!("bambulab/BambuStudio@master");
        assert!(bake(&input.to_string()).is_err());
        let mut input = sample();
        input["printers"] = json!([]);
        assert!(bake(&input.to_string()).is_err());
    }

    #[test]
    fn generation_is_deterministic_and_escapes_preset_names() {
        let mut input = sample();
        input["printers"][0]["nozzles"][0]["printer"] = json!("Preset \"quoted\"\nwith \\ path");
        let baked = bake(&input.to_string()).unwrap();
        assert_eq!(baked, bake(&input.to_string()).unwrap());
        assert!(baked.contains(r#"Preset \"quoted\"\nwith \\ path"#));
    }
}
