#!/usr/bin/env python3
"""Extract all lucide-react icon imports from the app source tree."""
import re
import os
import json
import sys

ROOT = sys.argv[1] if len(sys.argv) > 1 else "."
SRC = os.path.join(ROOT, "apps/desktop/src")

# Match:  import { A, B } from "lucide-react";   and   export { A } from "lucide-react";
# Dotall so multi-line import blocks are captured.
IMPORT_RE = re.compile(
    r"""(?:import|export)\s+(?:type\s+)?\{([^}]*)\}\s+from\s+["']lucide-react["']""",
    re.DOTALL,
)

# Type-only names we should exclude (they aren't icons)
TYPE_NAMES = {"LucideIcon", "LucideProps", "IconNode", "LucideIconName"}

usages = {}  # symbol -> set of files
files_scanned = 0

for dirpath, dirnames, filenames in os.walk(SRC):
    dirnames[:] = [d for d in dirnames if d not in ("node_modules", "dist", ".git")]
    for fn in filenames:
        if not fn.endswith((".ts", ".tsx", ".js", ".jsx")):
            continue
        path = os.path.join(dirpath, fn)
        rel = os.path.relpath(path, ROOT).replace("\\", "/")
        files_scanned += 1
        try:
            text = open(path, encoding="utf-8").read()
        except Exception as e:
            print(f"skip {rel}: {e}", file=sys.stderr)
            continue
        for m in IMPORT_RE.finditer(text):
            body = m.group(1)
            for part in body.split(","):
                part = part.strip()
                if not part:
                    continue
                # handle `X as Y` and `type X`
                part = re.sub(r"^type\s+", "", part).strip()
                if " as " in part:
                    part = part.split(" as ")[0].strip()
                if not part:
                    continue
                if part in TYPE_NAMES:
                    continue
                # only identifiers
                if not re.match(r"^[A-Za-z_$][A-Za-z0-9_$]*$", part):
                    continue
                usages.setdefault(part, set()).add(rel)

result = {k: sorted(v) for k, v in sorted(usages.items())}
out = os.path.join(os.path.dirname(os.path.abspath(__file__)), "lucide_usages.json")
with open(out, "w", encoding="utf-8") as f:
    json.dump(result, f, indent=2, ensure_ascii=False)

print(f"files scanned: {files_scanned}")
print(f"unique imported symbols: {len(result)}")
print(json.dumps({k: len(v) for k, v in list(result.items())[:400]}, indent=0))
