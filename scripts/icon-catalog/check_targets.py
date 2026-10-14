#!/usr/bin/env python3
"""Check the specific icons the user wants to animate against lucide-animated."""
import json, sys

LA_SLUGS, AI_NAMES, FINAL = sys.argv[1:4]
la = {l.strip() for l in open(LA_SLUGS, encoding="utf-8") if l.strip()}
ai = set(json.load(open(AI_NAMES, encoding="utf-8")))
data = json.load(open(FINAL, encoding="utf-8"))
slug = {}
for it in data["lucide"]:
    slug[it["name"]] = it["slug"]        # lucide's own slug, authoritative
for it in data["animated"]:
    slug.setdefault(it["name"], it["slug"])

GROUPS = [
    ("助手 / 知识切换", ["Sparkles", "BookCopy"]),
    ("窗口按钮", ["Minus", "X", "Maximize", "Minimize"]),
    ("知识库导航", ["House", "Files", "Library", "Database", "Orbit",
                    "ListTodo", "Plus", "ChevronRight", "Folder", "FolderOpen",
                    "LoaderCircle"]),
    ("资料库面板", ["Plus", "Database", "LoaderCircle"]),
]

print("%-22s %-16s %-24s %-8s %-8s" % ("icon", "app slug", "lucide-animated", "LA?", "AI?"))
print("-" * 84)
for g, names in GROUPS:
    print("\n## %s" % g)
    for n in names:
        s = slug.get(n, "?")
        print("%-22s %-16s %-24s %-8s %-8s"
              % (n, s, ("has `%s`" % s) if s in la else "(none)", s in la, n in ai))
