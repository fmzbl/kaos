from __future__ import annotations

import contextlib
import io
import unittest
from pathlib import Path

from .__main__ import COMMANDS, main, usage


class SisyphusCliTests(unittest.TestCase):
    def test_package_is_integrated_at_repository_root(self) -> None:
        repository = Path(__file__).resolve().parent.parent
        self.assertEqual(Path(__file__).resolve().parent, repository / "sisyphus")
        self.assertFalse((repository / "research" / "sisyphus").exists())

    def test_help_names_every_first_class_operation(self) -> None:
        rendered = usage()
        for command in COMMANDS:
            self.assertIn(command, rendered)

    def test_help_does_not_start_model_work(self) -> None:
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            main(["--help"])
        self.assertIn("python -m sisyphus", output.getvalue())

    def test_unknown_command_is_rejected(self) -> None:
        with self.assertRaisesRegex(SystemExit, "unknown Sisyphus command"):
            main(["not-a-command"])


if __name__ == "__main__":
    unittest.main()
