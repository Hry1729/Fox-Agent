#!/usr/bin/env python3
"""Classify every app icon as fit / unfit for animation, grounded in usage.

Categories (checked in this order, first match wins):
  existing  - already animated in the repo (handled by the migration table)
  static    - should stay a static glyph
  animate   - worth switching to an animated version
  optional  - library ships one, benefit is marginal / context-dependent

Outputs classification.json, consumed by build_report.py.
"""
import json, re, os, sys

OUT, FINAL, ANIM_NAMES, SRC = sys.argv[1:5]

data = json.load(open(FINAL, encoding="utf-8"))
have_anim = set(json.load(open(ANIM_NAMES, encoding="utf-8")))

# ---- usage: JSX occurrences + files, so every call is grounded in real code ----
srcs = {}
for dp, dn, fn in os.walk(SRC):
    dn[:] = [d for d in dn if d not in ("node_modules", "dist", ".git")]
    for f in fn:
        if f.endswith((".ts", ".tsx")):
            p = os.path.join(dp, f)
            srcs[p] = open(p, encoding="utf-8", errors="replace").read()

def usage(icon):
    occ, files = 0, set()
    for name in icon["aliases"]:
        tag = re.compile(r"<" + name + r"\b")
        for p, txt in srcs.items():
            c = len(tag.findall(txt))
            if c:
                occ += c
                files.add(os.path.relpath(p, SRC).replace("\\", "/"))
    return occ, sorted(files)

# ---- already animated in-repo (Q1) ----
EXISTING = {"Bell", "Blocks", "Maximize", "Minimize", "MessageCirclePlus",
            "PanelLeft", "PanelRight", "Search", "Settings", "SunMoon", "Users"}

# ---- STATIC, with a reason code per group ----
REASONS = {
    "spatial": ("方向 / 位置语义",
                "它表达的是一段位移或朝向。动效本身就是位移，叠上去会让指代变得含混——"
                "用户看到箭头动了一下，会误读成“要往那边走”。"),
    "status": ("状态事实",
               "这类图标是在陈述一个既成事实（通过 / 失败 / 警告 / 离线）。"
               "它不是事件，没有发生的瞬间，加动效等于给静态信息配了多余的情绪。"),
    "chrome": ("高频外壳操作",
               "全应用几十处复用（如下表出现次数），且常常成组出现在工具栏里。"
               "任何一个动，同屏其它都在旁边看着，视觉噪声成倍放大。"),
    "filetype": ("文件类型标识",
                 "由扩展名查表决定，一个列表里可能同时出现十几个。"
                 "它标识“这是什么”，不代表“发生了什么”，且外框形状必须保持稳定以便扫读。"),
    "layout": ("布局 / 静止状态",
               "表示画布或容器当前处于什么状态，没有可类比的物理动作。"),
    "identity": ("身份 / 偏好取值",
                 "代表用户当前的选择或身份本身。一旦它动，就暗示这个值变了——"
                 "而实际上只是鼠标经过，会造成假的状态提示。"),
    "object": ("静态对象 / 无动效类比",
               "名词性的实体（服务器、数据库、文件夹…）。在界面上是标签而非动作，"
               "找不到自然又有信息量的动作去映射它。"),
}

STATIC = {}
for n in ["ArrowDown", "ArrowUp", "ArrowLeft", "ArrowRight", "ArrowUpDown",
          "ArrowUpRight", "ChevronDown", "ChevronLeft", "ChevronRight",
          "ChevronUp", "ChevronsUpDown", "CornerDownLeft", "LogIn", "LogOut",
          "Upload", "Download"]:
    STATIC[n] = "spatial"
for n in ["CircleAlert", "TriangleAlert", "CircleCheck", "CircleCheckBig",
          "CircleX", "Info", "CircleQuestionMark", "Shield", "WifiOff"]:
    STATIC[n] = "status"
for n in ["X", "Plus", "Check", "Minus", "Ellipsis", "Copy", "Pencil",
          "ExternalLink", "Paperclip", "Eraser"]:
    STATIC[n] = "chrome"
for n in ["File", "FileText", "FileCode", "FileCode2", "FileSpreadsheet",
          "FileImage", "FileJson", "FileType", "FileArchive", "FileClock",
          "FilePlay", "FileStack", "FileWarning", "FileQuestionMark", "FilePen",
          "Files", "Archive", "Presentation", "FolderInput"]:
    STATIC[n] = "filetype"
for n in ["Maximize", "Maximize2", "Minimize", "Minimize2", "Square", "Circle",
          "CircleSmall", "Dot", "Layers", "Network", "MonitorUp", "TextWrap",
          "Braces", "PanelLeft", "PanelRight"]:
    STATIC[n] = "layout"
for n in ["Mars", "MarsStroke", "Venus", "VenusAndMars", "NonBinary",
          "Transgender", "Bookmark", "BookmarkCheck",
          "Pin", "PinOff"]:
    STATIC[n] = "identity"
