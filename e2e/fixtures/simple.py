"""A closed cylinder alone (cylinder.blend), the Suzanne head alone (head.blend) and
a block with a 0.5 mm fin and a 0.6 mm pin on it (thin.blend), 2 colours each."""
import os, sys
import bpy
out = sys.argv[sys.argv.index("--") + 1]
for name in ("cylinder", "head", "thin"):
    bpy.ops.wm.read_factory_settings(use_empty=True)
    a = bpy.data.materials.new("A"); a.use_nodes = True
    a.node_tree.nodes.get("Principled BSDF").inputs["Base Color"].default_value = (0.8, 0.1, 0.1, 1)
    b = bpy.data.materials.new("B"); b.use_nodes = True
    b.node_tree.nodes.get("Principled BSDF").inputs["Base Color"].default_value = (0.1, 0.2, 0.8, 1)
    if name == "cylinder":
        bpy.ops.mesh.primitive_cylinder_add(vertices=48, radius=0.022, depth=0.045, location=(0, 0, 0.0225))
    elif name == "head":
        bpy.ops.mesh.primitive_monkey_add(size=0.04, location=(0, 0, 0.02))
        m = bpy.context.object.modifiers.new("s", "SUBSURF"); m.levels = 2
        bpy.ops.object.modifier_apply(modifier="s")
    else:
        # 20 mm block; on it a 0.5 mm fin and a 0.6 mm pin, separate closed pieces in one object.
        bpy.ops.mesh.primitive_cube_add(size=0.02, location=(0, 0, 0.01))
        bpy.ops.mesh.primitive_cube_add(size=1, location=(-0.004, 0, 0.026))
        bpy.context.object.scale = (0.0005, 0.012, 0.012)
        bpy.ops.mesh.primitive_cylinder_add(vertices=16, radius=0.0003, depth=0.01, location=(0.006, 0, 0.025))
        for ob in bpy.context.scene.objects:
            ob.select_set(True)
        bpy.context.view_layer.objects.active = bpy.context.scene.objects[0]
        bpy.ops.object.join()
        bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
    o = bpy.context.object
    o.data.materials.append(a); o.data.materials.append(b)
    for f in o.data.polygons:
        f.material_index = int(f.center.z > (0.01 if name == "thin" else 0))  # local: the upper part (the cylinder's top cap, the fin and pin)
    bpy.ops.wm.save_as_mainfile(filepath=os.path.join(out, f"{name}.blend"))
