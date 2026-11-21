"""Deterministic E2E/benchmark fixtures, generated with the installed Blender.

    blender --background --factory-startup --python e2e/fixtures/generate.py -- <out dir> [subdiv]

figure.blend: a painted figurine (head with four flat colours on a striped,
textured body), about 80 mm tall. `subdiv` (default 2) sets the head's
subdivision: 2 is ~8k triangles, 5 is ~500k (the benchmark model).
"""
import math
import os
import sys

import bpy

args = sys.argv[sys.argv.index("--") + 1:]
out = args[0]
subdiv = int(args[1]) if len(args) > 1 else 2
os.makedirs(out, exist_ok=True)
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.context.scene.unit_settings.system = "METRIC"


def flat(name, rgb):
    m = bpy.data.materials.new(name)
    m.use_nodes = True
    m.node_tree.nodes.get("Principled BSDF").inputs["Base Color"].default_value = (*rgb, 1)
    return m


skin = flat("Skin", (0.9, 0.62, 0.45))
eyes = flat("Eyes", (0.02, 0.02, 0.02))
hat = flat("Hat", (0.75, 0.05, 0.05))
ears = flat("Ears", (0.95, 0.85, 0.2))

# Body: a cylinder with a striped image texture.
stripes = bpy.data.materials.new("Stripes")
stripes.use_nodes = True
size = 64
img = bpy.data.images.new("stripes", width=size, height=size)
px = []
for y in range(size):
    band = (y // 8) % 3
    c = [(0.1, 0.3, 0.85), (0.95, 0.95, 0.95), (0.1, 0.6, 0.2)][band]
    for _ in range(size):
        px += [*c, 1.0]
img.pixels[:] = px
img.filepath_raw = os.path.join(out, "stripes.png")
img.file_format = "PNG"
img.save()
tex = stripes.node_tree.nodes.new("ShaderNodeTexImage")
tex.image = img
stripes.node_tree.links.new(tex.outputs["Color"], stripes.node_tree.nodes.get("Principled BSDF").inputs["Base Color"])

bpy.ops.mesh.primitive_cylinder_add(vertices=48, radius=0.022, depth=0.045, location=(0, 0, 0.0225))
body = bpy.context.object
bpy.ops.object.mode_set(mode="EDIT")
bpy.ops.mesh.subdivide(number_cuts=6)
bpy.ops.uv.cylinder_project()
bpy.ops.object.mode_set(mode="OBJECT")
body.data.materials.append(stripes)

bpy.ops.mesh.primitive_monkey_add(size=0.04, location=(0, 0, 0.062), rotation=(0, 0, 0))
head = bpy.context.object
mod = head.modifiers.new("sub", "SUBSURF")
mod.levels = subdiv
mod.render_levels = subdiv
bpy.ops.object.modifier_apply(modifier="sub")
mesh = head.data
for m in (skin, eyes, hat, ears):
    mesh.materials.append(m)
for f in mesh.polygons:
    c = f.center
    if c.z > 0.013:
        f.material_index = 2  # hat
    elif abs(c.x) > 0.02:
        f.material_index = 3  # ears
    elif c.y < -0.012 and 0.0 < c.z < 0.01 and abs(c.x) < 0.014:
        f.material_index = 1  # eyes
    else:
        f.material_index = 0
bpy.ops.wm.save_as_mainfile(filepath=os.path.join(out, "figure.blend" if subdiv <= 3 else f"figure-s{subdiv}.blend"))
print("triangles", sum(len(p.vertices) - 2 for o in bpy.data.objects for p in o.data.polygons))
