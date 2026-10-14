#!/usr/bin/env python3
"""Emit the dataset for the lucide-animated assessment page.

Handles the naming work between lucide canonical names, the app's slugs, and the
slugs lucide-animated.com actually publishes (it follows Lucide's newer renames
only partially, and ships a few extra names: delete / upvote / downvote).
"""
import json, sys

FINAL, AI_NAMES, LA_SLUGS, OUT = sys.argv[1:5]

data = json.load(open(FINAL, encoding="utf-8"))
app = {}
for it in data["lucide"]:
    app[it["name"]] = it["slug"]
# animated-only names are the repo's own icons; keep them for the glyph map
anim_names = {it["name"] for it in data["animated"]}

# canonical -> slug published by lucide-animated.com
LA_OF = {
    "Trash2": "delete",
    "CircleQuestionMark": "circle-help",
    "Volume2": "volume",
    "FlaskConical": "flask",
    "LogIn": "login",
    "LogOut": "logout",
    "Paperclip": "attach-file",   # LA renamed Paperclip -> AttachFile
    "ThumbsUp": "upvote",         # LA ships thumb-up as Upvote
    "ThumbsDown": "downvote",     # ...and thumb-down as Downvote
}
# canonical -> the component/import name inside lucide-animated
COMP = {
    "Trash2": "Delete", "CircleQuestionMark": "CircleHelp", "Volume2": "Volume",
    "FlaskConical": "Flask", "LogIn": "Login", "LogOut": "Logout",
    "Paperclip": "AttachFile", "ThumbsUp": "Upvote", "ThumbsDown": "Downvote",
}

ai = set(json.load(open(AI_NAMES, encoding="utf-8")))
la = {l.strip() for l in open(LA_SLUGS, encoding="utf-8") if l.strip()}

rows = []
for name in sorted(app, key=str.lower):
    slug = app[name]
    la_slug = LA_OF.get(name, slug)
    rows.append({
        "name": name,
        "slug": slug,
        "laSlug": la_slug if la_slug in la else None,
        "laName": COMP.get(name, name) if la_slug in la else None,
        "ai": name in ai,
        "existing": name in anim_names,
    })

REASONS = {
    "spatial": "方向 / 位置语义，动效即位移，会含混指代",
    "status": "状态事实，没有发生的瞬间",
    "chrome": "高频外壳，同屏多个，动起来互相干扰",
    "filetype": "文件类型标识，一屏十几个，外框须稳定",
    "layout": "布局 / 静止态，没有可类比的物理动作",
    "identity": "身份 / 偏好取值，一动就是假的状态提示",
    "object": "名词性实体，找不到自然且对等的动作",
}
STATIC = {}
for n in ["ArrowDown","ArrowUp","ArrowLeft","ArrowRight","ArrowUpDown","ArrowUpRight",
          "ChevronDown","ChevronLeft","ChevronRight","ChevronUp","ChevronsUpDown",
          "CornerDownLeft","LogIn","LogOut","Upload","Download"]:
    STATIC[n] = "spatial"
for n in ["CircleAlert","TriangleAlert","CircleCheck","CircleCheckBig","CircleX","Info",
          "CircleQuestionMark","Shield","WifiOff"]:
    STATIC[n] = "status"
for n in ["X","Plus","Check","Minus","Ellipsis","Copy","Pencil","ExternalLink",
          "Paperclip","Eraser"]:
    STATIC[n] = "chrome"
for n in ["File","FileText","FileCode","FileCode2","FileSpreadsheet","FileImage",
          "FileJson","FileType","FileArchive","FileClock","FilePlay","FileStack",
          "FileWarning","FileQuestionMark","FilePen","Files","Archive","Presentation",
          "FolderInput"]:
    STATIC[n] = "filetype"
for n in ["Maximize","Maximize2","Minimize","Minimize2","Square","Circle","CircleSmall",
          "Dot","Layers","Network","MonitorUp","TextWrap","Braces","PanelLeft","PanelRight"]:
    STATIC[n] = "layout"
for n in ["Mars","MarsStroke","Venus","VenusAndMars","NonBinary","Transgender",
          "Bookmark","BookmarkCheck","Pin","PinOff"]:
    STATIC[n] = "identity"
for n in ["Activity","Gauge","Orbit","FlaskConical","Quote","Code","CodeXml","Terminal",
          "Cpu","HardDrive","Server","Monitor","Database","Globe","Earth","Clock","Clock3",
          "CalendarDays","Keyboard","Link2","ListFilter","ListTodo","ListTree",
          "MessageSquare","MessageSquareText","Image","ImagePlus","Images","Folder",
          "FolderOpen","FolderSearch","Folders","UserRound","PackageOpen","Book","BookCopy",
          "BookOpen","Library","NotebookPen","ClipboardList","Cable","Puzzle","Video",
          "Music","Music2","GitFork","GitCommitHorizontal","Webhook","Settings2","RotateCw"]:
    STATIC[n] = "object"
