# Runs inside Blender (headless). Imports and dumps the visible meshes,
# evaluated with modifiers, in world millimetres, with what each material slot
# takes its colour from. Output: scene.json plus little-endian binary arrays.
#
#   blender -b --factory-startup --disable-autoexec --python extract.py -- <out_dir> <model> [added files…]
import json
import os
import shlex
import shutil
import sys

import bpy
import numpy as np

ARGS = sys.argv[sys.argv.index("--") + 1:]
OUT = ARGS[0]
os.makedirs(OUT, exist_ok=True)
ORIGINAL = os.path.abspath(ARGS[1])
SOURCE = ORIGINAL
MODEL_DIR = os.path.dirname(ORIGINAL)
# Files the user added for what the model references, matched by name.
EXTRAS = [os.path.abspath(p) for p in ARGS[2:]]
BY_NAME = {os.path.basename(p).lower(): p for p in EXTRAS}
EXT = os.path.splitext(SOURCE)[1].lower()
STAGE = None
# What the user added beyond files: an image per material, an OBJ material
# library to use instead of the model's, and a .blend to take packed textures from.
try:
    with open(os.path.join(OUT, "additions.json"), encoding="utf-8") as f:
        ADDED = json.load(f)
except (OSError, ValueError):
    ADDED = {}
ASSIGNED = ADDED.get("textures") or {}
LIBRARY = ADDED.get("library")
LIBRARY_DIR = os.path.dirname(LIBRARY) if LIBRARY else None
TEXTURE_BLEND = ADDED.get("blend")
BLEND_IMAGES = {}
warnings = []


def warn(code, detail):
    item = {"code": code, "detail": str(detail)}
    if item not in warnings:
        warnings.append(item)


def directive(line):
    fields = line.strip().split(None, 1)
    return (fields[0], fields[1]) if len(fields) == 2 else ("", "")


def path_tokens(value):
    # Keep Windows separators and apostrophes inside unquoted filenames literal.
    try:
        return [s.strip('"\'') for s in shlex.split(value, comments=True, posix=False)]
    except ValueError:
        return [value.strip('"')]


def texture_name(value):
    tokens = path_tokens(value)
    while tokens and tokens[0].startswith("-"):
        option = tokens.pop(0)
        if option in ("-o", "-s", "-t"):
            for _ in range(3):
                if not tokens:
                    break
                try:
                    float(tokens[0])
                except ValueError:
                    break
                tokens.pop(0)
        else:
            count = 2 if option == "-mm" else (0 if option == "-halo" else 1)
            tokens = tokens[count:]
    return " ".join(tokens)


def locate(name, base):
    """Where a referenced file is: as written beside `base` or the model, by name
    beside the model, or among the added files. Same rules as companions.rs."""
    name = name.strip().strip('"')
    if not name:
        return None
    key = name.replace("\\", "/").rsplit("/", 1)[-1].lower()
    dirs = [MODEL_DIR] + ([LIBRARY_DIR] if LIBRARY_DIR else [])
    candidates = [os.path.join(base, name)] + [os.path.join(d, n) for d in dirs for n in (name, key)]
    for candidate in candidates:
        if os.path.isfile(candidate):
            return candidate
    # Match actual filenames: lowercasing a path alone misses skin.PNG on Linux.
    for folder in dirs:
        try:
            names = sorted(os.listdir(folder))
        except OSError:
            continue
        for filename in names:
            candidate = os.path.join(folder, filename)
            if filename.lower() == key and os.path.isfile(candidate):
                return candidate
    return BY_NAME.get(key)


def mtl_libraries(obj):
    names = []
    with open(obj, encoding="utf-8-sig", errors="replace") as f:
        for line in f:
            key, value = directive(line)
            if key != "mtllib":
                continue
            whole = os.path.join(os.path.dirname(obj), value.strip('"'))
            names += [value.strip('"')] if os.path.isfile(whole) else path_tokens(value)
    return names


