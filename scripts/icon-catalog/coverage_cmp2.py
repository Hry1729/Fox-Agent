#!/usr/bin/env python3
"""Accurate coverage: lucide-animated.com vs @animateicons/react vs Fox's 175 icons.

Handles two classes of naming noise:
  1. kebab conversion must split before digits (Maximize2 -> maximize-2)
  2. lucide renamed icons over time; lucide-animated tracks some new names, so
     map our canonical names onto whichever slug the site actually uses.
"""
import json, sys

FINAL, AI_NAMES, LA_SLUGS = sys.argv[1:4]

data = json.load(open(FINAL, encoding="utf-8"))
app = {}
for it in data["lucide"]:
    app[it["name"]] = it["slug"]
for it in data["animated"]:
    app.setdefault(it["name"], it["slug"])

def kebab(name):
    out = []
    for i, ch in enumerate(name):
        if ch.isupper():
            if i and (not name[i - 1].isupper() or (i + 1 < len(name) and name[i + 1].islower())):
                out.append("-")
            out.append(ch.lower())
        elif ch.isdigit() and out and out[-1] != "-" and not out[-1].isdigit():
            out.append("-"); out.append(ch)
        else:
            out.append(ch)
    return "".join(out)

# lucide canonical name -> slug used by lucide-animated.com
RENAME = {
    "Trash2": "delete",              # identical geometry, LA calls it Delete
    "CircleQuestionMark": "circle-help",
    "Volume2": "volume",
    "FlaskConical": "flask",
    "LogIn": "login",
    "LogOut": "logout",
}

ai = set(json.load(open(AI_NAMES, encoding="utf-8")))
la = {l.strip() for l in open(LA_SLUGS, encoding="utf-8") if l.strip()}

rows = []
for name in sorted(app, key=str.lower):
    slug = app[name] or kebab(name)
    la_slug = RENAME.get(name, slug)
    rows.append({
        "name": name, "slug": slug,
        "la": la_slug in la, "ai": name in ai,
    })

def names(pred): return [r["name"] for r in rows if pred(r)]

c = {
    "app": len(rows),
    "la": len(names(lambda r: r["la"])),
    "ai": len(names(lambda r: r["ai"])),
    "both": len(names(lambda r: r["la"] and r["ai"])),
    "onlyLA": len(names(lambda r: r["la"] and not r["ai"])),
    "onlyAI": len(names(lambda r: r["ai"] and not r["la"])),
    "neither": len(names(lambda r: not r["la"] and not r["ai"])),
}
print(json.dumps({
    "counts": c,
    "onlyLucideAnimated": names(lambda r: r["la"] and not r["ai"]),
    "onlyAnimateicons": names(lambda r: r["ai"] and not r["la"]),
    "neither": names(lambda r: not r["la"] and not r["ai"]),
    "lucideAnimatedTotal": len(la),
    "animateiconsTotal": len(ai),
    # the 22 icons the report recommends animating
    "recommended22": {
        n: {"la": r["la"], "ai": r["ai"]}
        for n in ["BellRing","LoaderCircle","RefreshCw","RotateCcw","Send","Sparkles",
                  "WandSparkles","Trash2","CircleStop","CirclePause","Play","Pause",
                  "Mic","Eye","EyeOff","ThumbsUp","ThumbsDown","Zap","Wrench",
                  "FolderPlus","MessageCircleMore","Target"]
        for r in [next(x for x in rows if x["name"] == n)]
    },
    # the 11 in-repo animated icons
    "existing11": {
        n: {"la": r["la"], "ai": r["ai"]}
        for n in ["Bell","Blocks","MessageCirclePlus","PanelLeft","PanelRight",
                  "Search","Settings","SunMoon","Users","Maximize","Minimize"]
        for r in [next(x for x in rows if x["name"] == n)]
    },
}, ensure_ascii=False, indent=1))
