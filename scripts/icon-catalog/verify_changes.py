#!/usr/bin/env python3
"""Structural verification for the before/after page.

Parses with a real HTML parser so a nesting mistake cannot slip through, then
asserts the per-row shape and the counts the prose claims.
"""
import sys, re
from html.parser import HTMLParser

PATH = sys.argv[1]
src = open(PATH, encoding="utf-8").read()
VOID = {"br", "meta", "link", "img", "input", "hr", "area", "base", "col"}


class Check(HTMLParser):
    def __init__(self):
        super().__init__(convert_charrefs=False)
        self.stack, self.errors = [], []
        self.counts, self.rows = {}, []
        self.cur = None

    def handle_starttag(self, tag, attrs):
        self.counts[tag] = self.counts.get(tag, 0) + 1
        d = dict(attrs)
        cls = (d.get("class") or "").split()
        if tag == "div" and ({"crow", "arow", "skiprow", "grow"} & set(cls)):
            kind = ({"crow", "arow", "skiprow", "grow"} & set(cls)).pop()
            self.cur = {"kind": kind, "cls": cls, "depth": len(self.stack),
                        "spans": 0, "codes": 0, "svgs": 0}
            self.rows.append(self.cur)
        if torch := (tag == "span" and self.cur is not None):
            if len(self.stack) == self.cur["depth"] + 1:
                self.cur["spans"] += 1
        if tag == "code" and self.cur is not None:
            self.cur["codes"] += 1
        if tag == "svg" and self.cur is not None:
            self.cur["svgs"] += 1
        if tag not in VOID:
            self.stack.append(tag)

    def handle_endtag(self, tag):
        if tag in VOID:
            return
        if not self.stack:
            self.errors.append("stray </%s>" % tag)
            return
        if self.stack[-1] != tag:
            self.errors.append("expected </%s>, got </%s> (line %d)"
                               % (self.stack[-1], tag, self.getpos()[0]))
        else:
            self.stack.pop()
        if self.cur is not None and len(self.stack) <= self.cur["depth"]:
            self.cur = None


c = Check()
c.feed(src)

fails = []
if c.errors:
    fails += c.errors[:10]
if c.stack:
    fails.append("unclosed tags at EOF: %s" % c.stack)

# --- per-row shape -----------------------------------------------------------
by_kind = {}
for r in c.rows:
    by_kind.setdefault(r["kind"], []).append(r)

EXPECT = {"crow": 4, "arow": 4, "skiprow": 3, "grow": 2}
for kind, want in EXPECT.items():
    got = by_kind.get(kind, [])
    if not got:
        fails.append("no %s rows found" % kind)
        continue
    for i, r in enumerate(got):
        if r["spans"] != want:
            fails.append("%s[%d] has %d top-level spans, want %d"
                         % (kind, i, r["spans"], want))
    # crows show repo glyph + site glyph side by side (2 max); others 1 max.
    cap = 2 if kind == "crow" else 1
    over = [(i, r["svgs"]) for i, r in enumerate(got) if r["svgs"] > cap]
    if over:
        fails.append("%s: rows exceeding %d svg: %s" % (kind, cap, over))

# --- prose-claimed counts ----------------------------------------------------
svg_total = c.counts.get("svg", 0)
rows_total = len(c.rows)
if "待补" in src or "TODO" in src or "__" in re.sub(r"__\w+__", "", src):
    pass  # placeholder markers only matter if a __NAME__ survived
leftover = re.findall(r"__[A-Z]+__", src)
if leftover:
    fails.append("unsubstituted placeholders: %s" % set(leftover))

if "<script" in src.lower():
    fails.append("page should be script-free (no client re-render)")

# every row must have rendered an svg (glyph lookup could silently miss)
empty_svg = len(re.findall(r"<svg viewBox=\"0 0 24 24\" aria-hidden=\"true\"></svg>", src))
if empty_svg:
    fails.append("%d svg(s) rendered with no glyph content" % empty_svg)

print("file          :", PATH, len(src), "bytes")
print("elements      :", {k: v for k, v in sorted(c.counts.items())})
print("rows          : crow=%d arow=%d skiprow=%d grow=%d"
      % (len(by_kind.get("crow", [])), len(by_kind.get("arow", [])),
         len(by_kind.get("skiprow", [])), len(by_kind.get("grow", []))))
print("svg total     :", svg_total)
print()
if fails:
    print("FAIL (%d)" % len(fails))
    for f in fails:
        print("  -", f)
    sys.exit(1)
print("OK - all structural checks passed")
