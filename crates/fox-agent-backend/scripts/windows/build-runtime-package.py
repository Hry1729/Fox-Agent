#!/usr/bin/env python3
"""Fox Windows sandbox runtime package: path budget, packing, SHA256, manifest.

Runs on any OS (Linux CI included); paths are judged as they will be on
Windows: ``<install root>\\<version>\\<relative path with backslashes>``.

  check <dir|zip> --version V [--budget 240] [--install-root ...]
      Fail (exit 1) when any file's installed path is >= budget characters,
      listing the offenders. Also checked: the enable script's staging path
      (``%ProgramFiles%\\Fox\\.s<8 hex>\\c\\...``).

  stage <out-dir> [--node-dir D] [--srt-prefix P] [--backend NAME=PREFIX ...]
      Assemble a package directory: copies the given Node directory to node\\,
      the srt npm prefix to srt\\, each backend prefix to backends\\<NAME>\\, and
      Fox's runtime assets (acp-bridge\\bridge.mjs, dsh-patches\\*.yml) from this
      crate. Does not download anything (prefixes come from
      ``npm install --prefix <p> --os=win32 --cpu=x64 <pkg>@<version>``).

  pack <dir> --version V --out <zip> [--budget 240]
      Verify the layout (node\\node.exe, srt cli.js, acp-bridge\\bridge.mjs equal
      to this crate's copy, dsh-patches equal if present), run the budget
      check, write a deterministic zip, <zip>.sha256 and <zip>.manifest.json
      (every file with size and SHA256, longest installed path).

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
import os
import shutil
import sys
import zipfile

CRATE = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
ASSETS = os.path.join(CRATE, "runtime-assets")
DEFAULT_INSTALL_ROOT = r"C:\Program Files\Fox\sandbox-runtime"
# enable-sandbox.ps1 extracts to %ProgramFiles%\Fox\.s<8 hex>\c first.
STAGING_PREFIX = r"C:\Program Files\Fox\.s00000000\c"
DEFAULT_BUDGET = 240
SRT_CLI = "srt/node_modules/@anthropic-ai/sandbox-runtime/dist/cli.js"
REQUIRED = ["node/node.exe", SRT_CLI, "acp-bridge/bridge.mjs"]
FIXED_TIME = (2026, 1, 1, 0, 0, 0)


def list_files(source, skip_links=False):
    """Relative POSIX paths of every regular file in a directory or zip.
    Symlinks are refused (a Windows package has none; npm on Windows writes
    .cmd shims) unless ``skip_links`` (check on a Linux-installed tree)."""
    if os.path.isfile(source) and zipfile.is_zipfile(source):
        with zipfile.ZipFile(source) as archive:
            return sorted(name.rstrip("/") for name in archive.namelist() if not name.endswith("/"))
    out = []
    for dirpath, dirnames, filenames in os.walk(source):
        dirnames.sort()
        for name in sorted(filenames):
            full = os.path.join(dirpath, name)
            if os.path.islink(full):
                if skip_links:
                    continue
                raise SystemExit(f"symlink in package (not allowed): {full}")
            out.append(os.path.relpath(full, source).replace(os.sep, "/"))
    return out


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


def verify_layout(directory):
    problems = [f"missing {rel}" for rel in REQUIRED if not os.path.isfile(os.path.join(directory, rel))]
    bridge = os.path.join(directory, "acp-bridge", "bridge.mjs")
    if os.path.isfile(bridge):
        with open(bridge, "rb") as a, open(os.path.join(ASSETS, "acp-bridge", "bridge.mjs"), "rb") as b:
            if norm_text(a.read()) != norm_text(b.read()):
                problems.append("acp-bridge/bridge.mjs differs from runtime-assets/acp-bridge/bridge.mjs (Fox refuses to start)")
    patches = os.path.join(directory, "dsh-patches")
    if os.path.isdir(patches):
        for name in sorted(os.listdir(os.path.join(ASSETS, "dsh-patches"))):
            mine = os.path.join(patches, name)
            if not os.path.isfile(mine):
                problems.append(f"missing dsh-patches/{name}")
                continue
            with open(mine, "rb") as a, open(os.path.join(ASSETS, "dsh-patches", name), "rb") as b:
                if norm_text(a.read()) != norm_text(b.read()):
                    problems.append(f"dsh-patches/{name} differs from runtime-assets")
    return problems


def cmd_check(args):
    report = budget_report(list_files(args.source, skip_links=True), args.version, args.install_root, args.budget)
    print_report(report)
    if args.json:
        with open(args.json, "w", encoding="utf-8") as handle:
            json.dump(report, handle, ensure_ascii=False, indent=2)
    return 1 if report["overBudget"] else 0


def copy_tree(src, dst, prune_dts):
    def ignore(_dir, names):
        if not prune_dts:
            return []
        return [n for n in names if n.endswith((".d.ts", ".d.ts.map", ".d.mts", ".d.cts", ".map"))]
    shutil.copytree(src, dst, symlinks=False, ignore=ignore, dirs_exist_ok=True)


def cmd_stage(args):
    out = args.out
    os.makedirs(out, exist_ok=True)
    if args.node_dir:
        copy_tree(args.node_dir, os.path.join(out, "node"), False)
    if args.srt_prefix:
        copy_tree(args.srt_prefix, os.path.join(out, "srt"), args.prune_dts)
    for spec in args.backend or []:
        name, _, prefix = spec.partition("=")
        if not name or not prefix:
            raise SystemExit(f"--backend expects NAME=PREFIX, got {spec!r}")
        copy_tree(prefix, os.path.join(out, "backends", name), args.prune_dts)
    copy_tree(os.path.join(ASSETS, "acp-bridge"), os.path.join(out, "acp-bridge"), False)
    test_file = os.path.join(out, "acp-bridge", "bridge.test.mjs")
    if os.path.exists(test_file):
        os.remove(test_file)
    copy_tree(os.path.join(ASSETS, "dsh-patches"), os.path.join(out, "dsh-patches"), False)
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
    tmp = args.out + ".tmp"
    with zipfile.ZipFile(tmp, "w", compression=zipfile.ZIP_DEFLATED, compresslevel=9) as archive:
        for rel in files:
            full = os.path.join(args.source, rel.replace("/", os.sep))
            info = zipfile.ZipInfo(rel, date_time=FIXED_TIME)
            info.compress_type = zipfile.ZIP_DEFLATED
            info.external_attr = 0o644 << 16
            with open(full, "rb") as handle:
                data = handle.read()
            archive.writestr(info, data)
            manifest_files.append({"path": rel, "size": len(data), "sha256": hashlib.sha256(data).hexdigest()})
    os.replace(tmp, args.out)
    digest = sha256_file(args.out)
    with open(args.out + ".sha256", "w", encoding="ascii") as handle:
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
    with open(args.out + ".manifest.json", "w", encoding="utf-8") as handle:
        json.dump(manifest, handle, ensure_ascii=False, indent=1)
    print(f"package: {args.out}")
    print(f"sha256:  {digest}")
    return 0


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    check = sub.add_parser("check")
    check.add_argument("source")
    check.add_argument("--version", required=True)
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
    pack.add_argument("--version", required=True)
    pack.add_argument("--out", required=True)
    pack.add_argument("--budget", type=int, default=DEFAULT_BUDGET)
    pack.add_argument("--install-root", default=DEFAULT_INSTALL_ROOT)
    args = parser.parse_args(argv)
    if getattr(args, "version", None) is not None and not all(c.isalnum() or c in "._-" for c in args.version):
        raise SystemExit("version may only contain [0-9A-Za-z._-]")
    return {"check": cmd_check, "stage": cmd_stage, "pack": cmd_pack}[args.command](args)


if __name__ == "__main__":
    sys.exit(main())
