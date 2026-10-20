#!/usr/bin/env python3
"""Fox Windows sandbox runtime package: path budget, packing, SHA256, manifest.

Runs on any OS (Linux CI included); paths are judged as they will be on
Windows: ``<install root>\\<version>\\<relative path with backslashes>``.

  check <dir|zip> [--version V] [--budget 240] [--install-root ...]
      Fail (exit 1) when any file's installed path is >= budget characters,
      listing the offenders. Also checked: the enable script's staging path
      (``%ProgramFiles%\\Fox\\.s<8 hex>\\c\\...``).

  stage <out-dir> [--node-dir D] [--srt-prefix P] [--backend NAME=PREFIX ...]
      Assemble a package directory: copies the given Node directory to node\\,
      the srt npm prefix to srt\\, each backend prefix to backends\\<NAME>\\, and
      Fox's runtime assets from this crate: acp-bridge\\bridge.mjs,
      dsh-patches\\*.yml and the pi approval gate pi-gate\\{pi-gated.cmd,
      fox-approval-gate.ts} (from fixtures/pi/; Fox launches pi gated only when
      <runtime>\\pi-gate\\pi-gated.cmd exists, otherwise stock pi without Host
      approvals). Does not download anything (prefixes come from
      ``npm install --prefix <p> --os=win32 --cpu=x64 <pkg>@<version>``).

  pack <dir> [--version V] --out <zip> [--budget 240]
      Verify the layout (node\\node.exe, srt cli.js, acp-bridge\\bridge.mjs and
      pi-gate\\{pi-gated.cmd,fox-approval-gate.ts} equal to this crate's copies,
      dsh-patches equal if present), run the budget
      check, write a deterministic zip, <zip>.sha256 and <zip>.manifest.json
      (every file with size and SHA256, longest installed path).

Long paths on Windows (round 6): every file the script reads or writes goes
through ``fs_path``: on Windows an extended-length path (``\\\\?\\`` + absolute
path with backslashes; a UNC path becomes ``\\\\?\\UNC\\server\\share\\...``), so
deep npm trees copy and pack without a short-path junction and without the
260-character MAX_PATH limit; elsewhere the plain absolute path. Inside the
script a file is known only by its package-relative key with forward slashes
(``rel_key``): the required-file check, the dsh-patch comparison, the budget
report and the zip entry names all use that key, never a joined OS path.

Shortening paths that are over budget (DeepSeek's inspector and the AWS SDK
type trees under pi are the longest): ``--prune-dts`` on stage drops
``*.d.ts`` / ``*.d.ts.map`` / ``*.map`` (not needed at run time); beyond that,
bundle the offending package with esbuild into a single file, or install it
flat (``npm install --install-strategy=shallow``) so nested ``node_modules``
levels disappear. See docs/sandbox.md 7.5.
"""
import argparse
import hashlib
import json
import ntpath
import os
import shutil
import sys
import zipfile

CRATE = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ASSETS = os.path.join(CRATE, "runtime-assets")
PI_FIXTURES = os.path.join(CRATE, "fixtures", "pi")
# Must equal fox_agent_backend::launch::RUNTIME_PACKAGE_VERSION (the
# directory Fox looks for under the install root); a unit test checks it.
DEFAULT_VERSION = "0.1.2"
# Package key -> source file in this crate. Each is copied by `stage` and must
# be byte-equal (modulo CRLF) in `pack`.
PI_GATE_FILES = {
    "pi-gate/pi-gated.cmd": os.path.join(PI_FIXTURES, "pi-gated.cmd"),
    "pi-gate/fox-approval-gate.ts": os.path.join(PI_FIXTURES, "fox-approval-gate.ts"),
}
DEFAULT_INSTALL_ROOT = r"C:\Program Files\Fox\sandbox-runtime"
# enable-sandbox.ps1 extracts to %ProgramFiles%\Fox\.s<8 hex>\c first.
STAGING_PREFIX = r"C:\Program Files\Fox\.s00000000\c"
DEFAULT_BUDGET = 240
SRT_CLI = "srt/node_modules/@anthropic-ai/sandbox-runtime/dist/cli.js"
REQUIRED = ["node/node.exe", SRT_CLI, "acp-bridge/bridge.mjs", *PI_GATE_FILES]
FIXED_TIME = (2026, 1, 1, 0, 0, 0)


