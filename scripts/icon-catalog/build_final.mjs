import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import * as L from "lucide-react";
import { readFileSync, writeFileSync, readdirSync } from "node:fs";

const OUT = process.argv[2];
const USAGES = process.argv[3];
const BASE = process.argv[4]; // .../apps/desktop/src

const usages = JSON.parse(readFileSync(USAGES, "utf8"));
const TYPE_ONLY = new Set(["LucideIcon", "LucideProps", "IconNode"]);

const rows = [];
const unresolved = [];

for (const symbol of Object.keys(usages)) {
  if (TYPE_ONLY.has(symbol)) continue;
  const Comp = L[symbol];
  if (!Comp || typeof Comp !== "object" || !Comp.render) {
    unresolved.push(symbol);
    continue;
  }
  const displayName = Comp.render.displayName || symbol;
  const markup = renderToStaticMarkup(createElement(Comp, { size: 24, strokeWidth: 1.75 }));
  const canon = markup.match(/lucide-([a-z0-9-]+?)(?:\s|")/);
  const canonical = canon ? canon[1] : symbol;
  const inner = markup.replace(/^<svg[^>]*>/, "").replace(/<\/svg>$/, "");
  rows.push({ import: symbol, name: displayName, canonical, inner, files: usages[symbol] });
}

const byCanon = new Map();
for (const r of rows) {
  if (!byCanon.has(r.canonical)) {
    byCanon.set(r.canonical, { canonical: r.canonical, inner: r.inner, names: new Set(), imports: new Set(), files: new Set() });
  }
  const g = byCanon.get(r.canonical);
  g.names.add(r.name);
  g.imports.add(r.import);
  r.files.forEach((f) => g.files.add(f));
}

const lucide = [...byCanon.values()].map((g) => ({
  slug: g.canonical,
  name: [...g.names].sort()[0],
  aliases: [...g.imports].sort(),
  inner: g.inner,
  files: [...g.files].sort(),
  kind: "lucide",
})).sort((a, b) => a.name.localeCompare(b.name));

// Animated (animate-ui) icons shipped in the repo. They are lucide glyphs
// driven by motion variants, so we render the matching lucide glyph and flag
// the tile as animated rather than re-parsing the motion JSX by hand.
const animDir = `${BASE}/components/animate-ui/icons`;
const ANIMATED = new Map();
for (const f of readdirSync(animDir)) {
  if (!f.endsWith(".tsx") || f === "icon.tsx") continue;
  const slug = f.replace(/\.tsx$/, "");
  const pascal = slug.split("-").map((s) => s[0].toUpperCase() + s.slice(1)).join("");
  const Comp = L[pascal];
  let inner = "";
  if (Comp && typeof Comp === "object" && Comp.render) {
    const markup = renderToStaticMarkup(createElement(Comp, { size: 24, strokeWidth: 1.75 }));
    inner = markup.replace(/^<svg[^>]*>/, "").replace(/<\/svg>$/, "");
  }
  ANIMATED.set(pascal, {
    slug,
    name: pascal,
    aliases: [pascal],
    inner,
    source: `components/animate-ui/icons/${f}`,
    files: [],
    kind: "animated",
  });
}

// Map animated icons to lucide equivalents for the pair
const animated = [...ANIMATED.values()].sort((a, b) => a.name.localeCompare(b.name));

writeFileSync(OUT, JSON.stringify({
  lucide,
  animated,
  unresolved,
  rawSymbols: Object.keys(usages).length,
  sourceFiles: 376,
}, null, 2));

console.log("lucide tiles:", lucide.length);
console.log("animated tiles:", animated.length);
console.log("unresolved:", unresolved.join(","));
console.log("animated empty inner:", animated.filter((a) => !a.inner).map((a) => a.slug));
