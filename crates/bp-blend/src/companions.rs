//! Files a model points at but does not contain: the `.mtl` of an OBJ, the
//! `.bin` of a glTF, and the textures of OBJ, glTF and FBX. Read without
//! Blender, so the app can ask for what is missing before converting.
//!
//! A reference is found as written beside the file that names it, beside the
//! model, or among the files the user added (by name, ignoring case). The
//! extractor resolves references the same way.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::Serialize;

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Kind {
    /// OBJ material libraries: without them every face is grey.
    Materials,
    /// glTF geometry buffers: without them there is no mesh at all.
    Buffers,
    Textures,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Companion {
    pub kind: Kind,
    /// The name as the model writes it.
    pub name: String,
    pub found: bool,
    /// Conversion fails without it.
    pub required: bool,
}

const IMAGE_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "tga", "bmp", "tif", "tiff", "exr", "webp", "hdr"];

/// Folders stand for the files directly inside them and one level down, so
/// "the textures folder" works whether textures sit in it or in a subfolder.
pub fn expand(extras: &[PathBuf]) -> Vec<PathBuf> {
    fn files(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
        let Ok(entries) = std::fs::read_dir(dir) else { return };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_file() {
                out.push(path);
            } else if path.is_dir() && depth > 0 {
                files(&path, depth - 1, out);
            }
        }
    }
    let mut out = vec![];
    for path in extras {
        if path.is_dir() {
            files(path, 1, &mut out);
        } else if path.is_file() {
            out.push(path.clone());
        }
    }
    out
}

fn base_name(name: &str) -> String {
    name.rsplit(['/', '\\']).next().unwrap_or(name).to_lowercase()
}

/// Where `name`, written in a file in `base`, actually is.
pub fn locate(name: &str, base: &Path, model_dir: &Path, extras: &[PathBuf]) -> Option<PathBuf> {
    let name = name.trim().trim_matches('"');
    if name.is_empty() {
        return None;
    }
    let written = Path::new(name);
    let mut candidates = vec![base.join(written), model_dir.join(written)];
    let key = base_name(name);
    candidates.push(model_dir.join(&key));
    if let Some(found) = candidates.into_iter().find(|p| p.is_file()) {
        return Some(found);
    }
    extras.iter().find(|p| p.file_name().and_then(|n| n.to_str()).is_some_and(|n| n.to_lowercase() == key)).cloned()
}

/// What `model` references, each once, materials before buffers before textures.
pub fn inspect(model: &Path, extras: &[PathBuf]) -> Vec<Companion> {
    let extras = expand(extras);
    let dir = model.parent().unwrap_or(Path::new("."));
    let ext = model.extension().and_then(|e| e.to_str()).unwrap_or("").to_lowercase();
    let mut out = BTreeMap::new();
    let mut add = |kind: Kind, name: String, base: &Path, required: bool| {
        let found = locate(&name, base, dir, &extras);
        let entry = out.entry((kind, base_name(&name))).or_insert(Companion { kind, name, found: false, required });
        entry.found |= found.is_some();
        found
    };
    match ext.as_str() {
        "obj" => {
            for library in obj_libraries(model) {
                if let Some(mtl) = add(Kind::Materials, library, dir, false) {
                    let base = mtl.parent().unwrap_or(dir).to_path_buf();
                    for texture in mtl_textures(&mtl) {
                        add(Kind::Textures, texture, &base, false);
                    }
                }
            }
        }
        "gltf" => {
            let json =
                std::fs::read_to_string(model).ok().and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok());
            let uris = |key: &str| -> Vec<String> {
                json.as_ref()
                    .and_then(|j| j[key].as_array())
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|i| i["uri"].as_str())
                            .filter(|u| !u.starts_with("data:"))
                            .map(percent_decode)
                            .collect()
                    })
                    .unwrap_or_default()
            };
            for uri in uris("buffers") {
                add(Kind::Buffers, uri, dir, true);
            }
            for uri in uris("images") {
                add(Kind::Textures, uri, dir, false);
            }
        }
        "fbx" => {
            let bytes = std::fs::read(model).unwrap_or_default();
            // Embedded media comes out with the model; only external files can be missing.
            if !fbx_embeds_media(&bytes) {
                for texture in fbx_textures(&bytes) {
                    add(Kind::Textures, texture, dir, false);
                }
            }
        }
        _ => {}
    }
    out.into_values().collect()
}