def obj_resources():
    """OBJ resources are relative to the OBJ/MTL, never the process directory."""
    for name in mtl_libraries(SOURCE):
        path = locate(name, os.path.dirname(SOURCE))
        if path is None:
            warn("missingMaterial", name)
            continue
        with open(path, encoding="utf-8-sig", errors="replace") as mtl:
            for row in mtl:
                key, value = directive(row)
                if key == "map_Kd":
                    # MTL texture options precede the filename.
                    tex = locate(value, os.path.dirname(path)) or locate(texture_name(value), os.path.dirname(path))
                    if value.startswith("-"):
                        warn("shaderApproximation", path)
                    if tex is None:
                        warn("missingTexture", texture_name(value))


def stage():
    """Importers look for an OBJ's MTL and a glTF's buffers beside the model.
    When the user supplied one from elsewhere, import a copy of the model from a
    folder that has every such file under the name the model uses."""
    global SOURCE, STAGE
    if EXT == ".obj":
        refs = mtl_libraries(ORIGINAL)
    elif EXT == ".gltf":
        with open(ORIGINAL, encoding="utf-8") as f:
            doc = json.load(f)
        from urllib.parse import unquote
        refs = [unquote(b["uri"]) for key in ("buffers", "images") for b in doc.get(key, [])
                if "uri" in b and not b["uri"].startswith("data:")]
    else:
        return
    beside = lambda r: os.path.isfile(os.path.join(MODEL_DIR, r))
    if not (LIBRARY and EXT == ".obj") and (not EXTRAS or all(beside(r) for r in refs)):
        return
    STAGE = os.path.join(OUT, "stage")
    os.makedirs(STAGE, exist_ok=True)
    SOURCE = os.path.join(STAGE, os.path.basename(ORIGINAL))
    shutil.copyfile(ORIGINAL, SOURCE)
    for ref in refs:
        # A library the user chose stands in for every one the OBJ names.
        found = LIBRARY if LIBRARY and EXT == ".obj" else locate(ref, MODEL_DIR)
        target = os.path.normpath(os.path.join(STAGE, ref))
        if found and target.startswith(STAGE):
            os.makedirs(os.path.dirname(target), exist_ok=True)
            shutil.copyfile(found, target)


def relink():
    """Points images that did not load at the file the user added or the one
    beside the model, wherever the model said they were."""
    for img in list(bpy.data.images):
        if img.source != "FILE" or img.packed_file or not img.filepath:
            continue
        path = os.path.normpath(bpy.path.abspath(img.filepath))
        if os.path.isfile(path):
            continue
        written = os.path.relpath(path, STAGE) if STAGE and path.startswith(STAGE) else img.filepath
        found = locate(written, MODEL_DIR) or locate(img.name, MODEL_DIR)
        if found:
            # An importer's placeholder does not come back with reload(): load the
            # file anew and hand it every node that used the placeholder.
            img.user_remap(bpy.data.images.load(found, check_existing=True))


def import_source():
    if EXT == ".blend":
        bpy.ops.wm.open_mainfile(filepath=SOURCE, load_ui=False, use_scripts=False)
        return
    bpy.ops.wm.read_factory_settings(use_empty=True)
    progress("importing", os.path.basename(ORIGINAL))
    stage()
    if EXT == ".obj":
        obj_resources()
        bpy.ops.wm.obj_import(filepath=SOURCE, forward_axis="NEGATIVE_Z", up_axis="Y")
    elif EXT == ".fbx":
        # Bundled since Blender 3.6. Fail explicitly if a custom build omits it.
        bpy.ops.import_scene.fbx(filepath=SOURCE, use_image_search=False, use_anim=False)
    elif EXT in (".gltf", ".glb"):
        bpy.ops.import_scene.gltf(filepath=SOURCE)
    elif EXT == ".ply":
        if bpy.app.version >= (4, 0, 0):
            bpy.ops.wm.ply_import(filepath=SOURCE)
        else:
            bpy.ops.import_mesh.ply(filepath=SOURCE)
    else:
        raise ValueError("Unsupported input: " + EXT)
    relink()
    # glTF and Blender's FBX importer produce metres. OBJ/PLY have no units:
    # interpret their coordinate values as millimetres, matching unitless .blend.
    warn("units", "m → mm" if EXT in (".fbx", ".gltf", ".glb") else "1 = 1 mm")


