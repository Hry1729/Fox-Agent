#!/usr/bin/env python3
"""Render the icon-animation review HTML."""
import json, sys, html

CLASS, ICONDATA, OUT = sys.argv[1:4]

rows = json.load(open(CLASS, encoding="utf-8"))
data = json.load(open(ICONDATA, encoding="utf-8"))

glyph = {}
for it in data["lucide"]:
    glyph[it["name"]] = it["inner"]
for it in data["animated"]:
    glyph.setdefault(it["name"], it["inner"])

# ---- Q1: existing animated icons -> animateicons equivalents ----
EXISTING = [
    ("Bell", "BellIcon", "ok", "通知中心，6 个文件在用。"),
    ("Blocks", "BlocksIcon", "ok", "插件 / 模块入口。"),
    ("MessageCirclePlus", "MessageCirclePlusIcon", "ok", "新建对话按钮。"),
    ("PanelLeft", "PanelLeftIcon", "ok", "左侧栏开合。"),
    ("Search", "SearchIcon", "ok", "全应用调用最多的动效候选，29 处。"),
    ("Settings", "SettingsIcon", "ok", "设置入口。"),
    ("SunMoon", "SunMoonIcon", "ok", "主题切换。"),
    ("Users", "UsersIcon", "ok", "成员 / 协作。"),
    ("PanelRight", "PanelLeftIcon", "replace", "库里没有 PanelRight。侧栏开合是成对动作，"
                                              "可改用 PanelLeft 水平翻转（scaleX(-1)），"
                                              "或保留现有实现。"),
    ("Maximize", "ExpandIcon", "replace", "库里没有 Maximize / Maximize2。语义最近的是 "
                                          "Expand（四角向外），可同时覆盖这两个现有图标。"),
    ("Minimize", "ShrinkIcon", "replace", "同上，对应 Shrink（四角向内）。"),
]

cat_meta = {
    "animate": ("建议改为动效", "fa"),
    "optional": ("视位置而定", "op"),
    "static": ("保持静态", "st"),
    "existing": ("已有动效", "ex"),
}

# ---- build classification rows ----
order = {"animate": 0, "optional": 1, "existing": 2, "static": 3}
rows.sort(key=lambda r: (order[r["cat"]], -r["occ"], r["name"]))

def tile(r):
    return (
        '<div class="row" data-cat="%s" data-name="%s" data-occ="%d">'
        '<span class="cat %s">%s</span>'
        '<span class="gl">%s</span>'
        '<span class="nm">%s<span class="sl">%s</span></span>'
        '<span class="ct">%d</span>'
        '<span class="ry">%s</span>'
        '</div>'
    ) % (
        r["cat"], r["name"].lower(), r["occ"],
        cat_meta[r["cat"]][1], cat_meta[r["cat"]][0],
        '<svg viewBox="0 0 24 24" aria-hidden="true">' + glyph.get(r["name"], "") + "</svg>",
        r["name"], r["slug"],
        r["occ"],
        html.escape(r["why"]),
    )

counts = {}
for r in rows:
    counts[r["cat"]] = counts.get(r["cat"], 0) + 1

mig = "".join(
    '<div class="mig">'
    '<span class="gl">%s</span>'
    '<span class="mig-names"><code>%s</code><span class="arr">%s</span><code>%s</code></span>'
    '<span class="pill %s">%s</span>'
    '<span class="mig-note">%s</span>'
    "</div>"
    % (
        '<svg viewBox="0 0 24 24" aria-hidden="true">' + glyph.get(n, "") + "</svg>",
        n, "&rarr;" if k == "ok" else "&asymp;", t,
        "ok" if k == "ok" else "warn",
        "直接替换" if k == "ok" else "需变通",
        html.escape(note),
    )
    for n, t, k, note in EXISTING
)

payload = json.dumps({"rows": rows}, ensure_ascii=False)