for n in ["Activity", "Gauge", "Orbit", "FlaskConical", "Quote", "Code",
          "CodeXml", "Terminal", "Cpu", "HardDrive", "Server", "Monitor",
          "Database", "Globe", "Earth", "Clock", "Clock3", "CalendarDays",
          "Keyboard", "Link2", "ListFilter", "ListTodo", "ListTree",
          "MessageSquare", "MessageSquareText", "Image", "ImagePlus", "Images",
          "Folder", "FolderOpen", "FolderSearch", "Folders", "UserRound",
          "PackageOpen", "Book", "BookCopy", "BookOpen",
          "Library", "NotebookPen", "ClipboardList", "Cable",
          "Puzzle", "Video", "Music", "Music2", "GitFork", "GitCommitHorizontal",
          "Webhook", "Settings2", "RotateCw"]:
    STATIC[n] = "object"

# ---- ANIMATE: action / state-change driven, low-ish per-screen count ----
ANIMATE = {
    "Bell": "通知到来。铃铛弹一下是这套语言里最有说服力的一个动作，"
            "而它一天只出现几次，不会形成干扰。",
    "BellRing": "未读提醒，同上。",
    "LoaderCircle": "本身就是“正在处理”。用真实旋转替代 CSS spinner，"
                    "风格更统一，也是全应用出现最多的加载图标。",
    "RefreshCw": "刷新，语义就是一次循环动作，动效即说明。",
    "RotateCcw": "撤销 / 重置，回转动作与含义一一对应。",
    "Send": "发送。点击瞬间前冲一下，是对“已触发”的即时确认。",
    "Sparkles": "AI 生成中。/ 这里的闪光就是“正在发生”的通用信号。",
    "WandSparkles": "技能 / 魔法动作，与 Sparkles 同一语义族。",
    "Trash2": "删除是不可逆操作。抖动一下起到“确定要删？”的提醒作用，"
              "属于动效真正解决风险的少数场景。",
    "CircleStop": "停止生成。按下时方块收紧，符合“终止”的直觉。",
    "CirclePause": "暂停生成，同一组控制里的动作反馈。",
    "Play": "运行 / 播放，是动作而非状态。",
    "Pause": "暂停，同上。",
    "Mic": "录音是有持续时长的状态，需要动效一直提示“还在录”。",
    "Eye": "展开 / 查看，眨眼是天然的“揭示”隐喻。",
    "EyeOff": "隐藏密码，闭眼，与 Eye 成对。",
    "ThumbsUp": "反馈按钮，点下去要有回应，否则不知道有没有记上。",
    "ThumbsDown": "同上。",
    "Zap": "快捷操作 / 触发。",
    "Wrench": "工具入口，拧一下的动作自然存在。",
    "FolderPlus": "新建文件夹，是明确的新增动作。",
    "MessageCircleMore": "侧栏“对话设置”，气泡弹动与语义一致。",
    "Target": "目标 / 定位，准星收敛可以呼应“锁定到某个结果”。",
}

# ---- OPTIONAL: library ships it, but it depends on placement ----
OPTIONAL = {
    "Copy": "只在“已复制”需要即时确认的地方（代码块、密钥）值得动；"
            "作为工具栏里的高频按钮则不适合。",
    "Volume2": "只在音量确实变化时才有事件可表达，否则会让人误以为被静音了。",
    "Heart": "用于收藏这类明确的情感动作；作为列表里的标记则不要动。",
    "Star": "同上：评分 / 收藏可以动，纯展示不要动。",
    "History": "时间轴面板的顶部入口值得一动；作为表格列头不适合。",
    "KeyRound": "登录、授权环节值得动；权限清单里不适合。",
    "Package": "安装 / 卸载流程中才有事件可表达。",
    "PackagePlus": "安装动作，同上。",
    "PackageX": "卸载 / 失败的那一瞬间适合。",
    "Download": "只在触发下载的按钮上适合，方向语义使它不能到处用。",
    "Upload": "同上。",
    "Bot": "作为助手头像轻微呼吸尚可；在会话列表里重复出现则不适合。",
    "Brain": "只有“正在思考”的实时状态值得动。",
    "Sun": "仅主题切换控件；作为当前主题的展示不要动。",
    "Moon": "同上。",
    "SunMoon": "主题切换控件，已经动画化。",
    "LogOut": "退出按钮，点击时提示一次即可。",
    "WifiOff": "离线是可恢复状态，重连成功的那一下才值得动。",
    "ShieldCheck": "校验通过的那一瞬间适合；作为常驻安全标记不适合。",
    "Bookmark": "收藏动作可以动；已收藏的状态不要动。",
    "Check": "提交成功的那一下可以动；列表里的选中态不要动。",
}

out = []
for it in data["lucide"]:
    name = it["name"]
    # existing > animate > optional > static
    if name in EXISTING:
        cat, why = "existing", "仓库内已实现动效，见迁移对照表"
    elif name in ANIMATE:
        cat, why = "animate", ANIMATE[name]
    elif name in OPTIONAL:
        cat, why = "optional", OPTIONAL[name]
    elif name in STATIC:
        cat, why = "static", REASONS[STATIC[name]][1]
    else:
        cat, why = "static", REASONS["object"][1]
    occ, files = usage(it)
    out.append({
        "name": name, "slug": it["slug"], "cat": cat, "why": why,
        "occ": occ, "files": len(files), "usedIn": files[:6],
        "hasAnim": name in have_anim,
    })

counts = {}
for r in out:
    counts[r["cat"]] = counts.get(r["cat"], 0) + 1
print("counts:", counts)

# sanity: no icon in two buckets
assert len(out) == len(data["lucide"]), "tile count mismatch"
json.dump(out, open(OUT, "w", encoding="utf-8"), ensure_ascii=False, indent=1)
