#!/usr/bin/env python3
"""Build/launcher contracts using fake tools: no installs, compilation or login."""

import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest


ROOT = Path(__file__).resolve().parent.parent
TOOL = """#!/usr/bin/env python3
import json, os, pathlib, sys
name = pathlib.Path(sys.argv[0]).name
with open(os.environ["TOOL_LOG"], "a") as log:
    log.write(json.dumps({"tool": name, "args": sys.argv[1:], "cwd": os.getcwd(),
                         "data": os.environ.get("WOW_DATA"),
                         "host": os.environ.get("WOW_HOST"),
                         "user": os.environ.get("WOW_USER"),
                         "password": os.environ.get("WOW_PASS")}) + "\\n")
if name == "uname":
    print(os.environ.get("TEST_OS", "Linux"))
elif name == "id":
    print("1000")
elif name == "sudo":
    os.execvp(sys.argv[1], sys.argv[1:])
elif name == "pkg-config":
    sys.exit(int(os.environ.get("TEST_PKG_STATUS", "0")))
elif name == "cargo" and "run" in sys.argv:
    sys.exit(int(os.environ.get("TEST_RUN_STATUS", "0")))
elif name == "cargo" and "build" in sys.argv:
    sys.exit(int(os.environ.get("TEST_BUILD_STATUS", "0")))
elif name == "rustup":
    sys.exit(int(os.environ.get("TEST_RUSTUP_STATUS", "0")))
"""


class ToolTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix="benilla tools ")
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name) / "checkout"
        self.root.mkdir()
        self.bin = Path(self.temp.name) / "bin"
        self.bin.mkdir()
        self.log = Path(self.temp.name) / "tools.jsonl"
        for name in ("build-benilla.sh", "run-benilla.sh"):
            shutil.copy2(ROOT / name, self.root / name)
        shutil.copy2(ROOT / "rust-toolchain.toml", self.root / "rust-toolchain.toml")
        (self.root / "Cargo.toml").touch()
        for name in ("cargo", "rustc", "rustup", "cc", "pkg-config", "python3",
                     "uname", "id", "sudo", "apt-get", "dnf", "pacman",
                     "xcode-select", "brew"):
            # Use the real interpreter even when python3 on PATH is fake.
            tool = self.bin / name
            tool.write_text(TOOL.replace("#!/usr/bin/env python3", f"#!{os.sys.executable}", 1))
            tool.chmod(0o755)
        self.env = {key: value for key, value in os.environ.items()
                    if not key.startswith(("WOW_", "CARGO_", "RUSTUP_", "TEST_"))}
        self.env.update(PATH=f"{self.bin}:{os.defpath}", TOOL_LOG=str(self.log),
                        HOME=str(Path(self.temp.name) / "home"))

    def run_script(self, name, *args, **env):
        return subprocess.run(
            ["bash", str(self.root / name), *args], cwd=self.temp.name,
            env={**self.env, **env}, text=True, capture_output=True, check=False)

    def calls(self, name=None):
        calls = [json.loads(line) for line in self.log.read_text().splitlines()] if self.log.exists() else []
        return [call for call in calls if name is None or call["tool"] == name]

    def test_help_has_no_side_effects(self):
        for script in ("build-benilla.sh", "run-benilla.sh"):
            with self.subTest(script=script):
                self.assertEqual(self.run_script(script, "--help").returncode, 0)
        self.assertEqual(self.calls(), [])

    def test_invalid_build_options(self):
        for args in (("--jobs",), ("--jobs", "0"), ("--jobs", "oops"),
                     ("--profile",), ("--profile", "oops"), ("--unknown",)):
            with self.subTest(args=args):
                self.assertNotEqual(self.run_script("build-benilla.sh", *args).returncode, 0)
        self.assertEqual(self.calls(), [])

    def test_build_all_binaries_from_any_directory(self):
        result = self.run_script("build-benilla.sh", "--skip-system", "--skip-toolchain",
                                 "--profile", "dev", "--jobs", "2")
        self.assertEqual(result.returncode, 0, result.stderr)
        build = self.calls("cargo")[-1]
        self.assertEqual(build["args"], ["build", "--locked", "--workspace", "--bins",
                                        "--profile", "dev", "--jobs", "2"])
        self.assertEqual(build["cwd"], str(self.root))
        self.assertFalse(self.calls("rustup"))

    def test_toolchain_uses_repository_pin(self):
        result = self.run_script("build-benilla.sh", "--skip-system")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls("rustup")[0]["args"], ["toolchain", "install"])
        self.assertEqual(self.calls("rustup")[0]["cwd"], str(self.root))

    def test_build_and_prerequisite_failures_propagate(self):
        for failure, code in (("TEST_BUILD_STATUS", "9"), ("TEST_PKG_STATUS", "1"),
                              ("TEST_RUSTUP_STATUS", "7")):
            with self.subTest(failure=failure):
                self.log.unlink(missing_ok=True)
                result = self.run_script("build-benilla.sh", "--skip-system", **{failure: code})
                self.assertNotEqual(result.returncode, 0)
                self.assertNotIn("Build complete", result.stdout)
                if failure != "TEST_BUILD_STATUS":
                    self.assertFalse(any("build" in call["args"] for call in self.calls("cargo")))

    def test_native_packages_and_toolchain(self):
        result = self.run_script("build-benilla.sh")
        self.assertEqual(result.returncode, 0, result.stderr)
        managers = [call for call in self.calls() if call["tool"] in ("apt-get", "dnf", "pacman", "brew")]
        self.assertTrue(managers)
        self.assertTrue(self.calls("rustup"))

    def test_macos_packages(self):
        result = self.run_script("build-benilla.sh", TEST_OS="Darwin")
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls("brew")[0]["args"], ["install", "pkg-config", "python"])

    def test_unsupported_os_fails_before_build(self):
        result = self.run_script("build-benilla.sh", TEST_OS="Unsupported")
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.calls("cargo"))

    def test_launcher_defaults_and_custom_target(self):
        result = self.run_script("run-benilla.sh", CARGO_TARGET_DIR="/tmp/custom-target")
        self.assertEqual(result.returncode, 0, result.stderr)
        run = self.calls("cargo")[-1]
        self.assertEqual(run["args"], ["run", "--locked", "--release", "-p", "benilla"])
        self.assertEqual(run["cwd"], str(self.root))
        self.assertIsNone(run["data"])
        self.assertEqual(run["host"], "127.0.0.1:3724")
        self.assertEqual(run["user"], "")
        self.assertEqual(run["password"], "")

    def test_sibling_data_and_explicit_overrides(self):
        data = Path(self.temp.name) / "VanillaClient" / "Data"
        data.mkdir(parents=True)
        self.assertEqual(self.run_script("run-benilla.sh").returncode, 0)
        self.assertEqual(Path(self.calls("cargo")[-1]["data"]).resolve(), data)
        for value in ("", "/another/Data"):
            with self.subTest(data=value):
                result = self.run_script("run-benilla.sh", "argument-user", "argument-password",
                                         WOW_DATA=value, WOW_HOST="server:5000",
                                         WOW_USER="environment-user", WOW_PASS="environment-password")
                self.assertEqual(result.returncode, 0, result.stderr)
                run = self.calls("cargo")[-1]
                self.assertEqual(run["data"], value)
                self.assertEqual(run["host"], "server:5000")
                self.assertEqual(run["user"], "argument-user")
                self.assertEqual(run["password"], "argument-password")

    def test_launcher_environment_credentials_and_failure(self):
        result = self.run_script("run-benilla.sh", WOW_USER="test-user", WOW_PASS="test-password",
                                 TEST_RUN_STATUS="6")
        self.assertEqual(result.returncode, 6, result.stderr)
        self.assertEqual(self.calls("cargo")[-1]["user"], "test-user")
        self.assertEqual(self.calls("cargo")[-1]["password"], "test-password")

    def test_original_parent_directory_location(self):
        parent = self.root.parent
        checkout = self.root.rename(parent / "benilla")
        shutil.copy2(checkout / "run-benilla.sh", parent / "run-benilla.sh")
        result = subprocess.run(
            ["bash", str(parent / "run-benilla.sh")], cwd="/",
            env=self.env, text=True, capture_output=True, check=False)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(self.calls("cargo")[-1]["cwd"], str(checkout))


if __name__ == "__main__":
    unittest.main()
