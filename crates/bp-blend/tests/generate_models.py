"""Small deterministic real-import fixtures, generated with the installed Blender."""
import os
import sys
import json

import bpy

out = sys.argv[sys.argv.index("--") + 1]
os.makedirs(out, exist_ok=True)
bpy.ops.wm.read_factory_settings(use_empty=True)
bpy.context.scene.unit_settings.system = "METRIC"
bpy.context.scene.unit_settings.scale_length = 1.0

red = bpy.data.materials.new("Red")
red.use_nodes = True
red.node_tree.nodes.get("Principled BSDF").inputs["Base Color"].default_value = (1, 0, 0, 1)
blue = bpy.data.materials.new("Blue texture")
blue.use_nodes = True
image = bpy.data.images.new("Blue", width=2, height=2)
image.pixels[:] = [0, 0, 1, 1] * 4
image.filepath_raw = os.path.join(out, "blue.png")
image.file_format = "PNG"
image.save()
node = blue.node_tree.nodes.new("ShaderNodeTexImage")
node.image = image
blue.node_tree.links.new(node.outputs["Color"], blue.node_tree.nodes.get("Principled BSDF").inputs["Base Color"])

for x in [-0.015, 0.015]:
    bpy.ops.mesh.primitive_cube_add(size=0.01, location=(x, 0, 0.005))
    mesh = bpy.context.object.data
    mesh.materials.append(red)
    mesh.materials.append(blue)
    for face in mesh.polygons:
        face.material_index = int(face.center.z > 0.004)

bpy.ops.wm.save_as_mainfile(filepath=os.path.join(out, "colors.blend"))
bpy.ops.wm.obj_export(filepath=os.path.join(out, "colors.obj"), global_scale=1000.0, path_mode="COPY")
bpy.ops.export_scene.fbx(filepath=os.path.join(out, "colors.fbx"), path_mode="COPY", bake_anim=False)
bpy.ops.export_scene.gltf(filepath=os.path.join(out, "colors.glb"), export_format="GLB")
bpy.ops.export_scene.gltf(filepath=os.path.join(out, "colors.gltf"), export_format="GLTF_SEPARATE")
with open(os.path.join(out, "colors.gltf"), encoding="utf-8") as f:
    unlit = json.load(f)
unlit.setdefault("extensionsUsed", []).append("KHR_materials_unlit")
for material in unlit["materials"]:
    material.setdefault("extensions", {})["KHR_materials_unlit"] = {}
with open(os.path.join(out, "unlit.gltf"), "w", encoding="utf-8") as f:
    json.dump(unlit, f)

# PLY has no material slots. Its colour attribute must survive without a shader.
bpy.ops.object.select_all(action="SELECT")
bpy.ops.object.join()
mesh = bpy.context.object.data
mesh.materials.clear()
attr = mesh.color_attributes.new(name="Color", type="FLOAT_COLOR", domain="POINT")
for vertex, color in zip(mesh.vertices, attr.data):
    color.color = (1, 0, 0, 1) if vertex.co.x < -0.005 else (0, 0, 1, 1)
mesh.color_attributes.active_color = attr
if bpy.app.version >= (4, 0, 0):
    bpy.ops.wm.ply_export(filepath=os.path.join(out, "colors.ply"), global_scale=1000.0, export_colors="SRGB")
else:
    bpy.ops.export_mesh.ply(filepath=os.path.join(out, "colors.ply"), global_scale=1000.0, use_colors=True)

# Missing sidecar and missing texture are separate failure modes.
with open(os.path.join(out, "missing.obj"), "w") as f:
    f.write("mtllib absent.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl Red\nf 1 2 3\n")
with open(os.path.join(out, "texture.obj"), "w") as f:
    f.write("mtllib texture.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl Red\nf 1 2 3\n")
with open(os.path.join(out, "texture.mtl"), "w") as f:
    f.write("newmtl Red\nKd 1 0 0\nmap_Kd absent.png\n")
with open(os.path.join(out, "tabs.obj"), "w") as f:
    f.write("mtllib\tabsent.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl Red\nf 1 2 3\n")
with open(os.path.join(out, "tabs-texture.obj"), "w") as f:
    f.write("mtllib\ttabs.mtl\nv 0 0 0\nv 1 0 0\nv 0 1 0\nusemtl Red\nf 1 2 3\n")
with open(os.path.join(out, "tabs.mtl"), "w") as f:
    f.write("newmtl Red\nKd 1 0 0\nmap_Kd\tabsent.png\n")
