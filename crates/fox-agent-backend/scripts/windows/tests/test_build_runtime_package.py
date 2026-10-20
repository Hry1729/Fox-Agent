#!/usr/bin/env python3
"""Unit tests for build-runtime-package.py (not shipped).

Run from anywhere:  python3 -m unittest discover -s scripts/windows/tests -v

* Pure path logic (extended-length conversion, package keys) is tested on
  every OS, including the Windows-only branches, as string functions.
* stage -> pack -> check runs for real on the current OS with a dependency
  tree whose deepest file path is far over 260 characters, the source given
  as a relative path (the case that failed with WinError 3 on Windows).
"""
import hashlib
import importlib.util
import json
import os
import shutil
import tempfile
import unittest
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
SCRIPT = os.path.join(os.path.dirname(HERE), "build-runtime-package.py")
spec = importlib.util.spec_from_file_location("build_runtime_package", SCRIPT)
brp = importlib.util.module_from_spec(spec)
spec.loader.exec_module(brp)


class ExtendedPathTest(unittest.TestCase):
    def test_drive_absolute(self):
        self.assertEqual(brp.extended_path(r"C:\Program Files\Fox\x"), "\\\\?\\C:\\Program Files\\Fox\\x")

    def test_forward_slashes_and_dots_are_normalized(self):
        self.assertEqual(brp.extended_path("C:/a/b/../c/./d.txt"), "\\\\?\\C:\\a\\c\\d.txt")

    def test_relative_uses_cwd(self):
        self.assertEqual(brp.extended_path(r"stage\node\node.exe", cwd=r"C:\work"), "\\\\?\\C:\\work\\stage\\node\\node.exe")
        self.assertEqual(brp.extended_path("../out/pkg.zip", cwd="C:/work/sub"), "\\\\?\\C:\\work\\out\\pkg.zip")

    def test_rooted_without_drive_takes_the_cwd_drive(self):
        self.assertEqual(brp.extended_path(r"\tmp\x", cwd=r"D:\work"), "\\\\?\\D:\\tmp\\x")

    def test_unc(self):
        self.assertEqual(brp.extended_path(r"\\server\share\pkg\a.js"), "\\\\?\\UNC\\server\\share\\pkg\\a.js")
        self.assertEqual(brp.extended_path("//server/share/pkg/a.js"), "\\\\?\\UNC\\server\\share\\pkg\\a.js")

    def test_already_extended_only_gets_separators_fixed(self):
        self.assertEqual(brp.extended_path("\\\\?\\C:\\a\\b"), "\\\\?\\C:\\a\\b")
        # The round-5 false "missing": forward slashes after \\?\ are not
        # normalized by Windows, so they must be fixed here.
        self.assertEqual(brp.extended_path("\\\\?\\C:\\pkg/node/node.exe"), "\\\\?\\C:\\pkg\\node\\node.exe")
        self.assertEqual(brp.extended_path("\\\\?\\UNC\\srv\\share/x"), "\\\\?\\UNC\\srv\\share\\x")

    def test_long_path_kept_whole(self):
        deep = "C:\\stage\\" + "\\".join(["node_modules\\@scope\\pkg-name-long"] * 12) + "\\index.d.ts"
        self.assertGreater(len(deep), 400)
        out = brp.extended_path(deep)
        self.assertTrue(out.startswith("\\\\?\\C:\\stage\\"))
        self.assertEqual(out[4:], deep)

    def test_fs_path_off_windows_is_plain_absolute(self):
        if brp.IS_WINDOWS:
            self.assertTrue(brp.fs_path("x").startswith("\\\\?\\"))
        else:
            self.assertEqual(brp.fs_path("x"), os.path.join(os.getcwd(), "x"))


class KeyTest(unittest.TestCase):
    def test_rel_key_normalizes_separators(self):
        self.assertEqual(brp.rel_key("node\\node.exe"), "node/node.exe")
        self.assertEqual(brp.rel_key("./srt//a\\b/"), "srt/a/b")
        with self.assertRaises(SystemExit):
            brp.rel_key("a/../../x")

    def test_child_joins_key_parts(self):
        self.assertEqual(brp.child(os.sep + "r", "a/b\\c.txt"), os.path.join(os.sep + "r", "a", "b", "c.txt"))

    def test_required_check_uses_keys(self):
        with tempfile.TemporaryDirectory() as tmp:
            problems = brp.verify_layout(tmp)
            self.assertEqual(problems, [f"missing {rel}" for rel in brp.REQUIRED])