IS_WINDOWS = os.name == "nt"
EXTENDED_PREFIX = "\\\\?\\"
EXTENDED_UNC_PREFIX = "\\\\?\\UNC\\"


def extended_path(path, cwd=None):
    r"""Windows extended-length form of ``path`` (pure string logic, testable
    on any OS). Relative paths are made absolute against ``cwd`` (default:
    the current directory); ``/`` becomes ``\`` and ``.`` / ``..`` are
    resolved, because Windows does no normalization after ``\\?\``. UNC
    ``\\server\share\x`` becomes ``\\?\UNC\server\share\x``; an already
    extended path only gets its separators fixed."""
    text = os.fspath(path)
    if text.startswith(("\\\\?\\", "//?/")):
        rest = text[4:].replace("/", "\\")
        if rest[:4].upper() == "UNC\\":
            return EXTENDED_UNC_PREFIX + ntpath.normpath(rest[4:]).lstrip("\\")
        return EXTENDED_PREFIX + ntpath.normpath(rest)
    text = text.replace("/", "\\")
    if not ntpath.isabs(text) or text.startswith("\\") and not text.startswith("\\\\"):
        base = (cwd if cwd is not None else os.getcwd()).replace("/", "\\")
        text = ntpath.join(base, text)
    text = ntpath.normpath(text)
    if text.startswith("\\\\"):
        return EXTENDED_UNC_PREFIX + text[2:]
    return EXTENDED_PREFIX + text


def fs_path(path):
    """The path to hand to the OS for any read or write."""
    if IS_WINDOWS:
        return extended_path(path)
    return os.path.abspath(os.fspath(path))


def rel_key(relative):
    """Package-relative key: forward slashes, no leading ``./`` or slash."""
    parts = [part for part in os.fspath(relative).replace("\\", "/").split("/") if part not in ("", ".")]
    if any(part == ".." for part in parts):
        raise SystemExit(f"path escapes the package: {relative!r}")
    return "/".join(parts)


def child(root_fs, key):
    """OS path of package key ``key`` under an ``fs_path`` root."""
    return os.path.join(root_fs, *rel_key(key).split("/"))


def walk_files(root_fs, follow_links=False, skip_links=False):
    """(key, OS path) of every regular file under ``root_fs``, sorted by key.
    Keys are built from the walk's own components, not with relpath, so an
    extended-length root never meets a plain one."""
    out = []
    for dirpath, dirnames, filenames in os.walk(root_fs, followlinks=follow_links):
        dirnames.sort()
        tail = dirpath[len(root_fs):]
        for name in sorted(filenames):
            full = os.path.join(dirpath, name)
            if os.path.islink(full) and not follow_links:
                if skip_links:
                    continue
                raise SystemExit(f"symlink in package (not allowed): {full}")
            if follow_links and not os.path.isfile(full):
                raise SystemExit(f"dangling link: {full}")
            out.append((rel_key(tail + os.sep + name), full))
    out.sort()
    return out


def list_files(source, skip_links=False):
    """Package keys of every regular file in a directory or zip.
    Symlinks are refused (a Windows package has none; npm on Windows writes
    .cmd shims) unless ``skip_links`` (check on a Linux-installed tree)."""
    source_fs = fs_path(source)
    if os.path.isfile(source_fs) and zipfile.is_zipfile(source_fs):
        with zipfile.ZipFile(source_fs) as archive:
            return sorted(rel_key(name) for name in archive.namelist() if not name.endswith("/"))
    return [key for key, _ in walk_files(source_fs, skip_links=skip_links)]


def installed_path(prefix, relative):
    return prefix.rstrip("\\") + "\\" + relative.replace("/", "\\")


