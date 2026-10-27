"""Exercise the extractor's path lookup without requiring Blender."""
import ast
import os
from pathlib import Path
import tempfile
import unittest


class LocateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.model = self.root / "model"
        self.model.mkdir()
        self.library = self.root / "materials"
        self.library.mkdir()
        source = Path(__file__).parents[1] / "src" / "extract.py"
        tree = ast.parse(source.read_text(encoding="utf-8"))
        locate = next(node for node in tree.body if isinstance(node, ast.FunctionDef) and node.name == "locate")
        self.env = {"os": os, "MODEL_DIR": str(self.model), "LIBRARY_DIR": None, "BY_NAME": {}}
        exec(compile(ast.Module(body=[locate], type_ignores=[]), str(source), "exec"), self.env)
        self.locate = self.env["locate"]

    def test_windows_reference_finds_mixed_case_texture_beside_model(self):
        texture = self.model / "skin.PNG"
        texture.write_bytes(b"png")
        self.assertTrue(os.path.samefile(self.locate(r"C:\artist\tex\Skin.png", str(self.model)), texture))

    def test_written_path_takes_priority_over_name_fallback(self):
        exact = self.library / "Skin.png"
        exact.write_bytes(b"png")
        (self.model / "skin.PNG").write_bytes(b"png")
        self.assertEqual(self.locate("Skin.png", str(self.library)), str(exact))

    def test_library_filename_takes_priority_over_added_file(self):
        texture = self.library / "skin.PNG"
        texture.write_bytes(b"png")
        self.env["LIBRARY_DIR"] = str(self.library)
        self.env["BY_NAME"] = {"skin.png": "added/SKIN.png"}
        self.assertTrue(os.path.samefile(self.locate("maps/Skin.png", str(self.model)), texture))

    def test_matching_directory_is_skipped_and_missing_folders_are_tolerated(self):
        (self.model / "skin.PNG").mkdir()
        self.env["LIBRARY_DIR"] = str(self.root / "absent")
        self.env["BY_NAME"] = {"skin.png": "added/SKIN.png"}
        self.assertEqual(self.locate("Skin.png", str(self.model)), "added/SKIN.png")
        self.assertIsNone(self.locate("missing.png", str(self.model)))


if __name__ == "__main__":
    unittest.main()