/// `mtllib` names. A name with spaces is taken whole when that file exists.
pub fn obj_libraries(obj: &Path) -> Vec<String> {
    let dir = obj.parent().unwrap_or(Path::new("."));
    let text = String::from_utf8_lossy(&std::fs::read(obj).unwrap_or_default()).into_owned();
    let mut names = vec![];
    for line in text.lines() {
        let Some(rest) = directive(line, "mtllib") else { continue };
        if dir.join(rest).is_file() || !rest.contains(char::is_whitespace) {
            names.push(rest.to_string());
        } else {
            names.extend(rest.split_whitespace().map(str::to_string));
        }
    }
    names
}

/// Colour maps of an MTL, without the options that may precede the file name.
pub fn mtl_textures(mtl: &Path) -> Vec<String> {
    let text = String::from_utf8_lossy(&std::fs::read(mtl).unwrap_or_default()).into_owned();
    text.lines().filter_map(|l| directive(l, "map_Kd")).map(texture_name).filter(|n| !n.is_empty()).collect()
}

fn directive<'a>(line: &'a str, key: &str) -> Option<&'a str> {
    let line = line.trim_start_matches('\u{feff}').trim();
    let (head, rest) = line.split_once(char::is_whitespace)?;
    (head == key).then(|| rest.trim()).filter(|r| !r.is_empty())
}

fn texture_name(value: &str) -> String {
    let mut tokens: Vec<&str> = value.split_whitespace().collect();
    while tokens.first().is_some_and(|t| t.starts_with('-')) {
        let option = tokens.remove(0);
        let take = match option {
            // Up to three numbers.
            "-o" | "-s" | "-t" => tokens.iter().take(3).take_while(|t| t.parse::<f32>().is_ok()).count(),
            "-mm" => 2,
            "-halo" => 0,
            _ => 1,
        };
        tokens.drain(..take.min(tokens.len()));
    }
    tokens.join(" ").trim_matches('"').to_string()
}

