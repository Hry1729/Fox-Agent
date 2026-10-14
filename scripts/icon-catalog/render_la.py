#!/usr/bin/env python3
"""Render the lucide-animated.com assessment page."""
import json, sys, html

LA_DATA, FINAL, OUT = sys.argv[1:4]

d = json.load(open(LA_DATA, encoding="utf-8"))
rows = d["rows"]
counts = d["counts"]
final = json.load(open(FINAL, encoding="utf-8"))

glyph = {}
for it in final["lucide"]:
    glyph[it["name"]] = it["inner"]
for it in final["animated"]:
    glyph.setdefault(it["name"], it["inner"])

cat_meta = {
    "animate": ("建议改为动效", "fa"),
    "optional": ("视位置而定", "op"),
    "static": ("保持静态", "st"),
    "existing": ("已有动效", "ex"),
}
order = {"animate": 0, "optional": 1, "existing": 2, "static": 3}
rows.sort(key=lambda r: (order[r["cat"]], r["name"].lower()))

# ---- 01: architecture comparison ----
CMP = [
    ("分发方式", "shadcn registry：<code>npx shadcn add</code> 逐个把源码拷进仓库",
     "npm 包：一条 import，tree-shaking 交给打包器"),
    ("图标数量", "468 个", "676 个 lucide（另含 501 个 Huge 图标）"),
    ("DOM 结构", '外层 <code>&lt;div&gt;</code> 包一个 <code>motion.svg</code>',
     "单个 <code>&lt;svg&gt;</code>（自带动画）"),
    ("默认触发", "hover 自动播，另有 ref 上的 <code>startAnimation()</code>",
     "不自动播，靠仓库现有的 <code>AnimateIcon</code> 包裹"),
    ("可用触发", "hover、ref 命令式", "hover、tap、进入视口、loop 循环"),
    ("动效形态", "逐图标手画，形态各异", "统一 path / path-loop 描边 + 各图标变体"),
    ("额外依赖", "<code>motion</code> + <code>@/lib/utils</code> 的 <code>cn</code> + Tailwind",
     "自带运行时（内部用 motion）"),
    ("覆盖仓库 175 个图标", "<b>82</b>", "<b>140</b>"),
    ("覆盖「建议改动」22 个", "<b>17</b>", "<b>22</b>（全覆盖）"),
    ("覆盖现有 11 个动画图标", "<b>8</b>", "<b>8</b>"),
    ("减少动效偏好", "未处理，需自己接 <code>useReducedMotion</code>",
     "仓库的 <code>AnimatedIconTrigger</code> 已处理"),
    ("迁移工作量", "<b>大</b>：逐个拉取 + 改导入 + 补 <code>utils cn</code> + 重做触发",
     "<b>小</b>：一个包，改导入即可"),
]

# ---- 03: the 11 in-repo icons ----
EXISTING = [
    ("Bell", "bell", "BellIcon", True, ""),
    ("Blocks", "blocks", "BlocksIcon", True, ""),
    ("MessageCirclePlus", "message-circle-plus", "MessageCirclePlusIcon", True, ""),
    ("Search", "search", "SearchIcon", True, ""),
    ("Settings", "settings", "SettingsIcon", True, ""),
    ("SunMoon", "sun-moon", "SunMoonIcon", True, ""),
    ("Users", "users", "UsersIcon", True, ""),
    ("Maximize", "maximize", "MaximizeIcon", True, ""),
    ("Minimize", "minimize", "MinimizeIcon", True, ""),
    ("PanelLeft", "panel-left-close", "PanelLeftCloseIcon", False,
     "站点只有开、关两个状态图标（panel-left-close / panel-left-open），"
     "仓库用的是单一 PanelLeft 朝左的图标。语义能对上，但是两个组件，"
     "不是同一枚。"),
    ("PanelRight", "panel-right-open", "PanelRightOpenIcon", False,
     "同上，站点只有 panel-right-open。仓库的 PanelRight 是朝右的图标，"
     "可以对应到它，但需要确认开合两个方向怎么切换。"),
]

