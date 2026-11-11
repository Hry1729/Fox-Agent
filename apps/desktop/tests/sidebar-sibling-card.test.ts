// SPEC item 4（缺口 A）：右侧栏与联系人卡都是**独立的同级圆角卡片**，
// 默认 1060 宽也走原生增长，不再有 1180px 覆盖层抽屉，也没有断点震荡。
//
// Workbench 需要整套 Tauri IPC 才能挂载，所以这里断言的是**真实源码与真实 CSS**
// 里的合同：渲染层级（卡片是同级的）、增长声明的条件（没有断点排除项）、CSS 里
// 同级卡片的外边距/圆角一致性，以及能力声明与 tauri.conf.json 的默认几何。
// 可执行的行为（1060 增长、最大化不叠加）在 sidebar-window-growth.test.ts 与
// sidebar-window-growth-hook.test.tsx 里用假原生窗口跑真数字。
import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'

const read = (relative: string) => readFileSync(new URL(relative, import.meta.url), 'utf8')
const collapse = (source: string) => source.replace(/\s+/g, ' ')

const workbench = read('../src/features/chat/workbench.tsx')
const flatWorkbench = collapse(workbench)
const css = read('../src/styles/workbench.css')
const flatCss = collapse(css)
const sideCardSource = read('../src/features/companions/contact-side-card.tsx')
const sideCardCss = read('../src/features/companions/contact-side-card.css')
const rightPanel = read('../src/features/chat/right-panel/fox-right-panel.tsx')
const controller = read('../src/features/workspace/use-sidebar-window-growth.ts')
const lanyardCss = read('../src/features/chat/components/expert-lanyard.css')
const capabilities = JSON.parse(read('../src-tauri/capabilities/default.json')) as { permissions: string[] }
const tauriConfig = JSON.parse(read('../src-tauri/tauri.conf.json')) as { app: { windows: Array<{ width: number; minWidth: number }> } }

/**
 * 「把某个类名自己藏起来」的判定：只看每条规则**最后一个复合选择器**是不是这个类
 * （允许附加类 / 伪类 / 伪元素 / 属性选择器），并且块内声明 `display: none`。
 * 这样 `.fox-workbench > .fox-resize-divider { display: none }` 仍然算违规，而
 * `.fox-resize-divider span { display: none; }`（分隔条内部那个占位 span，与断点
 * 隐藏无关）不再被宽泛的 `[^{]*` 前缀误判。
 */
/** 组合符（后代 / 子代 / 相邻 / 通用兄弟）之后的那一段复合选择器。 */
const lastCompound = (branch: string) => branch.trim().split(/[\s>+~]+/).pop() ?? ''
/** 附加类 / 伪类（可带参数）/ 伪元素 / 属性选择器，可以连续出现。 */
const QUALIFIER = '(?:\\.[\\w-]+|::?[a-z-]+(?:\\([^)]*\\))?|\\[[^\\]]*\\])*$'
/** 类名本身允许转义字符，构造正则前先转义。 */
const escaper = (value: string) => value.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')
function hidesItself(cssText: string, className: string) {
  const subject = new RegExp(`^\\.${escaper(className)}(?![\\w-])${QUALIFIER}`)
  // 顶层规则：选择器 → 不含嵌套的声明块。@media 内的规则同样被单独取到。
  return [...cssText.matchAll(/([^{}]+)\{([^{}]*)\}/g)].some(([, selector, body]) =>
    /display:\s*none/.test(body)
    && selector.split(',').some((branch) => subject.test(lastCompound(branch))))
}