def budget_report(files, version, install_root, budget):
    prefixes = [installed_path(install_root, version), STAGING_PREFIX]
    rows = []
    for rel in files:
        length = max(len(installed_path(prefix, rel)) for prefix in prefixes)
        rows.append((length, rel))
    rows.sort(reverse=True)
    over = [(length, rel) for length, rel in rows if length >= budget]
    return {
        "installRoot": installed_path(install_root, version),
        "budget": budget,
        "files": len(rows),
        "maxLength": rows[0][0] if rows else 0,
        "longest": [{"length": length, "path": rel} for length, rel in rows[:10]],
        "overBudget": [{"length": length, "path": rel} for length, rel in over],
    }


def print_report(report):
    print(f"install root: {report['installRoot']}")
    print(f"files: {report['files']}  longest installed path: {report['maxLength']}  budget: < {report['budget']}")
    if report["overBudget"]:
        print(f"OVER BUDGET: {len(report['overBudget'])} file(s)")
        for row in report["overBudget"][:50]:
            print(f"  {row['length']:4d}  {row['path']}")
        if len(report["overBudget"]) > 50:
            print(f"  ... and {len(report['overBudget']) - 50} more")
        print("shorten: stage --prune-dts; bundle the package with esbuild; or npm --install-strategy=shallow (docs/sandbox.md 7.5)")
    else:
        for row in report["longest"][:5]:
            print(f"  {row['length']:4d}  {row['path']}")