DEEP_PARTS = ["node_modules", "@aws-sdk", "client-sso-oidc-extra-long-name", "node_modules", "@smithy", "types-with-a-long-name"] * 6


def make_prefix(root):
    """A fake npm prefix with one very deep file and one shallow file.

    Text is written with newline="\n" so the bytes are the same on every OS
    (Windows text mode would turn LF into CRLF; the packer keeps bytes as-is).

    The fixture itself is created through brp.fs_path (extended-length on
    Windows), because plain os.makedirs/open hit WinError 206 past MAX_PATH.
    The returned path stays relative: that is what stage/pack must handle.
    """
    deep_dir = os.path.join(root, *DEEP_PARTS, "dist-types", "commands")
    os.makedirs(brp.fs_path(deep_dir))
    deep_file = os.path.join(deep_dir, "GetRoleCredentialsCommand.js")
    with open(brp.fs_path(deep_file), "w", newline="\n") as handle:
        handle.write("module.exports = 1\n")
    with open(brp.fs_path(os.path.join(deep_dir, "GetRoleCredentialsCommand.d.ts")), "w", newline="\n") as handle:
        handle.write("export {}\n")
    with open(brp.fs_path(os.path.join(root, "package.json")), "w", newline="\n") as handle:
        handle.write("{}\n")
    return deep_file


class VersionTest(unittest.TestCase):
    def test_default_version_matches_the_rust_constant(self):
        import re
        launch = os.path.join(brp.CRATE, "src", "launch.rs")
        with open(launch, encoding="utf-8") as handle:
            match = re.search(r'pub const RUNTIME_PACKAGE_VERSION: &str = "([^"]+)";', handle.read())
        self.assertIsNotNone(match, "RUNTIME_PACKAGE_VERSION not found in src/launch.rs")
        self.assertEqual(brp.DEFAULT_VERSION, match.group(1))
        self.assertEqual(brp.DEFAULT_VERSION, "0.1.2")