# category membership only; the display text lives in ANIM_WHY / OPT_WHY below
ANIMATE = {
    "Bell","BellRing","LoaderCircle","RefreshCw","RotateCcw","Send","Sparkles",
    "WandSparkles","Trash2","CircleStop","CirclePause","Play","Pause","Mic","Eye",
    "EyeOff","ThumbsUp","ThumbsDown","Zap","Wrench","FolderPlus","MessageCircleMore","Target",
}

ANIM_WHY = {
    "Bell": "通知到来，铃铛弹一下是这套语言里最有说服力的动作",
    "BellRing": "未读提醒，同上",
    "LoaderCircle": "本身就是「正在处理」，全应用出现最多（94 次）",
    "RefreshCw": "刷新，语义就是一次循环动作",
    "RotateCcw": "撤销 / 重置，回转动作与含义一一对应",
    "Send": "发送，点击瞬间前冲是对「已触发」的确认",
    "Sparkles": "AI 生成中，闪光就是「正在发生」的通用信号",
    "WandSparkles": "技能 / 魔法动作，与 Sparkles 同族",
    "Trash2": "删除不可逆，抖动起「确定要删？」的提醒作用（39 次）",
    "CircleStop": "停止生成，按下时方块收紧符合终止直觉",
    "CirclePause": "暂停生成，同一组控制里的动作反馈",
    "Play": "运行 / 播放，是动作而非状态",
    "Pause": "暂停，同上",
    "Mic": "录音是有持续时长的状态，需一直提示「还在录」",
    "Eye": "展开 / 查看，眨眼是天然的「揭示」隐喻",
    "EyeOff": "隐藏密码，闭眼，与 Eye 成对",
    "ThumbsUp": "反馈按钮，点下去要有回应",
    "ThumbsDown": "同上",
    "Zap": "快捷操作 / 触发",
    "Wrench": "工具入口，拧一下的动作自然存在",
    "FolderPlus": "新建文件夹，是明确的新增动作",
    "MessageCircleMore": "侧栏「对话设置」，气泡弹动与语义一致",
    "Target": "目标 / 定位，准星收敛呼应「锁定到某个结果」",
}
OPT_WHY = {
    "Copy": "只在「已复制」需要即时确认的地方（代码块、密钥）值得动；作为工具栏高频按钮则不适合。",
    "Volume2": "只在音量确实变化时才有事件可表达，否则会让人以为被静音了。",
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
    "Brain": "只有「正在思考」的实时状态值得动。",
    "Sun": "仅主题切换控件；作为当前主题的展示不要动。",
    "Moon": "同上。",
    "SunMoon": "主题切换控件，已经动画化。",
    "LogOut": "退出按钮，点击时提示一次即可。",
    "WifiOff": "离线是可恢复状态，重连成功的那一下才值得动。",
    "ShieldCheck": "校验通过的那一瞬间适合；作为常驻安全标记不适合。",
    "Bookmark": "收藏动作可以动；已收藏的状态不要动。",
    "Check": "提交成功的那一下可以动；列表里的选中态不要动。",
}
OPTIONAL = {
    "Copy","Volume2","Heart","Star","History","KeyRound","Package","PackagePlus","PackageX",
    "Download","Upload","Bot","Brain","Sun","Moon","SunMoon","LogOut","WifiOff","ShieldCheck",
    "Bookmark","Check",
}
EXISTING = {"Bell","Blocks","Maximize","Minimize","MessageCirclePlus","PanelLeft",
            "PanelRight","Search","Settings","SunMoon","Users"}

out = []
for r in rows:
    n = r["name"]
    if n in EXISTING:
        r["cat"], r["why"] = "existing", "仓库内已实现动效，见第 03 节对照表"
    elif n in ANIMATE:
        r["cat"], r["why"] = "animate", ANIM_WHY.get(n, "动作 / 状态变化驱动")
    elif n in OPTIONAL:
        r["cat"], r["why"] = "optional", OPT_WHY.get(n, "库里，但有条件——取决于放在哪个位置")
    else:
        g = STATIC.get(n, "object")
        r["cat"], r["why"] = "static", REASONS[g]
    # the site has no icon for this one, even though it is worth animating
    if r["laSlug"] is None:
        r["gap"] = True
    out.append(r)

counts = {}
for r in out:
    counts[r["cat"]] = counts.get(r["cat"], 0) + 1

json.dump({"rows": out, "counts": counts}, open(OUT, "w", encoding="utf-8"),
          ensure_ascii=False)
print("rows", len(out), "counts", counts)
print("with lucide-animated:", sum(1 for r in out if r["laSlug"]))
print("recommended22 covered by LA:",
      sum(1 for r in out if r["cat"] == "animate" and r["laSlug"]),
      "/", sum(1 for r in out if r["cat"] == "animate"))