describe('同级侧栏卡片（缺口 A）', () => {
  test('workbench 不再有紧凑覆盖层抽屉，也不再有 1180px 断点状态', () => {
    for (const dead of ['CompactRightPanel', 'compactLayout', 'compactRightOpen', 'has-open-right-panel', 'fox-compact-context', 'max-width: 1180px']) {
      expect(workbench.includes(dead)).toBe(false)
    }
    // 面板只剩一条渲染路径：同级卡片，且不带 compact 属性。
    expect(workbench.match(/<RightPanel compact/g)?.length ?? 0).toBe(0)
    expect((workbench.match(/<RightPanel /g) ?? []).length).toBe(1)
    expect((workbench.match(/<CompactRightPanel/g) ?? []).length).toBe(0)
  })

  test('普通右侧栏与联系人卡都是 fox-workbench 的同级子节点，不再是 content-card 的覆盖层', () => {
    const cardOpen = flatWorkbench.indexOf('<div className="fox-content-card">')
    const cardClosed = flatWorkbench.indexOf('</section> </div> </div>')
    const panelRender = flatWorkbench.indexOf('{showConversationRightSidebar && !rightSidebarCollapsed && <>')
    const sideCardRender = flatWorkbench.indexOf('{contactCardOpen && contact && <ContactSideCard')
    expect(cardOpen).toBeGreaterThan(-1)
    expect(cardClosed).toBeGreaterThan(cardOpen)
    expect(panelRender).toBeGreaterThan(cardClosed)
    expect(sideCardRender).toBeGreaterThan(panelRender)
    // 分隔条与面板成对出现在同级位置（分隔条占住既有的 5px 间隙）。
    expect(flatWorkbench.slice(panelRender, panelRender + 220)).toContain('<ResizeDivider')
  })

  test('增长声明只看卡片是否开着：默认 1060 宽也长，最大化面板只是压缩聊天', () => {
    const memo = workbench.slice(workbench.indexOf('const sidebarGrowthEntries = useMemo'), workbench.indexOf('const handleSidebarGrowthRefused'))
    expect(memo).toContain("if (showConversationRightSidebar && !rightSidebarCollapsed && !rightPanelMaximized) entries.push({ id: 'right-panel', width: rightWidth })")
    expect(memo).toContain("if (contactCardOpen) entries.push({ id: 'contact-card', width: CONTACT_SIDE_CARD_WIDTH })")
    // 没有任何视口断点或覆盖层条件参与增长判断。
    expect(/compact|matchMedia|1180|innerWidth/.test(memo)).toBe(false)
    expect(workbench).toContain('useSidebarWindowGrowth(sidebarGrowthEntries, handleSidebarGrowthRefused, sidebarLayoutRef)')
    expect(workbench).toContain('<div className="fox-workbench" ref={sidebarLayoutRef}>')
  })

  test('CSS：同级卡片共享 content-card 的外边距与圆角，5px 间隙等宽', () => {
    expect(flatCss).toContain('.fox-workbench > .fox-context-panel { flex: none; margin: 2px 5px 5px 0;')
    expect(flatCss).toContain('.fox-workbench > .fox-resize-divider { margin: 0 -5px 0 0; }')
    // content-card 与同级面板的 radius 声明完全一致。
    const radius = 'border-radius: calc(var(--fox-radius-window) + 4px) calc(var(--fox-radius-window) + 4px) var(--fox-radius-window) calc(var(--fox-radius-window) + 4px);'
    expect(css).toContain(radius)
    expect(sideCardCss).toContain(radius)
    // 两张卡同时开着时，面板的右边缘面对间隙而不是窗口边缘。
    expect(sideCardCss).toContain('.fox-workbench:has(.fox-contact-side-card) > .fox-context-panel:not(.is-maximized)')
    // 底边左侧角曾经过小（缺口 A 的圆角不一致）：侧栏卡片不再用裸窗口半径收尾。
    expect(sideCardCss.includes('var(--fox-radius-window) var(--fox-radius-window);')).toBe(false)
  })

  test('CSS：断点不再隐藏右侧栏，也没有覆盖层专用规则或导轨隐藏', () => {
    expect(css).toContain('@media (max-width: 1180px) {\n  .fox-chat-top-actions { margin-right: 0; }\n}')
    // 分隔条与同级面板都是默认可见的：样式确实存在，而且只给内部占位 span 让路。
    expect(flatCss).toContain('.fox-resize-divider { position: relative; z-index: 30; width: 5px;')
    expect(flatCss).toContain('.fox-workbench > .fox-resize-divider { margin: 0 -5px 0 0; }')
    // 任何地方都不再把同级卡片或分隔条本身藏起来（断点震荡的根源）。
    expect(hidesItself(css, 'fox-context-panel')).toBe(false)
    expect(hidesItself(css, 'fox-resize-divider')).toBe(false)
    expect(hidesItself(css, 'fox-contact-side-card')).toBe(false)
    // 反向确认这个判定不是空转：真实的隐藏规则会被抓到，无论是哪种写法。
    expect(hidesItself('.fox-workbench > .fox-resize-divider { display: none; }', 'fox-resize-divider')).toBe(true)
    expect(hidesItself('.fox-workbench>.fox-resize-divider{display:none}', 'fox-resize-divider')).toBe(true)
    expect(hidesItself('.fox-resize-divider.is-hidden { display: none; }', 'fox-resize-divider')).toBe(true)
    expect(hidesItself('@media (max-width: 900px) { .fox-context-panel:not(.is-maximized) { display: none } }', 'fox-context-panel')).toBe(true)
    // ……而「只藏子元素」不会被误判成藏整条卡片。
    expect(hidesItself('.fox-resize-divider span { display: none; }', 'fox-resize-divider')).toBe(false)
    expect(hidesItself('.fox-other, .fox-resize-divider span { display: none; }', 'fox-resize-divider')).toBe(false)
    expect(css.includes('.fox-compact-context-layer')).toBe(false)
    expect(css.includes('.fox-compact-context-sheet')).toBe(false)
    expect(css.includes('.fox-content-card.has-open-right-panel')).toBe(false)
    expect(css.includes('has-open-right-panel')).toBe(false)
    expect(lanyardCss.includes('.fox-compact-context-layer')).toBe(false)
  })

  test('面板最大化仍是同级卡片：靠 flex 抢宽度，不用覆盖层，也不声明增长', () => {
    expect(flatCss).toContain('.fox-workbench > .fox-context-panel.is-maximized { flex: 1 1 auto; width: auto; max-width: none; }')
    expect(rightPanel).toContain('${maximized ? \'is-maximized\' : \'\'}')
    expect(rightPanel).toContain('onToggleMaximized')
    expect(rightPanel).toContain('onCollapse')
  })

  test('原生合同：能力声明覆盖控制器用到的每个调用，默认几何与常量一致', () => {
    for (const permission of [
      'core:window:allow-inner-size',
      'core:window:allow-outer-size',
      'core:window:allow-outer-position',
      'core:window:allow-scale-factor',
      'core:window:allow-set-size',
      'core:window:allow-set-min-size',
      'core:window:allow-set-position',
      'core:window:allow-current-monitor',
      'core:window:allow-is-maximized',
      'core:window:allow-is-fullscreen',
    ]) expect(capabilities.permissions).toContain(permission)
    expect(tauriConfig.app.windows[0]!.width).toBe(1060)
    expect(tauriConfig.app.windows[0]!.minWidth).toBe(600)
    expect(controller).toContain('export const BASE_MIN_WIDTH = 600')
    expect(controller).toContain('export const SIDEBAR_CARD_GAP = 5')
    expect(sideCardSource).toContain('export const CONTACT_SIDE_CARD_WIDTH = 320')
  })

  test('缺口 B：几何只用原生 workArea，不伪造 Window.currentMonitor，也不回退 screen.avail', () => {
    expect(controller).toContain("from '@tauri-apps/api/window'")
    expect(controller).toContain('currentMonitor()')
    expect(controller).toContain('monitor.workArea')
    expect(controller.includes('screen.avail')).toBe(false)
    expect(controller.includes('workAreaSize')).toBe(false)
    // Window 适配器里没有 `currentMonitor` 方法（那是模块导出的函数）。
    const interfaceSource = controller.slice(controller.indexOf('export interface SidebarWindowLike'), controller.indexOf('export interface SidebarWindowGrowthDeps'))
    expect(interfaceSource.includes('currentMonitor')).toBe(false)
    expect(controller.includes(' as unknown as ')).toBe(false)
  })
})