def progress(key, name=""):
    # A step key the app translates, then the name it mentions.
    print("BP_PROGRESS " + key + (" " + name if name else ""), flush=True)


import_source()


def linear_to_srgb(c):
    c = np.clip(c, 0.0, 1.0)
    return np.where(c <= 0.0031308, c * 12.92, 1.055 * np.power(c, 1 / 2.4) - 0.055)


def dump(name, array):
    np.ascontiguousarray(array).tofile(os.path.join(OUT, name))
    return name


def find_upstream(socket, types, seen=None):
    seen = seen if seen is not None else set()
    for link in socket.links:
        node = link.from_node
        if node.name in seen:
            continue
        seen.add(node.name)
        if node.type in types:
            return node
        for inp in node.inputs:
            found = find_upstream(inp, types, seen)
            if found:
                return found
    return None


def base_color_socket(mat):
    tree = mat.node_tree
    if not tree:
        return None
    output = next((n for n in tree.nodes if n.type == "OUTPUT_MATERIAL" and n.is_active_output), None)
    surface = output.inputs["Surface"] if output else None
    bsdf = find_upstream(surface, {"BSDF_PRINCIPLED"}) if surface else None
    if bsdf is None:
        # Blender represents glTF KHR_materials_unlit with Emission + Transparent.
        # The transparent branch is white; the emission branch owns the actual colour.
        bsdf = find_upstream(surface, {"EMISSION"}) if surface else None
    if bsdf is None:
        bsdf = next((n for n in tree.nodes if n.type.startswith("BSDF") and "Color" in n.inputs), None)
        warn("shaderApproximation", mat.name)
    if bsdf is None:
        return None
    return bsdf.inputs["Base Color"] if "Base Color" in bsdf.inputs else bsdf.inputs["Color"]


def blend_textures():
    """Packed textures of the user's .blend, for materials of the same name: the
    image feeding each one's base colour (or its first image)."""
    # OBJ export writes spaces in material names as underscores.
    key = lambda n: n.lower().replace(" ", "_")
    names = {key(m.name): m.name for m in bpy.data.materials}
    with bpy.data.libraries.load(TEXTURE_BLEND, link=False) as (src, dst):
        wanted = [n for n in src.materials if key(n) in names]
        # A copy: Blender swaps the names in this list for the loaded materials.
        dst.materials = list(wanted)
    for blend_name, mat in zip(wanted, dst.materials):
        name = names[key(blend_name)]
        if mat is None or not mat.node_tree:
            continue
        sock = base_color_socket(mat)
        node = find_upstream(sock, {"TEX_IMAGE"}) if sock is not None and sock.is_linked else None
        node = node or next((n for n in mat.node_tree.nodes if n.type == "TEX_IMAGE" and n.image), None)
        if node and node.image and node.image.size[0] > 0:
            BLEND_IMAGES[name] = node.image
    warn("blendTextures", "%d/%d" % (len(BLEND_IMAGES), len(names)))


def resolve(mat, me):
    """('image', bpy.types.Image, uv_name|None) | ('vcol', attr_name) | ('flat', srgb)"""
    found = resolve_own(mat, me)
    # The .blend only fills materials that came out without a texture or attribute.
    if found[0] == "flat" and mat is not None and mat.name in BLEND_IMAGES:
        return ("image", BLEND_IMAGES[mat.name], None)
    return found


