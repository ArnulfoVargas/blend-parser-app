//! The command line, read by hand: a few flags do not need a parser crate.

use std::path::PathBuf;

use bp_core::color::{parse_hex, Rgb};

pub const USAGE: &str = "\
blendparser-cli: models and BlendParser projects to Bambu Studio 3MF files

Usage:
  blendparser-cli convert <file or folder>... [options]
  blendparser-cli printers
  blendparser-cli --help

Inputs are .blend, .obj, .fbx, .gltf, .glb, .ply or .bpproj files, or folders
(their files, not subfolders). Models go through Blender; projects open as saved.

Options:
  -o, --output FILE      3MF to write (one input only)
      --out-dir DIR      folder for the 3MF files (default: next to each input)
      --colors N         detect N colours (1 to 16; default 4 for a model)
      --palette HEX,...  use these colours instead, e.g. \"#FFFFFF,#222222\"
      --printer ID       printer from `blendparser-cli printers` (default: the app's pick)
      --nozzle MM        nozzle diameter (default: the printer's pick)
      --size MM          longest side in millimetres, in proportion
      --size X,Y,Z       width, depth and height in millimetres
      --islands MM2      clean colour islands smaller than this
      --detail 0-6       border detail (default 5)
      --simplify PCT     keep this percentage of the triangles (5 to 100)
      --flatten MM       flatten the base by this much
      --min-piece MM     drop loose pieces smaller than this
      --blender PATH     Blender to use (default: BLENDER_PATH or the usual places)
      --config DIR       the app's settings folder (default: BlendParser's own)
      --lang es|en       language of the piece names in the 3MF (default: en)
      --json             print the report as JSON

Exit codes: 0 converted, 1 converted with warnings, 2 a file failed, 64 bad usage.
The waste settings and your printers come from the app's settings folder.";

pub enum Command {
    Convert,
    Printers,
    Help,
}

pub enum Size {
    Longest(f32),
    Axes([f32; 3]),
}

pub struct Args {
    pub command: Command,
    pub inputs: Vec<PathBuf>,
    pub output: Option<PathBuf>,
    pub out_dir: Option<PathBuf>,
    pub colors: Option<usize>,
    pub palette: Option<Vec<Rgb>>,
    pub printer: Option<String>,
    pub nozzle: Option<f32>,
    pub size: Option<Size>,
    pub islands: Option<f32>,
    pub detail: Option<u8>,
    pub keep: Option<f32>,
    pub flatten: Option<f32>,
    pub min_piece: Option<f32>,
    pub blender: Option<PathBuf>,
    pub config: Option<PathBuf>,
    pub lang: String,
    pub json: bool,
}

fn number(flag: &str, v: &str, range: std::ops::RangeInclusive<f32>) -> Result<f32, String> {
    v.trim()
        .parse::<f32>()
        .ok()
        .filter(|n| range.contains(n))
        .ok_or_else(|| format!("{flag} takes a number from {} to {}, not {v:?}", range.start(), range.end()))
}

pub fn parse(args: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut it = args.into_iter();
    let command = match it.next().as_deref() {
        None | Some("-h" | "--help" | "help") => Command::Help,
        Some("convert") => Command::Convert,
        Some("printers") => Command::Printers,
        Some(other) => return Err(format!("unknown command {other:?}")),
    };
    let mut a = Args {
        command,
        inputs: vec![],
        output: None,
        out_dir: None,
        colors: None,
        palette: None,
        printer: None,
        nozzle: None,
        size: None,
        islands: None,
        detail: None,
        keep: None,
        flatten: None,
        min_piece: None,
        blender: None,
        config: None,
        lang: "en".into(),
        json: false,
    };
    while let Some(arg) = it.next() {
        if !arg.starts_with('-') {
            a.inputs.push(arg.into());
            continue;
        }
        if matches!(arg.as_str(), "-h" | "--help") {
            a.command = Command::Help;
            continue;
        }
        if arg == "--json" {
            a.json = true;
            continue;
        }
        let v = it.next().ok_or_else(|| format!("{arg} needs a value"))?;
        match arg.as_str() {
            "-o" | "--output" => a.output = Some(v.into()),
            "--out-dir" => a.out_dir = Some(v.into()),
            "--colors" => a.colors = Some(number(&arg, &v, 1.0..=16.0)? as usize),
            "--palette" => {
                let colors: Vec<Rgb> = v
                    .split(',')
                    .map(|c| parse_hex(c).ok_or_else(|| format!("--palette: {c:?} is not a #RRGGBB colour")))
                    .collect::<Result<_, _>>()?;
                if !(1..=16).contains(&colors.len()) {
                    return Err("--palette takes 1 to 16 colours".into());
                }
                a.palette = Some(colors);
            }
            "--printer" => a.printer = Some(v),
            "--nozzle" => a.nozzle = Some(number(&arg, &v, 0.1..=2.0)?),
            "--size" => {
                let parts: Vec<&str> = v.split(',').collect();
                a.size = Some(match parts.as_slice() {
                    [one] => Size::Longest(number(&arg, one, 1.0..=1000.0)?),
                    [x, y, z] => Size::Axes([
                        number(&arg, x, 1.0..=1000.0)?,
                        number(&arg, y, 1.0..=1000.0)?,
                        number(&arg, z, 1.0..=1000.0)?,
                    ]),
                    _ => return Err("--size takes MM or X,Y,Z".into()),
                });
            }
            "--islands" => a.islands = Some(number(&arg, &v, 0.0..=1000.0)?),
            "--detail" => a.detail = Some(number(&arg, &v, 0.0..=6.0)? as u8),
            "--simplify" => a.keep = Some(number(&arg, &v, 5.0..=100.0)? / 100.0),
            "--flatten" => a.flatten = Some(number(&arg, &v, 0.0..=50.0)?),
            "--min-piece" => a.min_piece = Some(number(&arg, &v, 0.0..=100.0)?),
            "--blender" => a.blender = Some(v.into()),
            "--config" => a.config = Some(v.into()),
            "--lang" => a.lang = v,
            _ => return Err(format!("unknown option {arg}")),
        }
    }
    if a.palette.is_some() && a.colors.is_some() {
        return Err("use --colors or --palette, not both".into());
    }
    Ok(a)
}

/// Files as given, and the models and projects directly inside each folder, in name order.
pub fn expand(inputs: &[PathBuf]) -> Result<Vec<PathBuf>, String> {
    let wanted =
        |p: &std::path::Path| bp_blend::supports(p) || p.extension().is_some_and(|e| e.eq_ignore_ascii_case("bpproj"));
    let mut out = vec![];
    for input in inputs {
        if input.is_dir() {
            let mut found: Vec<PathBuf> = std::fs::read_dir(input)
                .map_err(|e| format!("{}: {e}", input.display()))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .filter(|p| p.is_file() && wanted(p))
                .collect();
            found.sort();
            out.extend(found);
        } else if input.is_file() {
            if !wanted(input) {
                return Err(format!("{}: not a model or project BlendParser reads", input.display()));
            }
            out.push(input.clone());
        } else {
            return Err(format!("{}: no such file or folder", input.display()));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_str(s: &str) -> Result<Args, String> {
        parse(s.split_whitespace().map(String::from))
    }

    #[test]
    fn reads_the_options() {
        let a = parse_str("convert a.blend b.obj --colors 5 --size 80 --printer X1C --nozzle 0.6 --json --simplify 50")
            .unwrap();
        assert!(matches!(a.command, Command::Convert));
        assert_eq!(a.inputs.len(), 2);
        assert_eq!(a.colors, Some(5));
        assert!(matches!(a.size, Some(Size::Longest(v)) if v == 80.0));
        assert_eq!(a.keep, Some(0.5));
        assert!(a.json);
        let a = parse_str("convert a.blend --size 10,20,30 --palette #FFFFFF,#000000").unwrap();
        assert!(matches!(a.size, Some(Size::Axes([10.0, 20.0, 30.0]))));
        assert_eq!(a.palette.unwrap().len(), 2);
    }

    #[test]
    fn refuses_bad_values() {
        assert!(parse_str("convert a.blend --colors 40").is_err());
        assert!(parse_str("convert a.blend --size 1,2").is_err());
        assert!(parse_str("convert a.blend --palette red").is_err());
        assert!(parse_str("convert a.blend --colors 3 --palette #FFFFFF").is_err());
        assert!(parse_str("convert a.blend --wat 3").is_err());
        assert!(parse_str("convert a.blend --colors").is_err());
        assert!(parse_str("frobnicate").is_err());
    }
}