class StagePackTest(unittest.TestCase):
    def setUp(self):
        self.tmp = tempfile.mkdtemp(prefix="fox-brp-")
        self.cwd = os.getcwd()
        os.chdir(self.tmp)

    def tearDown(self):
        os.chdir(self.cwd)
        # Extended-length path so the >260-char fixture is really removed on
        # Windows; a failure is reported instead of silently leaving debris.
        shutil.rmtree(brp.fs_path(self.tmp))

    def test_stage_and_pack_deep_tree_from_relative_paths(self):
        node = os.path.join("in", "node")
        os.makedirs(node)
        with open(os.path.join(node, "node.exe"), "wb") as handle:
            handle.write(b"MZ")
        srt = os.path.join("in", "srt")
        os.makedirs(os.path.join(srt, "node_modules", "@anthropic-ai", "sandbox-runtime", "dist"))
        with open(os.path.join(srt, "node_modules", "@anthropic-ai", "sandbox-runtime", "dist", "cli.js"), "w", newline="\n") as handle:
            handle.write("// cli\n")
        deep_file = make_prefix(os.path.join("in", "pi"))
        self.assertGreater(len(os.path.abspath(deep_file)), 300, "the fixture must be past MAX_PATH")

        out = "stage"  # relative on purpose
        self.assertEqual(
            brp.main(["stage", out, "--node-dir", node, "--srt-prefix", srt, "--backend", "pi=" + os.path.join("in", "pi"), "--prune-dts"]),
            0,
        )
        keys = brp.list_files(out)
        deep_key = "backends/pi/" + "/".join(DEEP_PARTS) + "/dist-types/commands/GetRoleCredentialsCommand.js"
        self.assertIn(deep_key, keys)
        self.assertNotIn(deep_key[:-3] + ".d.ts", keys, "--prune-dts drops type files")
        self.assertIn("acp-bridge/bridge.mjs", keys)
        self.assertNotIn("acp-bridge/bridge.test.mjs", keys)
        self.assertIn("dsh-patches/read-only.yml", keys)
        # The pi approval gate ships next to each other under pi-gate/.
        self.assertIn("pi-gate/pi-gated.cmd", keys)
        self.assertIn("pi-gate/fox-approval-gate.ts", keys)
        for key, source in brp.PI_GATE_FILES.items():
            with open(brp.child(brp.fs_path(out), key), "rb") as staged, open(source, "rb") as original:
                self.assertEqual(staged.read(), original.read(), key)
        self.assertTrue(all("\\" not in key for key in keys))
        self.assertEqual(brp.verify_layout(out), [])

        # The deep path is over the install budget, so a big budget for the
        # pack itself; the over-budget refusal is checked separately below.
        zip_path = os.path.join("dist", "pkg.zip")
        self.assertEqual(brp.main(["pack", out, "--version", "0.0.1", "--out", zip_path, "--budget", "2000"]), 0)
        with zipfile.ZipFile(zip_path) as archive:
            names = archive.namelist()
            self.assertEqual(sorted(names), keys)
            self.assertEqual(archive.read(deep_key), b"module.exports = 1\n")
        with open(zip_path + ".manifest.json", encoding="utf-8") as handle:
            manifest = json.load(handle)
        self.assertEqual(manifest["fileCount"], len(keys))
        entry = next(item for item in manifest["files"] if item["path"] == deep_key)
        self.assertEqual(entry["sha256"], hashlib.sha256(b"module.exports = 1\n").hexdigest())
        with open(zip_path + ".sha256", encoding="ascii") as handle:
            digest = handle.read().split()[0]
        self.assertEqual(digest, brp.sha256_file(zip_path))
        # check on the zip and on the directory agree; default budget refuses.
        self.assertEqual(brp.main(["check", zip_path, "--version", "0.0.1"]), 1)
        self.assertEqual(brp.main(["check", out, "--version", "0.0.1", "--budget", "2000"]), 0)
        # --version defaults to the runtime version Fox looks for.
        default_zip = os.path.join("dist", "default.zip")
        self.assertEqual(brp.main(["pack", out, "--out", default_zip, "--budget", "2000"]), 0)
        with open(default_zip + ".manifest.json", encoding="utf-8") as handle:
            self.assertEqual(json.load(handle)["version"], brp.DEFAULT_VERSION)
        self.assertEqual(brp.main(["pack", out, "--version", "0.0.1", "--out", os.path.join("dist", "refused.zip")]), 1)

    def test_pack_reports_real_missing_files_only(self):
        os.makedirs(os.path.join("pkg", "acp-bridge"))
        shutil.copyfile(os.path.join(brp.ASSETS, "acp-bridge", "bridge.mjs"), os.path.join("pkg", "acp-bridge", "bridge.mjs"))
        problems = brp.verify_layout("pkg")
        self.assertEqual(
            problems,
            ["missing node/node.exe", f"missing {brp.SRT_CLI}", "missing pi-gate/pi-gated.cmd", "missing pi-gate/fox-approval-gate.ts"],
        )
        with open(os.path.join("pkg", "acp-bridge", "bridge.mjs"), "a", newline="\n") as handle:
            handle.write("// edited\n")
        self.assertIn("acp-bridge/bridge.mjs differs", " ".join(brp.verify_layout("pkg")))

    def test_pi_gate_mismatch_is_refused(self):
        os.makedirs(os.path.join("pkg", "pi-gate"))
        shutil.copyfile(brp.PI_GATE_FILES["pi-gate/pi-gated.cmd"], os.path.join("pkg", "pi-gate", "pi-gated.cmd"))
        with open(os.path.join("pkg", "pi-gate", "fox-approval-gate.ts"), "w", newline="\n") as handle:
            handle.write("// not the gate\n")
        problems = brp.verify_layout("pkg")
        self.assertIn("pi-gate/fox-approval-gate.ts differs from fixtures/pi/fox-approval-gate.ts", problems)
        self.assertNotIn("missing pi-gate/pi-gated.cmd", problems)
        self.assertFalse(any("pi-gated.cmd differs" in problem for problem in problems))

    def test_dsh_patch_mismatch_and_missing(self):
        os.makedirs(os.path.join("pkg", "dsh-patches"))
        with open(os.path.join("pkg", "dsh-patches", "read-only.yml"), "w", newline="\n") as handle:
            handle.write("changed: true\n")
        problems = brp.verify_layout("pkg")
        self.assertIn("dsh-patches/read-only.yml differs from runtime-assets", problems)
        self.assertIn("missing dsh-patches/workspace-write.yml", problems)

    @unittest.skipIf(not hasattr(os, "symlink") or brp.IS_WINDOWS, "symlinks")
    def test_symlinks_refused_in_pack_and_followed_in_stage(self):
        os.makedirs("src")
        with open(os.path.join("src", "real.js"), "w", newline="\n") as handle:
            handle.write("x")
        os.symlink("real.js", os.path.join("src", "link.js"))
        self.assertEqual(brp.copy_tree("src", "dst", False), 2)
        self.assertFalse(os.path.islink(os.path.join("dst", "link.js")))
        with self.assertRaises(SystemExit):
            brp.list_files("src")
        self.assertEqual(brp.list_files("src", skip_links=True), ["real.js"])


if __name__ == "__main__":
    unittest.main()