# ---- 04: migration costs ----
COSTS = [
    ("逐个拉取源码", "468 个图标没有 npm 包，每个都要 <code>npx shadcn add "
                    "&quot;https://lucide-animated.com/r/&lt;name&gt;.json&quot;</code>，"
                    "或者用它的 MCP / llms.txt 自己写脚本批量拉。仓库现在 11 个动画图标"
                    "是本地文件，直接换掉即可；但要扩到 22 个就得新增 11 次拉取。"),
    ("<code>cn</code> 与 Tailwind 依赖", "生成的文件里 <code>import { cn } from "
     "&quot;@/lib/utils&quot;</code>，仓库确实有 <code>@/lib/utils</code>，"
     "<code>components.json</code> 也配好了 <code>@/components</code> → "
     "<code>src/components</code>，所以路径能直接落地，不需要额外配置。"),
    ("触发方式要重做", "这是最实际的一处。仓库现在的 <code>AnimatedIconTrigger</code> "
     "用 <code>asChild</code> 把 <code>AnimateIcon</code> 包在按钮外面，"
     "注释写得很明确：<em>让整个动作（含文字标签）一起响应 hover / press，每次手势只播一次</em>。"
     "lucide-animated 的 hover 监听在图标自己那个 <code>&lt;div&gt;</code> 上——"
     "只有鼠标压到图标本身才播。要保留现在的行为，得给每个图标再加一层 ref 包装。"),
    ("动效形态不一样", "<code>lucide-animated</code> 每个文件都是手写的："
     "<code>bell</code> 是整枚 svg 摇（<code>rotate: [0,-10,10,-10,0]</code>），"
     "<code>delete</code> 是盖子 <code>y: -1.1</code> 上移 + 三条线微调。"
     "而仓库现有的 animate-ui 体系基于 variants + <code>path</code> / "
     "<code>path-loop</code> 描边循环。换库等于把动效语言整体换掉。"),
    ("减少动效偏好", "仓库在 <code>AnimatedIconTrigger</code> 里用 "
     "<code>useReducedMotion()</code> 关掉了 hover / tap 动画。"
     "lucide-animated 的 hover 是写死的，要保留这个无障碍行为得自己在包装层拦。"),
    ("体积与署名", "单图标源码约 2–4 kB（含 motion 变体），"
     "源码是逐个拷进仓库的，不参与 tree-shaking 讨论；"
     "MIT 协议，站点明确允许个人与商业项目使用，教程和 demo 不可再分发。"),
]

# ---- render helpers ----
def tile(r):
    la = ('<code>%s</code>' % r["laName"]) if r["laSlug"] else '<span class="no">&mdash;</span>'
    ai = '<span class="yes">&#10003;</span>' if r["ai"] else '<span class="no">&mdash;</span>'
    return (
        '<div class="row" data-cat="%s">'
        '<span class="cat %s">%s</span>'
        '<span class="gl">%s</span>'
        '<span class="nm">%s<span class="sl">%s</span></span>'
        '<span class="la">%s</span>'
        '<span class="ai">%s</span>'
        '<span class="ry">%s</span>'
        '</div>'
    ) % (
        r["cat"], cat_meta[r["cat"]][1], cat_meta[r["cat"]][0],
        '<svg viewBox="0 0 24 24" aria-hidden="true">' + glyph.get(r["name"], "") + "</svg>",
        r["name"], r["slug"], la, ai, html.escape(r["why"]),
    )

cmp_rows = "".join(
    '<div class="crow"><span class="ck">%s</span><span class="ca">%s</span>'
    '<span class="cb">%s</span></div>' % (k, a, b)
    for k, a, b in CMP
)

mig_rows = "".join(
    '<div class="mig">'
    '<span class="gl">%s</span>'
    '<span class="mig-names"><code>%s</code><span class="arr">&rarr;</span><code>%s</code></span>'
    '<span class="pill %s">%s</span>'
    '<span class="mig-note">%s</span>'
    '</div>' % (
        '<svg viewBox="0 0 24 24" aria-hidden="true">' + glyph.get(n, "") + "</svg>",
        n, comp,
        "ok" if ok else "warn",
        "直接替换" if ok else "需注意",
        html.escape(note) if note else "站点的同名图标，几何一致。",
    )
    for n, slug, comp, ok, note in EXISTING
)

