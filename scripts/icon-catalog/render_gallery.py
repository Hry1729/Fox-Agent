#!/usr/bin/env python3
"""Render the Fox icon gallery HTML from the extracted icon data."""
import json
import sys

DATA = sys.argv[1]
OUT = sys.argv[2]

with open(DATA, encoding="utf-8") as f:
    d = json.load(f)

lucide = d["lucide"]
animated = d["animated"]

# Two sections: lucide grid, then the repo's animated icons.
lucide_tiles = [{
    "kind": "lucide",
    "slug": i["slug"],
    "name": i["name"],
    "aliases": i["aliases"],
    "inner": i["inner"],
} for i in lucide]
lucide_tiles.sort(key=lambda t: t["name"].lower())

anim_tiles = [{
    "kind": "animated",
    "slug": a["slug"],
    "name": a["name"],
    "aliases": a["aliases"],
    "inner": a["inner"],
} for a in animated]
anim_tiles.sort(key=lambda t: t["name"].lower())

# Icons that exist in both sets, so the lucide tile can point at its animated twin
anim_slugs = {t["slug"] for t in anim_tiles}
for t in lucide_tiles:
    t["animated"] = t["slug"] in anim_slugs

payload = json.dumps({"lucide": lucide_tiles, "animated": anim_tiles}, ensure_ascii=False)
n_lucide = len(lucide_tiles)
n_anim = len(anim_tiles)
n_total = n_lucide + n_anim

