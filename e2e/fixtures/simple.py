"""A closed cylinder alone (cylinder.blend) and the Suzanne head alone (head.blend), 2 colours each."""
import os, sys
import bpy
out = sys.argv[sys.argv.index("--") + 1]
for name in ("cylinder", "head"):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    a = bpy.data.materials.new("A"); a.use_nodes = True
    a.node_tree.nodes.get("Principled BSDF").inputs["Base Color"].default_value = (0.8, 0.1, 0.1, 1)
    b = bpy.data.materials.new("B"); b.use_nodes = True
    b.node_tree.nodes.get("Principled BSDF").inputs["Base Color"].default_value = (0.1, 0.2, 0.8, 1)
    if name == "cylinder":
        bpy.ops.mesh.primitive_cylinder_add(vertices=48, radius=0.022, depth=0.045, location=(0, 0, 0.0225))
    else:
        bpy.ops.mesh.primitive_monkey_add(size=0.04, location=(0, 0, 0.02))
        m = bpy.context.object.modifiers.new("s", "SUBSURF"); m.levels = 2
        bpy.ops.object.modifier_apply(modifier="s")
    o = bpy.context.object
    o.data.materials.append(a); o.data.materials.append(b)
    for f in o.data.polygons:
        f.material_index = int(f.center.z > 0)  # local: the upper half (the cylinder's top cap)
    bpy.ops.wm.save_as_mainfile(filepath=os.path.join(out, f"{name}.blend"))
