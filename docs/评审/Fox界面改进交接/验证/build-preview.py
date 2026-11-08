"""把真实构建 CSS 与真实组件片段内联成一份可离线打开的预览 HTML。

输入：
  apps/desktop/dist/assets/*.css        产品真实构建产物（逐字合并，仅重写绝对路径）
  验证/真实组件片段.json                 render-real.tsx 渲染的真实组件 DOM
  验证/preview-template.html            页面模板
输出：
  真实改进预览.html                     自包含（图片/字体仍指向 apps/desktop/dist 真实资源）

运行： python 验证/build-preview.py
"""
from __future__ import annotations

import io
import json
import re
import base64
from datetime import datetime
from pathlib import Path

HERE = Path(__file__).resolve().parent
REPO = HERE.parents[3]  # 验证 -> 交接 -> 评审 -> docs -> Fox(根)
ASSETS = REPO / 'apps' / 'desktop' / 'dist' / 'assets'
DIST_PREFIX = '../../../apps/desktop/dist/'

CSS_FILES = [
    'index-DxrMkQVk.css',                              # 主 bundle：tailwind + themes + globals + 字体
    'workbench-YYhw9Pkp.css',                          # 工作台 / 过程 / 侧栏
    'fox-assistant-ui-chat-5e27r6OB.css',              # 会话消息
    'settings-pages-C5bmnBP2.css',                     # 模型服务设置
    'notification-center-implementation-Dh8RZWx1.css', # 通知中心
    'skeleton-BRNxd7Q3.css',
    'index-C8sqlqJS.css',                              # 组件封装样式
]

css_parts: list[str] = []
manifest: list[dict[str, object]] = []
for name in CSS_FILES:
    path = ASSETS / name
    text = path.read_text(encoding='utf-8')
    if '@import' in text:
        raise SystemExit(f'{name} 含 @import，内联顺序需要重新确认')
    if '</style' in text.lower():
        raise SystemExit(f'{name} 含 </style，不能直接内联')
    # 构建产物把字体/图片写成 /assets/... 的绝对站点路径；离线打开时改成相对真实 dist 的路径。
    text = re.sub(r'url\(/', lambda m: 'url(' + DIST_PREFIX, text)
    css_parts.append(f'/* ===== apps/desktop/dist/assets/{name} ===== */\n{text}')
    manifest.append({'name': f'apps/desktop/dist/assets/{name}', 'bytes': path.stat().st_size})

# 把改前证据、头像与吉祥物内联，页面在 file:// 与本地静态服务下都能完整显示。
# 字体分片数量太多，仍相对引用真实 dist；在线预览面板会回退系统字体，双击本地文件可加载真实字体。
def png_data(path: Path) -> str:
    return 'data:image/png;base64,' + base64.b64encode(path.read_bytes()).decode('ascii')

shots = REPO / 'docs' / 'windows-test' / 'run-1339' / 'shots-1339'
inline_images = {name: png_data(shots / name) for name in
                 ['R1-6-card.png', 'B4-5-toast.png', 'R2-4-menu.png', '_tmp-compose.png']}
inline_images['fox_sit_nicely.png'] = png_data(REPO / 'apps/desktop/dist/mascot/fox/idle/fox_sit_nicely.png')
inline_images['fox-assistant.png'] = png_data(REPO / 'apps/desktop/dist/avatars/fox-assistant.png')

data = json.loads((HERE / '真实组件片段.json').read_text(encoding='utf-8'))
data['cssFiles'] = manifest
data['buildTime'] = datetime.now().astimezone().isoformat(timespec='seconds')
data['inlineImages'] = inline_images

# 组件片段里的 /avatars/、/mascot/ 等站点绝对路径同样改为相对真实 dist 的路径。
for fragment in data['fragments'].values():
    fragment['html'] = re.sub(
        r'(src|href)="/([^"]+)"',
        lambda m: f'{m.group(1)}="{DIST_PREFIX}{m.group(2)}"',
        fragment['html'],
    )
    fragment['html'] = fragment['html'].replace(
        DIST_PREFIX + 'avatars/fox-assistant.png', inline_images['fox-assistant.png'])

payload = json.dumps(data, ensure_ascii=False, separators=(',', ':'))
payload = re.sub(r'</script', r'<\\/script', payload, flags=re.I)

template = (HERE / 'preview-template.html').read_text(encoding='utf-8')
html = (template
        .replace('/*__INLINE_CSS__*/', '\n'.join(css_parts))
        .replace('__INLINE_DATA__', payload)
        .replace('__BUILD_TIME__', str(data['buildTime'])))

out = HERE.parent / '真实改进预览.html'
out.write_text(html, encoding='utf-8')
print(f'wrote {out}  ({out.stat().st_size:,} bytes)')
print(f'css: {len(css_parts)} files, {sum(m["bytes"] for m in manifest):,} bytes')
print(f'fragments: {len(data["fragments"])}')