HTML = """<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Fox 图标总览</title>
<style>
  :root {
    color-scheme: light;
    --paper: #f4f1ea;
    --paper-raise: #faf8f3;
    --ink: #26241f;
    --ink-soft: #5c574d;
    --ink-faint: #a09a8d;
    --rule: #ded8ca;
    --dot: #ddd7c8;
    --accent: #1f5fd0;
    --accent-soft: #e8edf7;
    --sans: "HarmonyOS Sans SC", "Segoe UI Variable", "Segoe UI", system-ui, sans-serif;
    --mono: ui-monospace, "Cascadia Code", Consolas, "SF Mono", Menlo, monospace;
  }

  * { box-sizing: border-box; }

  html { -webkit-text-size-adjust: 100%; }

  body {
    margin: 0;
    font-family: var(--sans);
    color: var(--ink);
    background-color: var(--paper);
    background-image: radial-gradient(var(--dot) 0.8px, transparent 0.8px);
    background-size: 18px 18px;
    background-position: -1px -1px;
    line-height: 1.5;
  }

  .shell { max-width: 1180px; margin: 0 auto; padding: 0 24px 96px; }

  /* ---------- header ---------- */
  header { padding: 56px 0 22px; }

  h1 {
    margin: 0;
    font-size: 30px;
    font-weight: 600;
    letter-spacing: -0.015em;
    line-height: 1.15;
  }

  .lede {
    margin: 8px 0 0;
    font-size: 14.5px;
    color: var(--ink-soft);
    max-width: 62ch;
  }

  .lede b { font-weight: 600; color: var(--ink); }

  /* ---------- controls ---------- */
  .controls {
    position: sticky;
    top: 0;
    z-index: 20;
    display: flex;
    align-items: center;
    gap: 12px;
    flex-wrap: wrap;
    padding: 14px 0;
    margin-bottom: 20px;
    background: linear-gradient(var(--paper) 72%, transparent);
    border-bottom: 1px solid var(--rule);
  }

  .search {
    position: relative;
    flex: 1 1 260px;
    min-width: 210px;
  }

  .search svg {
    position: absolute;
    left: 12px;
    top: 50%;
    transform: translateY(-50%);
    width: 15px; height: 15px;
    stroke: var(--ink-faint);
    fill: none;
    stroke-width: 2;
    pointer-events: none;
  }

  .search input {
    width: 100%;
    font: inherit;
    font-size: 14px;
    color: var(--ink);
    padding: 9px 34px 9px 35px;
    border: 1px solid var(--rule);
    border-radius: 8px;
    background: var(--paper-raise);
    outline: none;
  }

  .search input::placeholder { color: var(--ink-faint); }

  .search input:focus {
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-soft);
  }

  .clear {
    position: absolute;
    right: 7px; top: 50%;
    transform: translateY(-50%);
    width: 22px; height: 22px;
    display: none;
    align-items: center;
    justify-content: center;
    border: 0;
    border-radius: 5px;
    background: none;
    cursor: pointer;
    color: var(--ink-faint);
    font-size: 15px;
    line-height: 1;
  }

  .clear:hover { background: var(--rule); color: var(--ink); }
  .search.filled .clear { display: flex; }

  .seg {
    display: inline-flex;
    border: 1px solid var(--rule);
    border-radius: 8px;
    background: var(--paper-raise);
    overflow: hidden;
  }

  .seg button {
    font: inherit;
    font-size: 13px;
    padding: 8px 13px;
    border: 0;
    background: none;
    color: var(--ink-soft);
    cursor: pointer;
    white-space: nowrap;
  }

  .seg button + button { border-left: 1px solid var(--rule); }
  .seg button:hover { color: var(--ink); background: #f0ece2; }

  .seg button[aria-pressed="true"] {
    background: var(--ink);
    color: var(--paper-raise);
  }

  /* ---------- section ---------- */
  section { margin-bottom: 38px; }

  .sec-head {
    display: flex;
    align-items: baseline;
    justify-content: space-between;
    gap: 14px;
    flex-wrap: wrap;
    padding-bottom: 9px;
    margin-bottom: 1px;
    border-bottom: 1px solid var(--rule);
  }

  .sec-head h2 {
    margin: 0;
    font-size: 15px;
    font-weight: 600;
    letter-spacing: -0.005em;
  }

  .sec-head p {
    margin: 0;
    font-size: 12.5px;
    color: var(--ink-faint);
  }

  .sec-head code {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--ink-soft);
  }

  /* ---------- grid ---------- */
  .grid {
    display: grid;
    grid-template-columns: repeat(6, minmax(0, 1fr));
    gap: 1px;
    background: var(--rule);
    border: 1px solid var(--rule);
    border-top: 0;
    border-radius: 0 0 10px 10px;
    overflow: hidden;
  }

  .tile {
    position: relative;
    display: flex;
    flex-direction: column;
    align-items: center;
    justify-content: center;
    gap: 13px;
    padding: 26px 10px 17px;
    background: var(--paper);
    border: 0;
    font: inherit;
    color: inherit;
    cursor: pointer;
    text-align: center;
    transition: background-color 130ms ease;
  }

  .tile:hover { background: var(--paper-raise); }

  .tile:focus-visible {
    outline: 2px solid var(--accent);
    outline-offset: -2px;
    z-index: 2;
  }

  .tile .glyph {
    width: 30px; height: 30px;
    stroke: var(--ink);
    stroke-width: 1.6;
    fill: none;
    stroke-linecap: round;
    stroke-linejoin: round;
    display: block;
    transition: stroke 130ms ease, transform 130ms ease;
  }

  .tile:hover .glyph { stroke: var(--accent); transform: scale(1.08); }

  .tile .label {
    display: block;
    font-family: var(--mono);
    font-size: 10.5px;
    letter-spacing: 0.055em;
    color: var(--ink-soft);
    word-break: break-all;
    line-height: 1.35;
  }

  .tile .sub {
    display: block;
    margin-top: 3px;
    font-family: var(--mono);
    font-size: 9px;
    letter-spacing: 0.03em;
    color: var(--ink-faint);
    word-break: break-all;
  }

  .tile .badge {
    position: absolute;
    top: 7px; right: 7px;
    width: 6px; height: 6px;
    border-radius: 50%;
    background: var(--accent);
  }

  .tile.has-anim .glyph::after { content: none; }

  /* small ring on lucide tiles that also ship as an animated icon */
  .tile[data-anim="1"]::after {
    content: "";
    position: absolute;
    inset: 6px;
    border-radius: 7px;
    border: 1px dashed var(--rule);
    pointer-events: none;
  }

  .legend {
    display: flex;
    align-items: center;
    gap: 22px;
    flex-wrap: wrap;
    margin: 0 0 22px;
    font-size: 12.5px;
    color: var(--ink-soft);
  }

  .legend span { display: inline-flex; align-items: center; gap: 8px; }

  .legend i {
    position: relative;
    width: 20px; height: 15px;
    border-radius: 4px;
    border: 1px solid var(--rule);
    background: var(--paper);
    display: inline-block;
    flex: none;
  }

  .legend i.dashed::after {
    content: "";
    position: absolute;
    inset: 2px;
    border-radius: 2px;
    border: 1px dashed var(--rule);
  }

  .legend i.dot::after {
    content: "";
    position: absolute;
    top: 2px; right: 2px;
    width: 5px; height: 5px;
    border-radius: 50%;
    background: var(--accent);
  }


  /* ---------- misc ---------- */
  .empty {
    padding: 72px 20px;
    text-align: center;
    color: var(--ink-soft);
    font-size: 14.5px;
  }

  footer {
    margin-top: 40px;
    padding-top: 18px;
    border-top: 1px solid var(--rule);
    display: flex;
    justify-content: space-between;
    gap: 16px;
    flex-wrap: wrap;
    font-size: 12.5px;
    color: var(--ink-faint);
  }

  footer code {
    font-family: var(--mono);
    font-size: 11.5px;
    color: var(--ink-soft);
  }

  .toast {
    position: fixed;
    left: 50%;
    bottom: 30px;
    transform: translate(-50%, 16px);
    display: flex;
    align-items: center;
    gap: 9px;
    padding: 9px 15px;
    border-radius: 9px;
    background: var(--ink);
    color: var(--paper-raise);
    font-family: var(--mono);
    font-size: 12px;
    box-shadow: 0 8px 26px rgba(38, 36, 31, 0.22);
    opacity: 0;
    pointer-events: none;
    transition: opacity 170ms ease, transform 170ms ease;
    max-width: min(90vw, 560px);
    z-index: 60;
  }

  .toast.show { opacity: 1; transform: translate(-50%, 0); }
  .toast .tick { color: #8fdca4; }

  @media (max-width: 1000px) { .grid { grid-template-columns: repeat(4, minmax(0, 1fr)); } }
  @media (max-width: 820px) {
    .shell { padding: 0 14px 72px; }
    header { padding: 34px 0 18px; }
    h1 { font-size: 23px; }
    .grid { grid-template-columns: repeat(3, minmax(0, 1fr)); }
    .tile { padding: 20px 6px 14px; gap: 10px; }
  }

  @media (prefers-reduced-motion: reduce) {
    * { transition: none !important; }
  }
</style>
</head>
<body>
<div class="shell">
  <header>
    <h1>Fox 图标总览</h1>
    <p class="lede">应用中实际使用的全部图标，共 <b>__TOTAL__</b> 个 —— <b>__LUCIDE__</b> 个来自 lucide-react v0.544.0，<b>__ANIM__</b> 个为仓库内的动画图标。点击任意图标即可复制 import 语句。</p>
  </header>

  <div class="controls">
    <div class="search" id="searchWrap">
      <svg viewBox="0 0 24 24" aria-hidden="true"><circle cx="11" cy="11" r="8"></circle><path d="m21 21-4.34-4.34"></path></svg>
      <input id="q" type="search" placeholder="搜索图标名，例如 alert、file、chevron…" autocomplete="off" spellcheck="false" aria-label="搜索图标">
      <button class="clear" id="clear" type="button" aria-label="清空搜索">&#215;</button>
    </div>
  </div>

  <div class="legend">
    <span><i class="dashed"></i> 同时有动画版本</span>
    <span><i class="dot"></i> 仓库内动画图标</span>
  </div>

  <main id="mount"></main>

  <footer>
    <span>扫描自 <code>apps/desktop/src</code> · 376 个源文件 · 236 个 lucide 导入符号</span>
    <span id="shown"></span>
  </footer>
</div>

<div class="toast" id="toast" role="status" aria-live="polite"></div>

<script>
const DATA = __DATA__;

const mount = document.getElementById('mount');
const q = document.getElementById('q');
const searchWrap = document.getElementById('searchWrap');
const clearBtn = document.getElementById('clear');
const toast = document.getElementById('toast');
const shown = document.getElementById('shown');

let query = '';

const SVG_OPEN = '<svg class="glyph" viewBox="0 0 24 24" aria-hidden="true">';

function importLine(t) {
  return 'import { ' + t.aliases[0] + ' } from "' + (t.kind === 'animated'
    ? '@/components/fox-ui/animated-icons'
    : 'lucide-react') + '"';
}

function matches(t) {
  if (!query) return true;
  const hay = (t.name + ' ' + t.slug + ' ' + t.aliases.join(' ')).toLowerCase();
  return query.split(/\\s+/).every(function (part) { return hay.indexOf(part) !== -1; });
}

function tileHtml(t) {
  const aliases = t.aliases.filter(function (a) { return a !== t.name; });
  const sub = aliases.length ? t.slug + ' · ' + aliases.join(' ') : t.slug;
  return '<button class="tile" type="button" data-key="' + t.kind + ':' + t.name + '"'
    + (t.animated ? ' data-anim="1"' : '')
    + ' title="' + importLine(t).replace(/"/g, '&quot;') + '">'
    + (t.kind === 'animated' ? '<span class="badge"></span>' : '')
    + SVG_OPEN + t.inner + '</svg>'
    + '<span class="label">' + t.name + '<span class="sub">' + sub + '</span></span>'
    + '</button>';
}

function sectionHtml(title, note, list) {
  const grid = list.length
    ? list.map(tileHtml).join('')
    : '<div class="empty" style="grid-column:1/-1">没有匹配的图标。</div>';
  return '<section>'
    + '<div class="sec-head"><h2>' + title + '</h2><p>' + note + '</p></div>'
    + '<div class="grid">' + grid + '</div>'
    + '</section>';
}

function render() {
  const lucide = DATA.lucide.filter(matches);
  const animated = DATA.animated.filter(matches);
  const total = lucide.length + animated.length;

  if (!total) {
    mount.innerHTML = '<div class="empty">没有匹配的图标。换个关键词，或清空搜索。</div>';
    shown.textContent = '';
    return;
  }

  mount.innerHTML =
    sectionHtml('Lucide', 'lucide-react v0.544.0 · ' + lucide.length + ' 个', lucide) +
    sectionHtml('动画图标', '仓库内实现 · ' + animated.length + ' 个', animated);

  shown.textContent = '当前显示 ' + total + ' / ' + (DATA.lucide.length + DATA.animated.length) + ' 个图标';
}

function apply() {
  query = q.value.trim().toLowerCase();
  searchWrap.classList.toggle('filled', q.value.length > 0);
  render();
}

q.addEventListener('input', apply);
clearBtn.addEventListener('click', function () { q.value = ''; q.focus(); apply(); });

let toastTimer;
function flash(line) {
  toast.innerHTML = '<span class="tick">&#10003;</span><span>' + line.replace(/</g, '&lt;') + '</span>';
  toast.classList.add('show');
  clearTimeout(toastTimer);
  toastTimer = setTimeout(function () { toast.classList.remove('show'); }, 2200);
}

mount.addEventListener('click', function (e) {
  const tile = e.target.closest('.tile');
  if (!tile) return;
  const [kind, name] = tile.dataset.key.split(':');
  const pool = kind === 'animated' ? DATA.animated : DATA.lucide;
  const t = pool.find(function (x) { return x.name === name; });
  if (!t) return;
  const line = importLine(t);
  if (navigator.clipboard && navigator.clipboard.writeText) {
    navigator.clipboard.writeText(line).then(function () { flash(line); }, function () { flash(line); });
  } else { flash(line); }
});

apply();
</script>
</body>
</html>
"""

HTML = (HTML
        .replace("__DATA__", payload)
        .replace("__TOTAL__", str(n_total))
        .replace("__LUCIDE__", str(n_lucide))
        .replace("__ANIM__", str(n_anim)))

with open(OUT, "w", encoding="utf-8") as f:
    f.write(HTML)

print(f"wrote {OUT}: {n_total} tiles ({n_lucide} lucide + {n_anim} animated)")