def resolve_own(mat, me):
    if mat is not None and mat.name in ASSIGNED:
        try:
            return ("image", bpy.data.images.load(ASSIGNED[mat.name], check_existing=True), None)
        except RuntimeError:
            warn("missingTexture", ASSIGNED[mat.name])
    if mat is None:
        if me.color_attributes.active_color:
            return ("vcol", me.color_attributes.active_color.name)
        warn("noColor", me.name)
        return ("flat", [0.8, 0.8, 0.8])
    sock = base_color_socket(mat)
    if sock is not None:
        if sock.is_linked:
            img_node = find_upstream(sock, {"TEX_IMAGE"})
            if img_node and img_node.image and img_node.image.size[0] > 0 and img_node.image.has_data:
                if len(sock.links) != 1 or sock.links[0].from_node != img_node:
                    warn("shaderApproximation", mat.name)
                vector = img_node.inputs["Vector"]
                if vector.is_linked and vector.links[0].from_node.type != "UVMAP":
                    warn("shaderApproximation", mat.name)
                if img_node.extension != "REPEAT" or img_node.projection != "FLAT":
                    warn("shaderApproximation", mat.name)
                uv_node = find_upstream(img_node.inputs["Vector"], {"UVMAP"})
                return ("image", img_node.image, uv_node.uv_map if uv_node and uv_node.uv_map else None)
            if img_node:
                warn("missingTexture", img_node.image.filepath if img_node.image else mat.name)
            attr = find_upstream(sock, {"VERTEX_COLOR", "ATTRIBUTE"})
            if attr is not None:
                name = getattr(attr, "layer_name", None) or getattr(attr, "attribute_name", None)
                if not name and me.color_attributes.active_color:
                    name = me.color_attributes.active_color.name
                if name in me.color_attributes:
                    if sock.links[0].from_node != attr:
                        warn("shaderApproximation", mat.name)
                    return ("vcol", name)
            warn("shaderApproximation", mat.name)
        return ("flat", linear_to_srgb(np.array(sock.default_value[:3])).tolist())
    return ("flat", linear_to_srgb(np.array(mat.diffuse_color[:3])).tolist())


if TEXTURE_BLEND and EXT != ".blend":
    blend_textures()
scene = bpy.context.scene
units = scene.unit_settings
unit_mm = units.scale_length * 1000.0 if units.system != "NONE" else 1.0
if EXT != ".blend":
    unit_mm = 1000.0 if EXT in (".fbx", ".gltf", ".glb") else 1.0
depsgraph = bpy.context.evaluated_depsgraph_get()

images, image_index = [], {}
materials = {}


def add_image(img):
    if img.name in image_index:
        return image_index[img.name]
    progress("texture", img.name)
    w, h = img.size
    px = np.empty(w * h * 4, dtype=np.float32)
    img.pixels.foreach_get(px)
    rgb = px.reshape(-1, 4)[:, :3]
    if img.is_float:
        rgb = linear_to_srgb(rgb)
    data = (np.clip(rgb, 0, 1) * 255 + 0.5).astype(np.uint8)
    idx = len(images)
    images.append({"name": img.name, "width": w, "height": h, "file": dump(f"img{idx}.bin", data)})
    image_index[img.name] = idx
    return idx


