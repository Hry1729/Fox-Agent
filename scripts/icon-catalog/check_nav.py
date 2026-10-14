#!/usr/bin/env python3
"""Resolve the exact icon set the user wants animated.

Lucide ships aliases for the same glyph (House/Home, Files/FileStack?..), so the
roster has to be matched on the alias set, not just the canonical slug.
"""
import json, sys

LA_SLUGS, AI_NAMES = sys.argv[1:3]
la = {l.strip() for l in open(LA_SLUGS, encoding="utf-8") if l.strip()}
ai = set(json.load(open(AI_NAMES, encoding="utf-8")))

# canonical name -> (all lucide slugs for that glyph, animateicons name)
TARGETS = [
    # --- 助手 / 资料 切换 ---
    ("助手/资料切换", "Sparkles",  ["sparkles"],                  "Sparkles"),
    ("助手/资料切换", "BookCopy",  ["book-copy"],                 "BookCopy"),
    # --- 窗口按钮 ---
    ("窗口按钮",      "Minus",     ["minus"],                     "Minus"),
    ("窗口按钮",      "X (关闭)",  ["x"],                         "X"),
    ("窗口按钮",      "Maximize",  ["maximize", "maximize-2"],    "Maximize"),
    ("窗口按钮",      "Minimize",  ["minimize", "minimize-2"],    "Minimize"),
    # --- 知识库导航栏 ---
    ("知识库导航",    "House",     ["house", "home"],             "House"),
    ("知识库导航",    "Files",     ["files", "file-stack"],       "Files"),
    ("知识库导航",    "Library",   ["library"],                   "Library"),
    ("知识库导航",    "Database",  ["database"],                  "Database"),
    ("知识库导航",    "Orbit",     ["orbit"],                     "Orbit"),
    ("知识库导航",    "ListTodo",  ["list-todo"],                 "ListTodo"),
    ("知识库导航",    "Plus",      ["plus"],                      "Plus"),
    ("知识库导航",    "ChevronRight", ["chevron-right"],          "ChevronRight"),
    ("知识库导航",    "Folder",    ["folder"],                    "Folder"),
    ("知识库导航",    "FolderOpen",["folder-open"],               "FolderOpen"),
    ("知识库导航",    "LoaderCircle", ["loader-circle", "loader-2"], "LoaderCircle"),
]

print("%-14s %-14s %-28s %-6s %-6s" % ("group", "icon", "lucide-animated slug", "LA", "AI"))
print("-" * 78)
miss_la, miss_ai = [], []
for g, name, slugs, ainame in TARGETS:
    hit = next((s for s in slugs if s in la), None)
    inai = ainame in ai
    if not hit: miss_la.append(name)
    if not inai: miss_ai.append(name)
    print("%-14s %-14s %-28s %-6s %-6s"
          % (g, name, hit or "(none)", "yes" if hit else "NO", "yes" if inai else "NO"))

print()
print("lucide-animated missing (%d): %s" % (len(miss_la), ", ".join(miss_la)))
print("animateicons   missing (%d): %s" % (len(miss_ai), ", ".join(miss_ai)))

# what IS available near the missing ones
print()
print("nearest available in lucide-animated:")
for probe, alts in [("Files", ["folders", "file-stack", "files"]),
                    ("Library", ["book-text", "library"]),
                    ("Database", ["database-backup", "database"]),
                    ("Orbit", ["atom", "route", "orbit"]),
                    ("ListTodo", ["clipboard-check", "list-todo"]),
                    ("BookCopy", ["book-text", "bookmark", "book-copy"]),
                    ("Minus", ["plus", "minus"])]:
    got = [a for a in alts if a in la]
    print("  %-12s -> %s" % (probe, ", ".join(got) if got else "(nothing close)"))
