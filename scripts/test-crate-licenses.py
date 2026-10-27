"""Cargo emits UTF-8 even when Windows uses a legacy default encoding."""
import contextlib
import io
import json
import pathlib
import runpy
import subprocess
import sys
import unittest
from unittest import mock


class CargoEncodingTests(unittest.TestCase):
    def run_script(self, payload, returncode=0):
        script = pathlib.Path(__file__).with_name("crate-licenses.py")
        real_run = subprocess.run
        output = io.StringIO()
        stderr = io.StringIO()

        def cargo(command, **kwargs):
            self.assertEqual(command[:2], ["cargo", "metadata"])
            # Use a real pipe and TextIOWrapper, under the Windows default.
            code = f"import sys; sys.{'stderr' if returncode else 'stdout'}.buffer.write({payload!r}); sys.exit({returncode})"
            return real_run([sys.executable, "-I", "-c", code], **kwargs)

        with mock.patch.object(subprocess, "_text_encoding", return_value="cp1252"), \
             mock.patch.object(subprocess, "run", side_effect=cargo), \
             mock.patch("os.chdir"), mock.patch("builtins.open", return_value=output), \
             mock.patch.object(sys, "argv", [str(script)]), \
             contextlib.redirect_stdout(io.StringIO()), contextlib.redirect_stderr(stderr):
            if returncode:
                with self.assertRaises(SystemExit) as raised:
                    runpy.run_path(str(script), run_name="__main__")
                self.assertEqual(raised.exception.code, 1)
            else:
                runpy.run_path(str(script), run_name="__main__")
        return output.getvalue(), stderr.getvalue()

    def test_metadata_is_utf8(self):
        metadata = {"workspace_members": [], "packages": [{
            "id": "test", "name": "crate-\u0101", "version": "1.0", "license": "MIT", "source": "registry"
        }]}
        output, _ = self.run_script(json.dumps(metadata, ensure_ascii=False).encode("utf-8"))
        self.assertIn("| crate-\u0101 | 1.0 | MIT |", output)

    def test_cargo_failure_preserves_utf8_diagnostic(self):
        _, stderr = self.run_script("missing crate-\u0101".encode("utf-8"), 7)
        self.assertIn("exit 7", stderr)
        self.assertIn("missing crate-\u0101", stderr)


if __name__ == "__main__":
    unittest.main()