parts = []
objs = [o for o in scene.objects if o.type == "MESH" and o.visible_get()]
for oi, obj in enumerate(objs):
    progress("object", obj.name)
    ev = obj.evaluated_get(depsgraph)
    me = ev.to_mesh()
    me.calc_loop_triangles()
    mw = np.array(obj.matrix_world)

    nv, nt, nl = len(me.vertices), len(me.loop_triangles), len(me.loops)
    if nt == 0:
        ev.to_mesh_clear()
        continue
    co = np.empty(nv * 3, dtype=np.float64)
    me.vertices.foreach_get("co", co)
    co = (np.c_[co.reshape(nv, 3), np.ones(nv)] @ mw.T)[:, :3] * unit_mm
    tv = np.empty(nt * 3, dtype=np.int64)
    tl = np.empty(nt * 3, dtype=np.int64)
    tm = np.empty(nt, dtype=np.int64)
    me.loop_triangles.foreach_get("vertices", tv)
    me.loop_triangles.foreach_get("loops", tl)
    me.loop_triangles.foreach_get("material_index", tm)
    tv, tl = tv.reshape(nt, 3), tl.reshape(nt, 3)
    if np.linalg.det(mw[:3, :3]) < 0:  # mirrored: keep the outside out
        tv, tl = tv[:, ::-1].copy(), tl[:, ::-1].copy()
    loop_vert = np.empty(nl, dtype=np.int64)
    me.loops.foreach_get("vertex_index", loop_vert)
    poly_mat = np.empty(len(me.polygons), dtype=np.int64)
    me.polygons.foreach_get("material_index", poly_mat)

    slots = [s.material for s in obj.material_slots] or [None]
    faces_per_slot = np.bincount(np.clip(poly_mat, 0, len(slots) - 1), minlength=len(slots))
    uv_files, uv_index, col_files, col_index, sources = [], {}, [], {}, []
    for si, mat in enumerate(slots):
        src = resolve(mat, me)
        label = mat.name if mat else "(sin material)"
        if src[0] == "image":
            layer = me.uv_layers.get(src[2]) if src[2] else me.uv_layers.active
            if layer is None:
                warn("missingUv", label)
                avg = np.array(src[1].pixels[:]).reshape(-1, 4)[:, :3].mean(0)
                src = ("flat", avg.tolist())
            else:
                if layer.name not in uv_index:
                    uv = np.empty(nl * 2, dtype=np.float32)
                    layer.data.foreach_get("uv", uv)
                    uv_index[layer.name] = len(uv_files)
                    uv_files.append(dump(f"p{oi}_uv{len(uv_files)}.bin", uv))
                img_idx = add_image(src[1])
                sources.append({"kind": "image", "image": img_idx, "uv": uv_index[layer.name]})
                info = ("image", f"{src[1].size[0]}×{src[1].size[1]}")
        if src[0] == "vcol":
            name = src[1]
            if name not in col_index:
                attr = me.color_attributes[name]
                n = len(attr.data)
                c = np.empty(n * 4, dtype=np.float32)
                attr.data.foreach_get("color_srgb", c)
                c = c.reshape(n, 4)[:, :3]
                if attr.domain == "POINT":
                    c = c[loop_vert]
                col_index[name] = len(col_files)
                col_files.append(dump(f"p{oi}_col{len(col_files)}.bin", c.astype(np.float32)))
            sources.append({"kind": "vcol", "attr": col_index[name]})
            info = ("vertex", name)
        if src[0] == "flat":
            rgb = [float(v) for v in src[1]]
            sources.append({"kind": "flat", "rgb": rgb})
            info = ("flat", "#%02X%02X%02X" % tuple(int(round(v * 255)) for v in rgb))
        m = materials.setdefault(label, {"name": label, "kind": info[0], "detail": info[1], "faces": 0})
        m["faces"] += int(faces_per_slot[si])

    parts.append({
        "name": obj.name,
        "vertices": nv,
        "triangles": nt,
        "loops": nl,
        "positions": dump(f"p{oi}_pos.bin", co.astype(np.float32)),
        "tris": dump(f"p{oi}_tris.bin", tv.astype(np.uint32)),
        "tri_loops": dump(f"p{oi}_loops.bin", tl.astype(np.uint32)),
        "tri_slot": dump(f"p{oi}_slot.bin", np.clip(tm, 0, len(slots) - 1).astype(np.uint16)),
        "uv_layers": uv_files,
        "color_layers": col_files,
        "sources": sources,
    })
    ev.to_mesh_clear()

with open(os.path.join(OUT, "scene.json"), "w", encoding="utf-8") as f:
    json.dump({"unit_mm": unit_mm, "images": images, "materials": list(materials.values()), "parts": parts, "warnings": warnings}, f)
progress("extracted")