def sha256_file(path):
    digest = hashlib.sha256()
    with open(path, "rb") as handle:
        for chunk in iter(lambda: handle.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def norm_text(data):
    return data.replace(b"\r\n", b"\n")


def read_bytes(path_fs):
    with open(path_fs, "rb") as handle:
        return handle.read()


def verify_layout(directory):
    """Problems with a staged package, judged by package keys (forward
    slashes) so separators can never cause a false "missing"."""
    root = fs_path(directory)
    keys = set(list_files(directory)) if os.path.isdir(root) else set()
    problems = [f"missing {rel}" for rel in REQUIRED if rel_key(rel) not in keys]
    if "acp-bridge/bridge.mjs" in keys:
        if norm_text(read_bytes(child(root, "acp-bridge/bridge.mjs"))) != norm_text(read_bytes(fs_path(os.path.join(ASSETS, "acp-bridge", "bridge.mjs")))):
            problems.append("acp-bridge/bridge.mjs differs from runtime-assets/acp-bridge/bridge.mjs (Fox refuses to start)")
    for key, source in PI_GATE_FILES.items():
        if key in keys and norm_text(read_bytes(child(root, key))) != norm_text(read_bytes(fs_path(source))):
            problems.append(f"{key} differs from fixtures/pi/{os.path.basename(source)}")
    if any(key.startswith("dsh-patches/") for key in keys):
        for name in sorted(os.listdir(fs_path(os.path.join(ASSETS, "dsh-patches")))):
            key = f"dsh-patches/{name}"
            if key not in keys:
                problems.append(f"missing {key}")
                continue
            if norm_text(read_bytes(child(root, key))) != norm_text(read_bytes(fs_path(os.path.join(ASSETS, "dsh-patches", name)))):
                problems.append(f"{key} differs from runtime-assets")
    return problems


def cmd_check(args):
    report = budget_report(list_files(args.source, skip_links=True), args.version, args.install_root, args.budget)
    print_report(report)
    if args.json:
        with open(fs_path(args.json), "w", encoding="utf-8") as handle:
            json.dump(report, handle, ensure_ascii=False, indent=2)
    return 1 if report["overBudget"] else 0


PRUNED_SUFFIXES = (".d.ts", ".d.ts.map", ".d.mts", ".d.cts", ".map")


def copy_tree(src, dst, prune_dts, skip=()):
    """Copy every file under ``src`` to ``dst`` (links followed, like
    ``shutil.copytree(symlinks=False)``), both through ``fs_path``. Returns
    the number of files copied."""
    src_fs, dst_fs = fs_path(src), fs_path(dst)
    if not os.path.isdir(src_fs):
        raise SystemExit(f"not a directory: {src}")
    count = 0
    for key, full in walk_files(src_fs, follow_links=True):
        if key in skip or prune_dts and key.endswith(PRUNED_SUFFIXES):
            continue
        target = child(dst_fs, key)
        os.makedirs(os.path.dirname(target), exist_ok=True)
        shutil.copyfile(full, target)
        shutil.copymode(full, target)
        count += 1
    return count


def cmd_stage(args):
    out = args.out
    os.makedirs(fs_path(out), exist_ok=True)
    if args.node_dir:
        copy_tree(args.node_dir, os.path.join(out, "node"), False)
    if args.srt_prefix:
        copy_tree(args.srt_prefix, os.path.join(out, "srt"), args.prune_dts)
    for spec in args.backend or []:
        name, _, prefix = spec.partition("=")
        if not name or not prefix:
            raise SystemExit(f"--backend expects NAME=PREFIX, got {spec!r}")
        copy_tree(prefix, os.path.join(out, "backends", name), args.prune_dts)
    copy_tree(os.path.join(ASSETS, "acp-bridge"), os.path.join(out, "acp-bridge"), False, skip=("bridge.test.mjs",))
    test_file = child(fs_path(out), "acp-bridge/bridge.test.mjs")
    if os.path.exists(test_file):
        os.remove(test_file)
    copy_tree(os.path.join(ASSETS, "dsh-patches"), os.path.join(out, "dsh-patches"), False)
    for key, source in PI_GATE_FILES.items():
        target = child(fs_path(out), key)
        os.makedirs(os.path.dirname(target), exist_ok=True)
        shutil.copyfile(fs_path(source), target)
    print(f"staged {out}")
    return 0


def cmd_pack(args):
    problems = verify_layout(args.source)
    if problems:
        for problem in problems:
            print(f"layout: {problem}")
        return 1
    files = list_files(args.source)
    report = budget_report(files, args.version, args.install_root, args.budget)
    print_report(report)
    if report["overBudget"]:
        return 1
    manifest_files = []
    source_fs = fs_path(args.source)
    out_fs = fs_path(args.out)
    os.makedirs(os.path.dirname(out_fs), exist_ok=True)
    tmp = out_fs + ".tmp"
    with zipfile.ZipFile(tmp, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for rel in files:
            full = child(source_fs, rel)
            info = zipfile.ZipInfo(rel_key(rel), date_time=FIXED_TIME)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            with open(full, "rb") as handle:
                data = handle.read()
            archive.writestr(info, data)
            manifest_files.append({"path": rel, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    os.replace(tmp, out_fs)
    digest = sha256_file(out_fs)
    with open(out_fs + ".sha256", "w", encoding="ascii") as handle:
        handle.write(f"{digest}  {os.path.basename(args.out)}\n")
    manifest = {
        "version": args.version,
        "sha256": digest,
        "installRoot": report["installRoot"],
        "budget": args.budget,
        "maxInstalledPathLength": report["maxLength"],
        "fileCount": len(manifest_files),
        "files": manifest_files,
    }
    with open(out_fs + ".manifest.json", "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, ensure_ascii=False, indent=1)
    print(f"package: {args.out}")
    print(f"sha256:  {digest}")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    check = sub.add_parser("check")
    check.add_argument("source")
    check.add_argument("--version", default=DEFAULT_VERSION)
    check.add_argument("--budget", type=int, default=DEFAULT_BUDGET)
    check.add_argument("--install-root", default=DEFAULT_INSTALL_ROOT)
    check.add_argument("--json")
    stage = sub.add_parser("stage")
    stage.add_argument("out")
    stage.add_argument("--node-dir")
    stage.add_argument("--srt-prefix")
    stage.add_argument("--backend", action="append")
    stage.add_argument("--prune-dts", action="store_true")
    pack = sub.add_parser("pack")
    pack.add_argument("source")
    pack.add_argument("--version", default=DEFAULT_VERSION)
    pack.add_argument("--out", required=True)
    pack.add_argument("--budget", type=int, default=DEFAULT_BUDGET)
    pack.add_argument("--install-root", default=DEFAULT_INSTALL_ROOT)
    args = parser.parse_args(argv)
    if getattr(args, "version", None) is not None and not all(c.isalnum() or c in "._-" for c in args.version):
        raise SystemExit("version may only contain [0-9A-Za-z._-]")
    return {"check": cmd_check, "stage": cmd_stage, "pack": cmd_pack}[args.command](args)


if __name__ == "__main__":
    sys.exit(main())