cost_rows = "".join(
    '<div class="cost"><h4>%s</h4><p>%s</p></div>' % (t, b) for t, b in COSTS
)

payload = json.dumps({"rows": rows}, ensure_ascii=False)
gmap = json.dumps(glyph, ensure_ascii=False)

TEMPLATE = """<!doctype html>
<html lang="zh-CN">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<title>lucide-animated 评估</title>
<style>
  :root {
    color-scheme: light;
    --paper:#f4f1ea; --raise:#faf8f3; --ink:#26241f; --soft:#5c574d;
    --faint:#a09a8d; --rule:#ded8ca; --dot:#ddd7c8; --accent:#1f5fd0;
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
  .shell{max-width:1080px;margin:0 auto;padding:0 26px 110px}

  header{padding:54px 0 26px;border-bottom:1px solid var(--rule);margin-bottom:34px}
  h1{margin:0 0 10px;font-size:29px;font-weight:600;letter-spacing:-.015em;line-height:1.18}
  .lede{margin:0;font-size:14.5px;color:var(--soft);max-width:74ch}
  .lede b{font-weight:600;color:var(--ink)}
  .stats{display:flex;gap:34px;flex-wrap:wrap;margin-top:22px}
  .stat{display:flex;flex-direction:column;gap:2px}
  .stat b{font-size:21px;font-weight:600;letter-spacing:-.01em}
  .stat span{font-size:12px;color:var(--faint)}

  h2{font-size:17px;font-weight:600;margin:46px 0 6px;letter-spacing:-.005em}
  h2 .num{color:var(--faint);font-weight:500;margin-right:8px}
  .sub{margin:0 0 20px;font-size:13.5px;color:var(--soft);max-width:74ch}
  .sub b{color:var(--ink);font-weight:600}

  .callout{
    padding:15px 18px;border:1px solid var(--rule);border-left:3px solid var(--accent);
    border-radius:0 8px 8px 0;background:var(--raise);font-size:13.5px;color:var(--soft);
    margin:18px 0 0;
  }
  .callout.good{border-left-color:var(--go)}
  .callout.bad{border-left-color:var(--warn)}
  .callout b{color:var(--ink);font-weight:600}
  .callout code{font-family:var(--mono);font-size:12.5px;color:var(--ink);background:var(--paper);padding:1px 4px;border-radius:3px}
  .callout em{font-style:normal;color:var(--ink)}

  /* comparison */
  .cmp{border:1px solid var(--rule);border-radius:10px;overflow:hidden;background:var(--raise)}
  .cmp-head,.crow{display:grid;grid-template-columns:180px 1fr 1fr;gap:16px;align-items:start;padding:11px 16px}
  .cmp-head{background:var(--paper);border-bottom:1px solid var(--rule);font-size:11.5px;color:var(--faint);letter-spacing:.02em}
  .cmp-head b{color:var(--ink);font-weight:600}
  .crow{border-top:1px solid var(--rule);font-size:12.5px}
  .crow:first-child{border-top:0}
  .crow .ck{color:var(--ink);font-weight:500}
  .crow .ca,.crow .cb{color:var(--soft)}
  .crow code{font-family:var(--mono);font-size:11.5px;color:var(--ink)}
  .crow b{color:var(--ink);font-weight:600}

  /* migration */
  .mig-list{border:1px solid var(--rule);border-radius:10px;overflow:hidden;background:var(--raise)}
  .mig{display:grid;grid-template-columns:34px 240px 76px 1fr;gap:14px;align-items:center;
    padding:12px 16px;border-top:1px solid var(--rule);font-size:13px}
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

  /* costs */
  .costs{display:grid;grid-template-columns:repeat(2,1fr);gap:14px}
  .cost{border:1px solid var(--rule);border-radius:10px;background:var(--raise);padding:15px 17px}
  .cost h4{margin:0 0 6px;font-size:13px;font-weight:600}
  .cost p{margin:0;font-size:12.5px;color:var(--soft)}
  .cost code{font-family:var(--mono);font-size:11.5px;color:var(--ink);
    background:var(--paper);padding:1px 4px;border-radius:3px}

  /* coverage table */
  .legend{display:flex;gap:9px;flex-wrap:wrap;margin:0 0 16px}
  .legend button{font:inherit;font-size:12.5px;padding:6px 13px;border:1px solid var(--rule);
    background:var(--raise);color:var(--soft);border-radius:20px;cursor:pointer}
  .legend button[aria-pressed="true"]{background:var(--ink);color:var(--raise);border-color:var(--ink)}
  .legend button .n{color:inherit;opacity:.6;margin-left:5px;font-variant-numeric:tabular-nums}
  .table{border:1px solid var(--rule);border-radius:10px;overflow:hidden;background:var(--raise)}
  .thead,.row{display:grid;grid-template-columns:74px 30px 152px 118px 34px 1fr;gap:12px;align-items:center}
  .thead{padding:9px 16px;background:var(--paper);border-bottom:1px solid var(--rule);
    font-size:11px;color:var(--faint);letter-spacing:.02em}
  .thead .c{text-align:center}
  .row{padding:9px 16px;border-top:1px solid var(--rule);font-size:12.5px}
  .row:first-child{border-top:0}
  .row:hover{background:var(--paper)}
  .row .gl svg{width:19px;height:19px;stroke:var(--ink);stroke-width:1.6;fill:none;
    stroke-linecap:round;stroke-linejoin:round;display:block}
  .row .nm{font-family:var(--mono);font-size:11.5px;color:var(--ink);word-break:break-all}
  .row .sl{display:block;font-size:9.5px;color:var(--faint);margin-top:1px}
  .row .la{text-align:center}
  .row .ai{text-align:center}
  .row .la code{font-family:var(--mono);font-size:10.5px;color:var(--go);
    background:var(--go-bg);border-radius:4px;padding:1px 5px;display:inline-block;
    max-width:112px;overflow:hidden;text-overflow:ellipsis;white-space:nowrap;vertical-align:middle}
  .row .ry{color:var(--soft)}
  .yes{color:var(--go);font-size:13px}
  .no{color:var(--faint);font-size:12px}
  .warn-inline{color:var(--warn);font-size:11.5px}
  .cat{font-size:10.5px;border-radius:20px;padding:2px 8px;text-align:center;justify-self:start;white-space:nowrap}
  .cat.fa{background:var(--go-bg);color:var(--go)}
  .cat.op{background:var(--warn-bg);color:var(--warn)}
  .cat.st{background:var(--stop-bg);color:var(--stop)}
  .cat.ex{background:var(--keep-bg);color:var(--keep)}
  .empty{padding:60px;text-align:center;color:var(--soft)}

  footer{margin-top:46px;padding-top:18px;border-top:1px solid var(--rule);
    font-size:12.5px;color:var(--faint)}
  footer code{font-family:var(--mono);font-size:11.5px;color:var(--soft)}

  @media (max-width:900px){
    .cmp-head,.crow{grid-template-columns:1fr;gap:4px}
    .cmp-head span:nth-child(1){color:var(--ink)}
    .mig{grid-template-columns:30px 1fr;gap:6px 12px}
    .mig-note{grid-column:2}
    .costs{grid-template-columns:1fr}
    .thead{display:none}
    .row{grid-template-columns:64px 26px 1fr 30px;gap:8px 10px}
    .row .la{grid-column:3;text-align:left}
    .row .ry{grid-column:1/-1}
  }
  @media (prefers-reduced-motion:reduce){*{transition:none!important}}
</style>
</head>
<body>
<div class="shell">
  <header>
    <h1>lucide-animated 评估</h1>
    <p class="lede">和上一份 <b>animateicons</b> 的对照。结论先说：<b>lucide-animated</b>
    更像你仓库现在这套东西的同类，而不是 animateicons 的同类——它是 shadcn registry，
    源码逐个拷进仓库，每个图标自己管 hover 动画。这既是它的长处，也是迁移成本所在。</p>
    <div class="stats">
      <span class="stat"><b>468</b><span>站点图标总数</span></span>
      <span class="stat"><b>82</b><span>覆盖仓库 175 个</span></span>
      <span class="stat"><b>17 / 22</b><span>覆盖建议改动的</span></span>
      <span class="stat"><b>8 / 11</b><span>覆盖现有动画图标</span></span>
    </div>
  </header>

  <h2><span class="num">01</span>它和 animateicons 不是一类东西</h2>
  <p class="sub">同样是「lucide + motion 的动画图标」，但分发、结构、触发方式都不同。
    这决定了迁移是<b>改一个 import</b> 还是<b>逐个拉源码再重做触发</b>。</p>
  <div class="cmp">
    <div class="cmp-head"><span></span><span><b>lucide-animated.com</b></span><span><b>animateicons</b>（上一份）</span></div>
    __CMP__
  </div>

  <div class="callout good">
    <b>对你这个仓库，lucide-animated 有一个别人没有的优势。</b>
    你现在用的 animate-ui 也是「本地源码 + motion」这一路数，所以两者是同一个物种：
    图标文件躺在 <code>src/components/</code> 下，动效可以随便改，不受包版本牵制。
    从 animate-ui 换到 lucide-animated，是<b>同构替换</b>；换成 animateicons 则是引入一个
    外部运行时，动效形态由库决定，你改不了。
  </div>

  <h2><span class="num">02</span>覆盖面对比：82 对 140</h2>
  <p class="sub">按仓库实际在用的 <b>175</b> 个图标逐个比对。lucide-animated 有 468 枚，
    animateicons 有 676 枚 lucide 图标，差距主要落在「名词性图标」上——
    文件类型、设备、容器这类。</p>
  <div class="callout">
    <b>差的都是什么。</b>lucide-animated 覆盖不到的 93 个里，绝大多数本来就不该动：
    <code>FileCode</code>、<code>FileSpreadsheet</code>、<code>FileArchive</code> 这些文件类型，
    <code>Database</code>、<code>Server</code>、<code>HardDrive</code> 这些设备图标，
    <code>CircleAlert</code>、<code>Info</code>、<code>TriangleAlert</code> 这些状态标记。
    <b>真正有影响的是 5 枚</b>：<code>BellRing</code>、<code>WandSparkles</code>、
    <code>CircleStop</code>、<code>CirclePause</code>、<code>Target</code>——
    它们都在「建议改为动效」的 22 个名单里，但 lucide-animated 没有。
    也就是说<b>只靠它一个库凑不齐 22 枚</b>，还得留着 animateicons 或自己写。
  </div>

  <h2><span class="num">03</span>现有 11 个动画图标</h2>
  <p class="sub">8 个有同名同级对应，可以直接换。<b>PanelLeft / PanelRight 是唯一的两处真问题</b>——
    站点上没有这两个名字，只有开、关两种状态图标。</p>
  <div class="mig-list">__MIG__</div>
  <div class="callout bad">
    <b>PanelLeft / PanelRight 值得单独说。</b>仓库现在用的是「单一 PanelLeft，
    靠 <code>icon</code> 组件切换动画」，而 lucide-animated 给的是
    <code>panel-left-close</code> 和 <code>panel-left-open</code> 两枚独立图标，
    hover 时分别是合上和展开。如果侧栏本来就维护了 <code>isOpen</code> 状态，
    按状态选图标反而更贴切；如果没有，就得自己包一层。
  </div>

  <h2><span class="num">04</span>换过去要付的代价</h2>
  <p class="sub">六件事，按实际影响排序。前两件是机械劳动，<b>第三件是真正的设计决策</b>。</p>
  <div class="costs">__COSTS__</div>

  <h2><span class="num">05</span>全部 175 个图标的覆盖明细</h2>
  <p class="sub">判定沿用上一份：<b>建议改为动效</b>的是动作或状态变化驱动的图标，
    <b>保持静态</b>的是方向、状态、文件类型、高频外壳。第三列是 lucide-animated
    里的对应组件名，第四列标记 animateicons 有没有。点按钮筛选。</p>
  <div class="legend" id="legend">
    <button data-filter="all" aria-pressed="true">全部<span class="n">175</span></button>
    <button data-filter="animate" aria-pressed="false">建议改为动效<span class="n">__N_ANIM__</span></button>
    <button data-filter="optional" aria-pressed="false">视位置而定<span class="n">__N_OPT__</span></button>
    <button data-filter="static" aria-pressed="false">保持静态<span class="n">__N_STATIC__</span></button>
    <button data-filter="existing" aria-pressed="false">已有动效<span class="n">__N_EX__</span></button>
  </div>
  <div class="table" id="table">
    <div class="thead">
      <span>判定</span><span></span><span>图标</span>
      <span class="c">lucide-animated</span><span class="c">animate</span><span>理由</span>
    </div>
    <div id="tbody">__ROWS__</div>
  </div>

  <h2><span class="num">06</span>建议</h2>
  <p class="sub">如果目标是「一个库搞定」，两个都做不到——需要取舍。</p>
  <div class="callout good">
    <b>如果优先考虑改动小、可控：选 lucide-animated。</b>
    它和你现在的 animate-ui 是同一物种，8 个现有图标可以同构替换，
    <code>components.json</code> 的别名（<code>@/components</code>、<code>@/lib/utils</code>）
    已经配好，拉下来的文件能直接落地。代价是要逐个拉源码，并且要补一层 ref 包装
    才能保住「整个按钮一起响应 hover」的现有行为。缺口用 animate-ui 现有的图标补，
    或者自己写 5 个。
  </div>
  <div class="callout">
    <b>如果优先考虑覆盖面：选 animateicons。</b>22 个建议改动的图标它全有，
    一个包改 import 就能上。代价是动效形态由库决定、单图标体积更大，
    以及要接受一个 `<code>&lt;div&gt;</code>` 外壳。
  </div>
  <div class="callout bad">
    <b>两个都上是最差的选择——那还是两个库。</b>你原本的目标是收敛到一个。
    真要收敛，把「8 个同构替换 + 5 个自写」这条路走完，比引入第二个包更接近目标。
  </div>

  <footer>
    站点数据抓取自 <code>lucide-animated.com</code>（llms.txt 索引 468 枚 + registry JSON）·
    对照 <code>@animateicons/react@0.12.0</code> · 仓库 <code>lucide-react@0.544.0</code>。
    图标对照已处理站点未跟随的 lucide 更名：<code>Trash2→Delete</code>、
    <code>CircleQuestionMark→CircleHelp</code>、<code>Volume2→Volume</code>、
    <code>FlaskConical→Flask</code>、<code>Paperclip→AttachFile</code>、
    <code>LogIn/LogOut→Login/Logout</code>，以及站点自造的 <code>Upvote/Downvote</code>。
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
    tbody.innerHTML = '<div class="empty">没有匹配的图标。</div>';
    return;
  }
  tbody.innerHTML = rows.map(function (r) {
    const la = r.laSlug
      ? '<code title="' + esc(r.laSlug) + '">' + esc(r.laName) + '</code>'
      : '<span class="no">&mdash;</span>';
    const ai = r.ai ? '<span class="yes">&#10003;</span>' : '<span class="no">&mdash;</span>';
    const gap = (r.cat === 'animate' && !r.laSlug)
      ? ' <span class="warn-inline">(lucide-animated 无此枚)</span>' : '';
    return '<div class="row" data-cat="' + r.cat + '">'
      + '<span class="cat ' + LABEL[r.cat] + '">' + NAME[r.cat] + '</span>'
      + '<span class="gl"><svg viewBox="0 0 24 24" aria-hidden="true">'
      + (GLYPH[r.name] || '') + '</svg></span>'
      + '<span class="nm">' + esc(r.name) + '<span class="sl">' + esc(r.slug) + '</span></span>'
      + '<span class="la">' + la + '</span>'
      + '<span class="ai">' + ai + '</span>'
      + '<span class="ry">' + esc(r.why) + gap + '</span>'
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

HTML = (TEMPLATE
        .replace("__CMP__", cmp_rows)
        .replace("__MIG__", mig_rows)
        .replace("__COSTS__", cost_rows)
        .replace("__ROWS__", "".join(tile(r) for r in rows))
        .replace("__DATA__", payload)
        .replace("__GLYPH__", gmap)
        .replace("__N_ANIM__", str(counts.get("animate", 0)))
        .replace("__N_OPT__", str(counts.get("optional", 0)))
        .replace("__N_STATIC__", str(counts.get("static", 0)))
        .replace("__N_EX__", str(counts.get("existing", 0))))

open(OUT, "w", encoding="utf-8").write(HTML)
print("wrote", OUT, len(HTML), "bytes | counts", counts)
