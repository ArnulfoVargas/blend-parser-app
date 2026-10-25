# Runs inside Blender (headless). Dumps the visible meshes of the open .blend,
# evaluated with modifiers, in world millimetres, with what each material slot
# takes its colour from. Output: scene.json plus little-endian binary arrays.
#
#   blender -b file.blend --factory-startup --python extract.py -- <out_dir>
import json
import os
import sys

import bpy
import numpy as np

OUT = sys.argv[sys.argv.index("--") + 1]
os.makedirs(OUT, exist_ok=True)


def progress(key, name=""):
    # A step key the app translates, then the name it mentions.
    print("BP_PROGRESS " + key + (" " + name if name else ""), flush=True)


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
    bsdf = next((n for n in tree.nodes if n.type == "BSDF_PRINCIPLED"), None)
    if bsdf is None:
        bsdf = next((n for n in tree.nodes if n.type.startswith("BSDF") and "Color" in n.inputs), None)
    if bsdf is None:
        return None
    return bsdf.inputs["Base Color"] if "Base Color" in bsdf.inputs else bsdf.inputs["Color"]


def resolve(mat, me):
    """('image', bpy.types.Image, uv_name|None) | ('vcol', attr_name) | ('flat', srgb)"""
    if mat is None:
        return ("flat", [0.8, 0.8, 0.8])
    sock = base_color_socket(mat)
    if sock is not None:
        if sock.is_linked:
            img_node = find_upstream(sock, {"TEX_IMAGE"})
            if img_node and img_node.image and img_node.image.size[0] > 0:
                uv_node = find_upstream(img_node.inputs["Vector"], {"UVMAP"})
                return ("image", img_node.image, uv_node.uv_map if uv_node and uv_node.uv_map else None)
            attr = find_upstream(sock, {"VERTEX_COLOR", "ATTRIBUTE"})
            if attr is not None:
                name = getattr(attr, "layer_name", None) or getattr(attr, "attribute_name", None)
                if not name and me.color_attributes.active_color:
                    name = me.color_attributes.active_color.name
                if name in me.color_attributes:
                    return ("vcol", name)
        return ("flat", linear_to_srgb(np.array(sock.default_value[:3])).tolist())
    return ("flat", linear_to_srgb(np.array(mat.diffuse_color[:3])).tolist())


scene = bpy.context.scene
units = scene.unit_settings
unit_mm = units.scale_length * 1000.0 if units.system != "NONE" else 1.0
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
    json.dump({"unit_mm": unit_mm, "images": images, "materials": list(materials.values()), "parts": parts}, f)
progress("extracted")