fn percent_decode(uri: &str) -> String {
    let bytes = uri.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let Some(v) = std::str::from_utf8(&bytes[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok())
            {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Binary FBX keeps a texture's bytes in a `Content` record of raw data.
fn fbx_embeds_media(bytes: &[u8]) -> bool {
    let tag = b"\x07ContentR";
    bytes.windows(tag.len() + 4).any(|w| {
        w.starts_with(tag)
            && u32::from_le_bytes([w[tag.len()], w[tag.len() + 1], w[tag.len() + 2], w[tag.len() + 3]]) > 0
    })
}

/// Image file names in an FBX: length-prefixed strings in binary files, quoted in ASCII ones.
fn fbx_textures(bytes: &[u8]) -> Vec<String> {
    let lower: Vec<u8> = bytes.iter().map(u8::to_ascii_lowercase).collect();
    let mut names = vec![];
    for ext in IMAGE_EXTENSIONS {
        let needle = format!(".{ext}");
        let mut from = 0;
        while let Some(at) = find(&lower[from..], needle.as_bytes()) {
            let end = from + at + needle.len();
            from = end;
            // Binary: 'S' then a little-endian length, then the string.
            let binary = (1..=end.min(1024)).find_map(|n| {
                let start = end - n;
                (start >= 5
                    && bytes[start - 5] == b'S'
                    && u32::from_le_bytes([bytes[start - 4], bytes[start - 3], bytes[start - 2], bytes[start - 1]])
                        as usize
                        == n)
                    .then(|| &bytes[start..end])
            });
            // ASCII: back to the opening quote, and the quote must close right after.
            let quoted = || {
                (bytes.get(end) == Some(&b'"'))
                    .then(|| bytes[..end].iter().rposition(|&b| b == b'"' || b == b'\n').map(|q| &bytes[q + 1..end]))
                    .flatten()
            };
            if let Some(raw) = binary.or_else(quoted) {
                let name = String::from_utf8_lossy(raw).into_owned();
                if !name.is_empty() && !name.contains(['\0', '\n']) {
                    names.push(name);
                }
            }
        }
    }
    names
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let stamp = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos();
        let dir = std::env::temp_dir().join(format!("bp-companions-{name}-{}-{stamp}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn obj_asks_for_its_mtl_then_for_the_textures_inside_it() {
        let dir = temp("obj");
        let obj = dir.join("spider.obj");
        std::fs::write(&obj, "mtllib spider.mtl\nv 0 0 0\n").unwrap();
        let missing = inspect(&obj, &[]);
        assert_eq!(
            missing,
            vec![Companion { kind: Kind::Materials, name: "spider.mtl".into(), found: false, required: false }]
        );

        // The user adds the MTL from elsewhere; its texture sits in a folder they add next.
        let other = temp("obj-extras");
        let mtl = other.join("spider.mtl");
        std::fs::write(&mtl, "newmtl body\nmap_Kd -s 1 1 1 textures/Body Color.PNG\n").unwrap();
        let after_mtl = inspect(&obj, std::slice::from_ref(&mtl));
        assert!(after_mtl[0].found);
        assert_eq!(after_mtl[1].kind, Kind::Textures);
        assert_eq!(after_mtl[1].name, "textures/Body Color.PNG");
        assert!(!after_mtl[1].found);

        let textures = temp("obj-textures");
        std::fs::create_dir_all(textures.join("sub")).unwrap();
        std::fs::write(textures.join("sub").join("body color.png"), b"png").unwrap();
        let done = inspect(&obj, &[mtl, textures]);
        assert!(done.iter().all(|c| c.found), "{done:?}");
    }

    #[test]
    fn gltf_buffers_are_required_and_data_uris_are_not_files() {
        let dir = temp("gltf");
        let model = dir.join("m.gltf");
        std::fs::write(
            &model,
            r#"{"buffers":[{"uri":"m%20data.bin"},{"uri":"data:application/octet-stream;base64,AA=="}],"images":[{"uri":"albedo.png"}]}"#,
        )
        .unwrap();
        std::fs::write(dir.join("albedo.png"), b"png").unwrap();
        let found = inspect(&model, &[]);
        assert_eq!(found.len(), 2);
        assert_eq!(
            (found[0].kind, found[0].name.as_str(), found[0].found, found[0].required),
            (Kind::Buffers, "m data.bin", false, true)
        );
        assert_eq!((found[1].kind, found[1].found), (Kind::Textures, true));
    }

    #[test]
    fn fbx_names_external_textures_once_and_trusts_embedded_media() {
        let dir = temp("fbx");
        let model = dir.join("m.fbx");
        let path = b"C:\\artist\\tex\\Skin.png";
        let mut bytes = b"Kaydara FBX Binary  \0".to_vec();
        for _ in 0..2 {
            bytes.push(b'S');
            bytes.extend((path.len() as u32).to_le_bytes());
            bytes.extend(path);
        }
        std::fs::write(&model, &bytes).unwrap();
        let found = inspect(&model, &[]);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].name, "C:\\artist\\tex\\Skin.png");
        assert!(!found[0].found);
        // Beside the model by name counts, wherever the artist kept it.
        std::fs::write(dir.join("skin.PNG"), b"png").unwrap();
        assert!(inspect(&model, &[])[0].found);

        bytes.extend(b"\x07ContentR");
        bytes.extend(4u32.to_le_bytes());
        bytes.extend(b"\x89PNG");
        std::fs::remove_file(dir.join("skin.PNG")).unwrap();
        std::fs::write(&model, &bytes).unwrap();
        assert!(inspect(&model, &[]).is_empty());
    }

    #[test]
    fn ascii_fbx_and_mtl_options() {
        let dir = temp("ascii");
        let model = dir.join("m.fbx");
        std::fs::write(&model, "Video: \"Video::a\" {\n RelativeFilename: \"maps/wood.jpg\"\n}\n").unwrap();
        assert_eq!(inspect(&model, &[])[0].name, "maps/wood.jpg");
        assert_eq!(texture_name("-o 0.5 0.5 -bm 1 wood grain.png"), "wood grain.png");
        assert_eq!(percent_decode("a%20b%2Fc.bin"), "a b/c.bin");
    }
}