TEMPLATE = """<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>Fox 图标动效评估</title>
<style>
  :root {
    color-scheme: light;
    --paper:#f4f1ea; --raise:#faf8f3; --ink:#26241f; --soft:#5c574d;
    --faint:#a09a8d; --rule:#ded8ca; --dot:#ddd7c8;
    --go:#1f7a4d; --go-bg:#e7f1ea;
    --warn:#9a6410; --warn-bg:#f6eedd;
    --stop:#8a4a42; --stop-bg:#f2e8e5;
    --keep:#4a5b7a; --keep-bg:#e9edf4;
    --sans:"HarmonyOS Sans SC","Segoe UI Variable","Segoe UI",system-ui,sans-serif;
    --mono:ui-monospace,"Cascadia Code",Consolas,monospace;
  }
  *{box-sizing:border-box}
  body{
    margin:0;font-family:var(--sans);color:var(--ink);line-height:1.55;
    background-color:var(--paper);
    background-image:radial-gradient(var(--dot) .8px,transparent .8px);
    background-size:18px 18px;background-position:-1px -1px;
  }
  .shell{max-width:1060px;margin:0 auto;padding:0 26px 110px}

  header{padding:54px 0 26px;border-bottom:1px solid var(--rule);margin-bottom:34px}
  h1{margin:0 0 10px;font-size:29px;font-weight:600;letter-spacing:-.015em;line-height:1.18}
  .lede{margin:0;font-size:14.5px;color:var(--soft);max-width:70ch}
  .lede b{font-weight:600;color:var(--ink)}
  .stats{display:flex;gap:30px;flex-wrap:wrap;margin-top:20px}
  .stat{display:flex;flex-direction:column;gap:2px}
  .stat b{font-size:21px;font-weight:600;letter-spacing:-.01em}
  .stat span{font-size:12px;color:var(--faint)}

  h2{font-size:17px;font-weight:600;margin:44px 0 6px;letter-spacing:-.005em}
  h2 .num{color:var(--faint);font-weight:500;margin-right:8px}
  .sub{margin:0 0 20px;font-size:13.5px;color:var(--soft);max-width:74ch}

  .callout{
    padding:15px 18px;border:1px solid var(--rule);border-left:3px solid var(--keep);
    border-radius:0 8px 8px 0;background:var(--raise);font-size:13.5px;color:var(--soft);
    margin:18px 0 0;max-width:none;
  }
  .callout b{color:var(--ink);font-weight:600}
  .callout code{font-family:var(--mono);font-size:12.5px;color:var(--ink)}

  /* migration table */
  .mig-list{border:1px solid var(--rule);border-radius:10px;overflow:hidden;background:var(--raise)}
  .mig{
    display:grid;grid-template-columns:34px 220px 82px 1fr;gap:14px;align-items:center;
    padding:12px 16px;border-top:1px solid var(--rule);font-size:13px;
  }
  .mig:first-child{border-top:0}
  .mig .gl svg{width:22px;height:22px;stroke:var(--ink);stroke-width:1.6;fill:none;
    stroke-linecap:round;stroke-linejoin:round;display:block}
  .mig-names{display:flex;align-items:center;gap:8px;flex-wrap:wrap}
  .mig-names code{font-family:var(--mono);font-size:11.5px;color:var(--ink);
    background:var(--paper);border:1px solid var(--rule);border-radius:4px;padding:1px 6px}
  .mig-names .arr{color:var(--faint);font-size:13px}
  .pill{font-size:11px;border-radius:20px;padding:2px 9px;white-space:nowrap;justify-self:start}
  .pill.ok{background:var(--go-bg);color:var(--go)}
  .pill.warn{background:var(--warn-bg);color:var(--warn)}
  .mig-note{color:var(--soft);font-size:12.5px}

  /* classification */
  .legend{display:flex;gap:9px;flex-wrap:wrap;margin:0 0 16px}
  .legend button{
    font:inherit;font-size:12.5px;padding:6px 13px;border:1px solid var(--rule);
    background:var(--raise);color:var(--soft);border-radius:20px;cursor:pointer;
  }
  .legend button[aria-pressed="true"]{background:var(--ink);color:var(--raise);border-color:var(--ink)}
  .legend button .n{color:inherit;opacity:.6;margin-left:5px;font-variant-numeric:tabular-nums}

  .table{border:1px solid var(--rule);border-radius:10px;overflow:hidden;background:var(--raise)}
  .thead,.row{
    display:grid;grid-template-columns:74px 34px 168px 56px 1fr;gap:14px;align-items:center;
  }
  .thead{
    padding:9px 16px;background:var(--paper);border-bottom:1px solid var(--rule);
    font-size:11.5px;color:var(--faint);letter-spacing:.02em;
  }
  .row{padding:9px 16px;border-top:1px solid var(--rule);font-size:12.5px}
  .row:first-child{border-top:0}
  .row:hover{background:var(--paper)}
  .row .gl svg{width:20px;height:20px;stroke:var(--ink);stroke-width:1.6;fill:none;
    stroke-linecap:round;stroke-linejoin:round;display:block}
  .row .nm{font-family:var(--mono);font-size:11.5px;color:var(--ink);word-break:break-all}
  .row .sl{display:block;font-size:9.5px;color:var(--faint);margin-top:1px}
  .row .ct{font-variant-numeric:tabular-nums;color:var(--soft);text-align:right}
  .row .ry{color:var(--soft)}
  .cat{font-size:10.5px;border-radius:20px;padding:2px 8px;text-align:center;justify-self:start;white-space:nowrap}
  .cat.fa{background:var(--go-bg);color:var(--go)}
  .cat.op{background:var(--warn-bg);color:var(--warn)}
  .cat.st{background:var(--stop-bg);color:var(--stop)}
  .cat.ex{background:var(--keep-bg);color:var(--keep)}

  .empty{padding:60px;text-align:center;color:var(--soft)}

  footer{margin-top:46px;padding-top:18px;border-top:1px solid var(--rule);
    font-size:12.5px;color:var(--faint)}
  footer code{font-family:var(--mono);font-size:11.5px;color:var(--soft)}

  @media (max-width:880px){
    .mig{grid-template-columns:30px 1fr;gap:6px 12px}
    .pill{justify-self:start}
    .mig-note{grid-column:2}
    .thead{display:none}
    .row{grid-template-columns:64px 28px 1fr;gap:8px 12px}
    .row .ct{grid-column:3;text-align:left}
    .row .ry{grid-column:1/-1}
  }
  @media (prefers-reduced-motion:reduce){*{transition:none!important}}
</style>
</head>
<body>
<div class="shell">
  <header>
    <h1>Fox 图标动效评估</h1>
    <p class="lede">两个问题：现有 11 个动画图标能否换成一个库，以及全应用 <b>175</b> 个 lucide 图标里哪些该动、哪些不该动。结论基于 animateicons.in（<b>@animateicons/react</b> v0.12.0，676 个 lucide 动效图标）与仓库里每个图标的实际调用位置。</p>
    <div class="stats">
      <span class="stat"><b>__N_ANIM__</b><span>建议改为动效</span></span>
      <span class="stat"><b>__N_OPT__</b><span>视位置而定</span></span>
      <span class="stat"><b>__N_STATIC__</b><span>保持静态</span></span>
      <span class="stat"><b>__N_EX__</b><span>已有动效</span></span>
    </div>
  </header>

  <h2><span class="num">01</span>现有动画图标能否被替代</h2>
  <p class="sub">仓库里 11 个动画图标来自 animate-ui。animateicons 覆盖了其中 8 个同名图标，可以直接替换；剩下 3 个库里没有，需要变通。替换后即可只保留<strong>一个</strong>图标体系（lucide + animateicons）。</p>

  <div class="mig-list">__MIG__</div>

  <div class="callout">
    <b>换库前需要知道的几点。</b>animateicons 的图标渲染出来是
    <code>&lt;div&gt;</code> 而不是 <code>&lt;svg&gt;</code>，
    所以现在写在 <code>svg</code> 上的 <code>className</code>、
    <code>strokeWidth</code>、<code>currentColor</code> 继承关系都要重新核对；
    它自带 motion 运行时（不额外依赖 motion 包），单个图标 tree-shaking 后约
    4–5 kB，而 lucide 单个约 1 kB。两个库都基于 motion，仓库现有的
    <code>AnimatedIconTrigger</code>（hover / tap 触发）可以整体换掉，
    但如果需要动画循环或联动 <code>AnimatedIconTrigger</code> 的 <code>animateOnHover</code> 与
    <code>AnimateIcon</code> 上下文，得确认接口是否对应。
  </div>

  <h2><span class="num">02</span>哪些图标适合改成动效</h2>
  <p class="sub">下面把 175 个图标逐个判定。<strong>建议改为动效</strong>的是动作或状态变化驱动的图标，数量少、位置关键，动效能确认「操作已发生」；<strong>保持静态</strong>的是方向、状态、文件类型、高频外壳等——它们动起来要么语义冲突，要么同屏实例太多造成干扰。点按钮筛选。</p>

  <div class="legend" id="legend">
    <button data-filter="all" aria-pressed="true">全部<span class="n">175</span></button>
    <button data-filter="animate" aria-pressed="false">建议改为动效<span class="n">__N_ANIM__</span></button>
    <button data-filter="optional" aria-pressed="false">视位置而定<span class="n">__N_OPT__</span></button>
    <button data-filter="static" aria-pressed="false">保持静态<span class="n">__N_STATIC__</span></button>
    <button data-filter="existing" aria-pressed="false">已有动效<span class="n">__N_EX__</span></button>
  </div>

  <div class="table" id="table">
    <div class="thead"><span>判定</span><span></span><span>图标</span><span>出现次数</span><span>理由</span></div>
    <div id="tbody">__ROWS__</div>
  </div>

  <h2><span class="num">03</span>判定依据</h2>
  <p class="sub">不是凭图标好不好看，而是看它在应用里被放进什么位置。同样的图形，在「触发动作」的位置该动，在「陈述状态」的位置就不该动。</p>

  <div class="callout">
    <b>建议改为动效的四类。</b>一是<strong>不可逆或高风险操作</strong>（删除、清空）——动效承担确认职能；
    二是<strong>异步过程的进行中状态</strong>（加载、思考、生成、录音）——动效本身就是状态指示；
    三是<strong>会被反复点击的动作按钮</strong>（播放、暂停、发送、刷新）——点击后需要即时反馈；
    四是<strong>外部事件到达</strong>（通知铃铛）——动效替代了「去看一眼」。
  </div>
  <div class="callout">
    <b>保持静态的七类。</b>方向箭头（动效即位移，指代会含混）；状态事实（通过 / 警告 / 离线，本身没有发生的瞬间）；
    高频外壳操作（同屏几十个，一个动全体乱）；文件类型标识（查表产生，一屏十几个，外框须稳定以便扫读）；
    布局状态（画布静止态，没有可类比的物理动作）；身份与偏好取值（动就等于提示值变了，会是假信号）；
    名词性实体（服务器、数据库、文件夹，是标签不是动作，找不到自然且信息量对等的动作）。
  </div>

  <footer>
    扫描自 <code>apps/desktop/src</code> · 376 个源文件 · 截图与对照基于
    <code>@animateicons/react@0.12.0</code> 与 <code>lucide-react@0.544.0</code>。
    出现次数为 JSX 中的实际渲染次数。
  </footer>
</div>

<script>
const GLYPH = __GLYPH__;
const DATA = __DATA__;
const LABEL = { animate:'fa', optional:'op', static:'st', existing:'ex' };
const NAME  = { animate:'建议改为动效', optional:'视位置而定', static:'保持静态', existing:'已有动效' };

const tbody = document.getElementById('tbody');
const legend = document.getElementById('legend');
let filter = 'all';

function esc(s) {
  return String(s).replace(/&/g,'&amp;').replace(/</g,'&lt;').replace(/>/g,'&gt;');
}

function render() {
  const rows = DATA.rows.filter(function (r) { return filter === 'all' || r.cat === filter; });
  if (!rows.length) {
    tbody.innerHTML = '<div class="empty">这一类的图标在库中暂无对应动效版本。</div>';
    return;
  }
  tbody.innerHTML = rows.map(function (r) {
    return '<div class="row" data-cat="' + r.cat + '">'
      + '<span class="cat ' + LABEL[r.cat] + '">' + NAME[r.cat] + '</span>'
      + '<span class="gl"><svg viewBox="0 0 24 24" aria-hidden="true">'
      + (GLYPH[r.name] || '') + '</svg></span>'
      + '<span class="nm">' + esc(r.name) + '<span class="sl">' + esc(r.slug) + '</span></span>'
      + '<span class="ct">' + r.occ + '</span>'
      + '<span class="ry">' + esc(r.why) + '</span>'
      + '</div>';
  }).join('');
}

legend.addEventListener('click', function (e) {
  const b = e.target.closest('button[data-filter]');
  if (!b) return;
  filter = b.dataset.filter;
  [].forEach.call(legend.querySelectorAll('button'), function (x) {
    x.setAttribute('aria-pressed', String(x === b));
  });
  render();
});
</script>
</body>
</html>
"""

# glyph map for the JS re-render
gmap = json.dumps(glyph, ensure_ascii=False)

HTML = (TEMPLATE
        .replace("__MIG__", mig)
        .replace("__ROWS__", "".join(tile(r) for r in rows))
        .replace("__DATA__", payload)
        .replace("__GLYPH__", gmap)
        .replace("__N_ANIM__", str(counts.get("animate", 0)))
        .replace("__N_OPT__", str(counts.get("optional", 0)))
        .replace("__N_STATIC__", str(counts.get("static", 0)))
        .replace("__N_EX__", str(counts.get("existing", 0))))

open(OUT, "w", encoding="utf-8").write(HTML)
print("wrote", OUT, "| counts", counts)
