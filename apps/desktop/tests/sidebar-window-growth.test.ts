// SPEC item 4：共享的侧栏窗口增长控制器（原生层行为）。
//
// 假窗口按已安装的 @tauri-apps/api 2.x 真实形状建模，见 sidebar-window-growth-harness.ts。
// 覆盖：DPI、workArea 钳制 + 先左移（钳制不是失败：不回报、不关卡片）、混合 DPI / 负原点、
// 外框边框、关闭按拥有量缩回并保留当前高度、每次原生写之前重读最大化归属、失败重试与
// 部分改写 / min 复位失败后的诚实记账、最大化反复还原不叠加、增长在途时关闭会撤销、
// dispose 缩回、非 Tauri、快速开关。
import { describe, expect, test } from 'bun:test'
import {
  BASE_MIN_HEIGHT,
  BASE_MIN_WIDTH,
  SIDEBAR_CARD_GAP,
  createSidebarWindowGrowth,
  type SidebarWindowGrowth,
} from '../src/features/workspace/use-sidebar-window-growth'
import {
  createHarness,
  createMonitor,
  drain,
  minCalls,
  positionCalls,
  settle,
  sizeCalls,
  type FakeCall,
  type FakeWindow,
} from './sidebar-window-growth-harness'

const contactCard = [{ id: 'contact-card', width: 320 }]
const panelCard = [{ id: 'right-panel', width: 300 }]
const requestedWidths = (win: FakeWindow) => win.requests.filter((call) => call.name === 'setSize').map((call) => call.width)

async function disposeToBaseline(win: FakeWindow, growth: SidebarWindowGrowth) {
  const height = win.state.height
  growth.dispose()
  await drain(win, growth)
  expect(win.state.width).toBe(1060)
  expect(win.state.height).toBe(height)
  expect(win.state.min).toEqual({ width: BASE_MIN_WIDTH, height: BASE_MIN_HEIGHT })
  expect(growth.getAppliedDelta()).toBe(0)
  expect(win.pendingCount()).toBe(0)
  expect(win.listenerCount()).toBe(0)
}

function barrier() {
  let release!: () => void
  const promise = new Promise<void>((resolve) => { release = resolve })
  return { promise, release }
}

function watchSystemWrites(win: FakeWindow) {
  const forbidden: FakeCall[] = []
  const setSize = win.setSize
  const setPosition = win.setPosition
  const setMinSize = win.setMinSize
  win.setSize = (size) => {
    if (win.state.maximized || win.state.fullscreen) forbidden.push({ name: 'setSize', ...size })
    return setSize(size)
  }
  win.setPosition = (position) => {
    if (win.state.maximized || win.state.fullscreen) forbidden.push({ name: 'setPosition', ...position })
    return setPosition(position)
  }
  win.setMinSize = (size) => {
    if ((win.state.maximized || win.state.fullscreen) && size?.width !== 600) forbidden.push({ name: 'setMinSize', ...size })
    return setMinSize(size)
  }
  return forbidden
}

describe('sidebar window growth controller', () => {
  test('WebView 刷新保留原生窗口时先撤销上次侧栏增长，再次打开不累加', async () => {
    const { win, deps } = createHarness()
    let saved: { width: number; delta: number } | null = null
    const session = { ...deps, loadGrowth: () => saved, saveGrowth: (value: typeof saved) => { saved = value } }
    const beforeReload = createSidebarWindowGrowth(session)
    await beforeReload.setSidebars(panelCard)
    expect(win.state.width).toBe(1365)
    expect(saved).toEqual({ width: 1365, delta: 305 })

    // The native window survives a WebView reload; the previous JS context does not.
    const afterReload = createSidebarWindowGrowth(session)
    await afterReload.setSidebars([])
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)
    expect(saved).toBeNull()
    await afterReload.setSidebars(panelCard)
    expect(win.state.width).toBe(1365)
    await afterReload.setSidebars([])
    expect(win.state.width).toBe(1060)
    expect(saved).toBeNull()
    afterReload.dispose()
  })

  test('重启后窗口宽度不匹配旧记录时丢弃记录，不缩小新窗口', async () => {
    const { win, deps } = createHarness()
    let saved: { width: number; delta: number } | null = { width: 1365, delta: 305 }
    const growth = createSidebarWindowGrowth({ ...deps, loadGrowth: () => saved, saveGrowth: (value) => { saved = value } })
    await growth.setSidebars([])
    expect(win.state.width).toBe(1060)
    expect(sizeCalls(win)).toHaveLength(0)
    expect(saved).toBeNull()
    growth.dispose()
  })

  test('默认 1060 宽：Fox 侧栏把窗口向右撑 325，并抬高 min 宽（DPI 感知）', async () => {
    const { win, growth } = createHarness({ scaleFactor: 2, width: 1060, height: 778 })
    const seen: number[] = []
    const unsubscribe = growth.subscribe(() => seen.push(growth.getAppliedDelta()))
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    expect(sizeCalls(win)).toEqual([{ name: 'setSize', width: 1385, height: 778 }])
    expect(win.state.width).toBe(1060 + 320 + SIDEBAR_CARD_GAP)
    expect(win.state.height).toBe(778)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH + 325)
    expect(minCalls(win).at(-1)).toMatchObject({ width: BASE_MIN_WIDTH + 325, height: BASE_MIN_HEIGHT })
    expect(growth.getAppliedDelta()).toBe(320 + SIDEBAR_CARD_GAP)
    expect(seen).toContain(325)
    unsubscribe()
    growth.dispose()
  })

  test('非整数 DPI（1.25）也按逻辑像素增长，物理读数只用于钳制', async () => {
    const { win, growth } = createHarness({ scaleFactor: 1.25, width: 1060, height: 778 })
    growth.setSidebars([{ id: 'right-panel', width: 360 }])
    await settle()
    // 778 逻辑 × 1.25 = 972.5，真实窗口只能落在整数物理像素 973 上，读回来就是
    // 778.4 逻辑。控制器按“读到的当前高度”原样写回：778.4 × 1.25 = 973 物理，
    // 高度一个像素都没变——这正是保留当前高度的要求，不是漂移。
    expect(sizeCalls(win).at(-1)).toMatchObject({ width: 1425, height: 778.4 })
    expect(win.state.width).toBeCloseTo(1424.8, 8)
    expect(win.state.height).toBe(778.4)
    expect((await win.innerSize()).width).toBe(1781)
    expect((await win.innerSize()).height).toBe(973)
    expect(growth.getAppliedDelta()).toBeCloseTo(364.8, 8)
    expect(win.state.min.width).toBeCloseTo(BASE_MIN_WIDTH + 364.8, 8)
    growth.setSidebars([])
    await settle()
    expect(await win.innerSize()).toEqual({ width: 1325, height: 973 })
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(win.state.min.width).toBe(600)
    growth.dispose()
  })

  test('两张卡片同时打开：增量相加，绝不重复计算同一张卡', async () => {
    const { win, growth } = createHarness()
    growth.setSidebars([{ id: 'right-panel', width: 300 }, { id: 'contact-card', width: 200 }])
    await settle()
    expect(win.state.width).toBe(1060 + 305 + 205)
    expect(growth.getAppliedDelta()).toBe(510)
    const before = sizeCalls(win).length
    growth.setSidebars([{ id: 'right-panel', width: 300 }, { id: 'contact-card', width: 200 }])
    await settle()
    // 声明没变但窗口可能变了：会重新对账，但不会再 setSize。
    expect(sizeCalls(win).length).toBe(before)
    growth.dispose()
  })

  test('工作区右缘不够：先在区内左移，再按剩余空间缩减实际增量', async () => {
    // 物理工作区右缘 2400，窗口外框右缘 1060*2+100*2=2320，可用 80 物理 = 40 逻辑。
    const { win, logs, growth } = createHarness({
      scaleFactor: 2,
      origin: { x: 100, y: 50 },
      monitor: createMonitor({ x: 0, y: 0, width: 2400, height: 1200, taskbar: 0 }),
    })
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    // 需要 650 物理，只有 80：左移到工作区左缘（200 物理），可用变为 280 物理 = 140 逻辑。
    expect(positionCalls(win)).toEqual([{ name: 'setPosition', x: 0, y: 50 }])
    expect(win.state.x).toBe(0)
    expect(sizeCalls(win)).toEqual([{ name: 'setSize', width: 1200, height: 778 }])
    expect(growth.getAppliedDelta()).toBe(140)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH + 140)
    expect(logs.some((line) => line.includes('work area too small'))).toBe(true)
    growth.dispose()
  })

  test('窗口边框计入钳制：外框比内容宽 16 物理时，实际增量小 8 逻辑', async () => {
    // 同一块 2400 物理宽的工作区：无边框时可用 = 2400-2320 = 80 物理，左移 200 后 280 → 增量 140；
    // 16 物理边框让外框右缘多 16，可用只剩 264 物理 → 增量 132，正好是边框宽度的逻辑值。
    const { win, growth } = createHarness({
      scaleFactor: 2,
      chrome: { width: 16, height: 30 },
      origin: { x: 100, y: 50 },
      monitor: createMonitor({ width: 2400, height: 1200, taskbar: 0 }),
    })
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    expect(sizeCalls(win)).toEqual([{ name: 'setSize', width: 1192, height: 778 }])
    expect(win.state.width).toBe(1192)
    expect(growth.getAppliedDelta()).toBe(132)
    growth.dispose()
  })

  test('多显示器：负原点与不同 DPI 下仍用物理像素比较', async () => {
    // 副屏在工作区左侧：物理原点 -2400、物理宽 2560 → 工作区右缘 = 物理 160。
    // 窗口在逻辑 -860（@2x = 物理 -1720），外框右缘 -1720+2120 = 400 已越过右缘：
    // 可用 -240 物理。需要 1300 物理，可左移的空间只有 -1720-(-2400) = 680 物理，
    // 于是先左移 680（逻辑 -1200），可用变为 440 物理 = 220 逻辑 → 只长 220，
    // 外框右缘正好贴住工作区右缘 160。
    const { win, logs, growth } = createHarness({
      scaleFactor: 2,
      origin: { x: -860, y: 50 },
      monitor: { workArea: { position: { x: -2400, y: 0 }, size: { width: 2560, height: 1040 } } },
    })
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    expect(positionCalls(win)).toEqual([{ name: 'setPosition', x: -1200, y: 50 }])
    expect(win.state.x).toBe(-1200)
    expect(sizeCalls(win)).toEqual([{ name: 'setSize', width: 1280, height: 778 }])
    expect(win.state.width).toBe(1280)
    expect(win.state.height).toBe(778)
    expect(growth.getAppliedDelta()).toBe(220)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH + 220)
    expect(logs.some((line) => line.includes('work area too small'))).toBe(true)
    growth.dispose()
  })

  test('关闭时只缩回自己拥有的增量，保留用户改过的高度，并还原 min', async () => {
    const { win, growth } = createHarness()
    growth.setSidebars([{ id: 'right-panel', width: 320 }])
    await settle()
    win.state.height = 900
    growth.setSidebars([])
    await settle()
    expect(sizeCalls(win).at(-1)).toMatchObject({ width: 1060, height: 900 })
    expect(win.state.width).toBe(1060)
    expect(win.state.height).toBe(900)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH)
    expect(growth.getAppliedDelta()).toBe(0)
    growth.dispose()
  })

  test('最大化 / 全屏：完全没有 setSize / setMinSize / setPosition；还原后不会二次叠加增量', async () => {
    const { win, growth } = createHarness()
    growth.setSidebars([{ id: 'right-panel', width: 320 }])
    await settle()
    expect(win.state.width).toBe(1385)
    const sizesAfterGrow = sizeCalls(win).length

    // 侧栏开着时用户最大化：清零增量、还原 min，但不做 setSize、不解除最大化。
    win.state.maximized = true
    win.fireResize()
    await settle()
    expect(growth.getAppliedDelta()).toBe(0)
    expect(sizeCalls(win).length).toBe(sizesAfterGrow)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH)

    // 还原：系统交回的是**已经长过的 1385**（不是测试手动重置的 1060）。
    win.state.maximized = false
    win.fireResize()
    await settle()
    expect(sizeCalls(win).length).toBe(sizesAfterGrow)
    expect(win.state.width).toBe(1385)
    expect(growth.getAppliedDelta()).toBe(325)

    // 反复最大化 / 还原三次，宽度不累积成 1385+325。
    for (let cycle = 0; cycle < 3; cycle++) {
      win.state.maximized = true
      win.fireResize()
      await settle()
      win.state.maximized = false
      win.fireResize()
      await settle()
    }
    expect(win.state.width).toBe(1385)
    expect(sizeCalls(win).length).toBe(sizesAfterGrow)
    growth.dispose()
  })

  test('打开前就已最大化 / 全屏：零原生尺寸调用；还原后按当前工作区补一次增长', async () => {
    const { win, growth } = createHarness({ maximized: true })
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    expect(sizeCalls(win)).toEqual([])
    expect(minCalls(win)).toEqual([])
    expect(growth.getAppliedDelta()).toBe(0)

    win.state.maximized = false
    win.fireResize()
    await settle()
    expect(sizeCalls(win)).toEqual([{ name: 'setSize', width: 1385, height: 778 }])
    expect(win.state.width).toBe(1385)
    growth.dispose()
  })

  test('尺寸读取期间被最大化：增长一个原生调用都不发，还原后补上', async () => {
    const harness = createHarness()
    const { win, growth } = harness
    // 模拟用户在 `await innerSize()/outerSize()/currentMonitor()` 之间点了最大化：
    // 钳制用的读数已经过期，只有原生写之前的重新检查能拦住这次增长。只翻一次，
    // 否则还原后的补增长又会被同一个开关拦住。
    const monitor = harness.monitor
    let flips = 0
    harness.deps.getMonitor = async () => { if (flips++ === 0) win.state.maximized = true; return monitor }
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    expect(sizeCalls(win)).toEqual([])
    expect(positionCalls(win)).toEqual([])
    expect(minCalls(win)).toEqual([])
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)

    win.state.maximized = false
    win.fireResize()
    await settle()
    expect(sizeCalls(win)).toEqual([{ name: 'setSize', width: 1385, height: 778 }])
    expect(win.state.width).toBe(1385)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH + 325)
    growth.dispose()
  })

  test('左移之前被最大化：既不移窗也不加宽', async () => {
    // 工作区很紧，本来必然走“先左移再增长”；在 getMonitor 之后翻最大化，
    // 拦住 setPosition。
    const harness = createHarness({
      scaleFactor: 2,
      origin: { x: 100, y: 50 },
      monitor: createMonitor({ width: 2400, height: 1200, taskbar: 0 }),
    })
    const { win, growth } = harness
    const monitor = harness.monitor
    harness.deps.getMonitor = async () => { win.state.maximized = true; return monitor }
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    expect(positionCalls(win)).toEqual([])
    expect(sizeCalls(win)).toEqual([])
    expect(win.state.x).toBe(100)
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
    growth.dispose()
  })

  test('按 Monitor.workArea 而不是整块显示器尺寸钳制（任务栏不被覆盖，卡片不被关掉）', async () => {
    // 工作区只有 2000 物理宽（整块显示器更宽；若误用 monitor.size 就会增长成功）。
    // 窗口外框左移到底也占 2120 物理 → 完全没有增长空间。SPEC §4：这时侧栏保持
    // 目标宽度、聊天变窄，工作区不够**不是**原生调用失败，所以绝不回报拒绝。
    const { win, logs, refused, growth } = createHarness({
      scaleFactor: 2,
      monitor: { workArea: { position: { x: 0, y: 0 }, size: { width: 2000, height: 1500 } } },
    })
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    expect(sizeCalls(win)).toEqual([])
    expect(win.state.width).toBe(1060)
    expect(win.state.x).toBe(0)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(refused).toEqual([])
    expect(logs.some((line) => line.includes('no room left in the work area'))).toBe(true)
    // 再来一次对账（例如原生 resize 通知）仍然稳定：不加宽、不移窗、不回报。
    win.fireResize()
    await settle()
    expect(sizeCalls(win)).toEqual([])
    expect(positionCalls(win)).toEqual([{ name: 'setPosition', x: 0, y: 50 }])
    expect(refused).toEqual([])
    expect(growth.getAppliedDelta()).toBe(0)
    growth.dispose()
  })

  test('工作区只够一部分：按实测 140 增长，之后的 resize 通知不再移动或重复加宽', async () => {
    // 物理工作区右缘 2400，外框右缘 2320 → 可用 80；左移到工作区左缘后 280 物理
    // = 140 逻辑。卡片声明仍是 325，聊天变窄是最后手段，不是失败。
    const { win, logs, refused, growth } = createHarness({
      scaleFactor: 2,
      origin: { x: 100, y: 50 },
      monitor: createMonitor({ x: 0, y: 0, width: 2400, height: 1200, taskbar: 0 }),
    })
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    expect(win.state.width).toBe(1200)
    expect(growth.getAppliedDelta()).toBe(140)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH + 140)
    const sizes = sizeCalls(win).length
    const moves = positionCalls(win).length
    expect(refused).toEqual([])

    // 原生层把这次增长回报成一次 resize 事件：重算后依然只剩 0 空间。
    win.fireResize()
    await settle()
    win.fireResize()
    await settle()
    expect(sizeCalls(win).length).toBe(sizes)
    expect(positionCalls(win).length).toBe(moves)
    expect(win.state.width).toBe(1200)
    expect(win.state.x).toBe(0)
    expect(growth.getAppliedDelta()).toBe(140)
    expect(refused).toEqual([])
    expect(logs.filter((line) => line.includes('work area too small')).length).toBeGreaterThan(0)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)
    expect(growth.getAppliedDelta()).toBe(0)
  })

  test('失败重试之前先看归属：resize 抛错时用户最大化，绝不把第二次 setSize 发出去', async () => {
    // 复现审查意见 1：第一次 setSize 失败，就在失败的那一刻用户点了最大化。
    // 若重试不看归属，第二次 setSize(1385) 会直接打在最大化的窗口上。
    const { win, logs, refused, growth } = createHarness({ setSizeFailures: 1, maximizeOnSetSizeFailure: true })
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    expect(sizeCalls(win).length).toBe(1)
    expect(sizeCalls(win)[0]).toMatchObject({ width: 1385 })
    expect(win.state.maximized).toBe(true)
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
    // 归属交给系统不是原生失败：DOM 卡片保持打开，只是变窄。
    expect(refused).toEqual([])
    expect(logs.some((line) => line.includes('retrying once'))).toBe(false)

    // 还原后按当前工作区补上这次增长。
    win.state.maximized = false
    win.fireResize()
    await settle()
    expect(sizeCalls(win).length).toBe(2)
    expect(sizeCalls(win).at(-1)).toMatchObject({ width: 1385, height: 778 })
    expect(win.state.width).toBe(1385)
    expect(growth.getAppliedDelta()).toBe(325)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH + 325)
    growth.dispose()
  })

  test('min 复位失败一次：重试把它补上，关闭照样缩回 1060', async () => {
    const { win, logs, growth } = createHarness()
    growth.setSidebars([{ id: 'right-panel', width: 320 }])
    await settle()
    expect(win.state.width).toBe(1385)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH + 325)
    // 打开时 min 写得好好的；失败发生在关闭的第一次复位上。
    win.failNextSetMinSize(1)
    growth.setSidebars([])
    await settle()
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(logs.some((line) => line.includes('setMinSize failed') && line.includes('retrying once'))).toBe(true)
    expect(logs.some((line) => line.includes('still failing'))).toBe(false)
    expect(logs.some((line) => line.includes('shrink clamped'))).toBe(false)
    growth.dispose()
  })

  test('min 复位一直失败：setSize「成功」但被旧 min 钳住，仍然拥有那段宽度并能补缩', async () => {
    // 复现审查意见 2：基线 600 → 打开长到 925（min 也抬到 925）→ 关闭时 min 复位
    // 失败，setSize(600) 被旧 min 钳在 925。原生 promise 是成功返回的，几何却不是
    // 请求的那个：拥有量必须按实测算（还是 325），不能记成 0 把这段窗口忘掉。
    const { win, logs, growth } = createHarness({ width: 600, height: 778 })
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    expect(win.state.width).toBe(925)
    expect(growth.getAppliedDelta()).toBe(325)

    win.failNextSetMinSize(2) // 关闭时的两次复位尝试全都抛错
    growth.setSidebars([])
    await settle()
    expect(sizeCalls(win).at(-1)).toMatchObject({ width: 600 })
    expect(win.state.width).toBe(925)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH + 325)
    expect(growth.getAppliedDelta()).toBe(325)
    expect(logs.some((line) => line.includes('still failing'))).toBe(true)
    expect(logs.some((line) => line.includes('shrink clamped') && line.includes('FAILED'))).toBe(true)

    // min 复位现在能成功了：下一次对账（这里用 dispose 的强制对账）把它真正缩回去。
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(600)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH)
    expect(growth.getAppliedDelta()).toBe(0)
  })

  test('setSize 失败重试一次即可；日志说明发生过重试', async () => {
    const { win, logs, growth } = createHarness({ setSizeFailures: 1 })
    growth.setSidebars([{ id: 'right-panel', width: 320 }])
    await settle()
    expect(sizeCalls(win).length).toBe(2)
    expect(win.state.width).toBe(1385)
    expect(growth.getAppliedDelta()).toBe(325)
    expect(logs.some((line) => line.includes('retrying once'))).toBe(true)
    growth.dispose()
  })

  test('原生层部分改写后仍失败：按实测尺寸记账并回报回滚，关闭时缩回真实宽度', async () => {
    const { win, logs, refused, growth } = createHarness({ setSizeFailures: 2, setSizeAppliesAnyway: true })
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    // 两次调用都抛错，但窗口其实已经变宽了 → 增量必须反映现实，而不是当成 0。
    expect(win.state.width).toBe(1385)
    expect(growth.getAppliedDelta()).toBe(325)
    expect(refused.length).toBe(1)
    expect(logs.some((line) => line.includes('refused after retry'))).toBe(true)
    // 回报后 DOM 关掉卡片：这次按真实的 325 缩回去，不能留在 1385。
    growth.setSidebars([])
    await settle()
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
    growth.dispose()
  })

  // 「谁要不到宽度就回滚谁」：声明是一组同级卡片，`entries[length-1]` 只是固定的渲染
  // 顺序（右侧面板先、联系人卡后），不是请求者。控制器改成按「已接受的声明」做 id 对比，
  // 所以下面两种开关顺序必须各自只怪罪新增加的那张卡，而不是关掉另一张。
  test('回滚只看请求者：先开联系人卡、再开右侧面板时只回报面板', async () => {
    const { win, growth, refused } = createHarness()
    growth.setSidebars(contactCard)
    await settle()
    expect(win.state.width).toBe(1385)
    win.failNextSetSize(2)
    growth.setSidebars([...contactCard, { id: 'right-panel', width: 300 }])
    await settle()
    expect(refused).toEqual([[{ id: 'right-panel', width: 300, accepted: null }]])
    expect(win.state.width).toBe(1385)
    // DOM 只关面板：联系人卡那 325px 仍然归它，dispose 时能干净地缩回基线宽度。
    growth.setSidebars(contactCard)
    await settle()
    expect(refused).toHaveLength(1)
    expect(growth.getAppliedDelta()).toBe(325)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
  })

  test('回滚只看请求者：先开右侧面板、再开联系人卡时只回报联系人卡', async () => {
    const { win, growth, refused } = createHarness()
    growth.setSidebars([{ id: 'right-panel', width: 300 }])
    await settle()
    expect(win.state.width).toBe(1365)
    win.failNextSetSize(2)
    growth.setSidebars([{ id: 'right-panel', width: 300 }, ...contactCard])
    await settle()
    // 面板排在声明最前面，但真正要不到宽度的是后开的联系人卡。
    expect(refused).toEqual([[{ id: 'contact-card', width: 320, accepted: null }]])
    expect(win.state.width).toBe(1365)
    expect(growth.getAppliedDelta()).toBe(305)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
  })

  test('已经落地的卡片变宽失败：回报它真正拥有的宽度，DOM 恢复而不是关掉', async () => {
    const { win, growth, refused } = createHarness()
    growth.setSidebars([{ id: 'right-panel', width: 300 }, { id: 'contact-card', width: 320 }])
    await settle()
    expect(win.state.width).toBe(1690)
    win.failNextSetSize(2)
    growth.setSidebars([{ id: 'right-panel', width: 500 }, { id: 'contact-card', width: 320 }])
    await settle()
    // 变宽的那张卡带着 `accepted: 300`（真正落地的宽度）回来，同级的联系人卡不在名单里。
    expect(refused).toEqual([[{ id: 'right-panel', width: 500, accepted: 300 }]])
    expect(win.state.width).toBe(1690)
    expect(growth.getAppliedDelta()).toBe(630)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
  })

  test('重复的失败声明：每次回报同一份回滚，失败的趟次永不进入已接受基线', async () => {
    const { win, growth, refused } = createHarness()
    growth.setSidebars(contactCard)
    await settle()
    win.failNextSetSize(100)
    growth.setSidebars([...contactCard, { id: 'right-panel', width: 300 }])
    await settle()
    growth.setSidebars([...contactCard, { id: 'right-panel', width: 300 }])
    await settle()
    expect(refused).toEqual([
      [{ id: 'right-panel', width: 300, accepted: null }],
      [{ id: 'right-panel', width: 300, accepted: null }],
    ])
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1385, 1690, 1690, 1690, 1690])
    expect(win.state.width).toBe(1385)
    expect(growth.getAppliedDelta()).toBe(325)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1385)
  })

  test('被新声明取代的失败趟不回报任何东西：陈旧的失败不能回滚活着的 DOM', async () => {
    const { win, growth, refused } = createHarness({ defer: true })
    await drain(win, growth, contactCard)
    expect(win.state.width).toBe(1385)
    win.failNextSetSize(2)
    growth.setSidebars([...contactCard, { id: 'right-panel', width: 300 }])
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    // DOM 先关掉面板：这一趟的增长已经过期，失败也不该决定现在的界面。
    growth.setSidebars(contactCard)
    await drain(win, growth)
    expect(refused).toEqual([])
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1385, 1690])
    expect(win.state.width).toBe(1385)
    expect(growth.getAppliedDelta()).toBe(325)
    // The stale attempt consumed one armed failure; the pending one must not eat
    // the dispose shrink as well.
    win.failNextSetSize(0)
    growth.dispose()
    await drain(win, growth)
    expect(win.state.width).toBe(1060)
  })

  // 审查 P2（复现自现有延迟假窗口）：卡片 A 的增长**已派发**、还压在延迟队列里时新声明插队；
  // 旧一代的写随后成功落地，窗口确实带着 A 的那段宽度（resolveResize 按实测记账），但
  // finally 里的 `current(operation)` 拦下了基线更新——accepted 从未记录 A。之后新一代卡片
  // 两次失败时，refusalsFor 把**两张**卡都按 accepted:null 回报，Workbench 连带关掉已被成功
  // 容纳的那张。修复：被取代的**成功**趟把它那张在最新声明里原样保留的卡片并入已接受基线，
  // 同时陈旧回调、未发出的原生写、失败的改写照旧一概不进基线。

  test('被取代的迟到成功仍算容纳（先开联系人卡）：后继面板失败只回报面板', async () => {
    const { win, growth, refused } = createHarness({ defer: true })
    growth.setSidebars(contactCard)
    await settle()
    // 1385 的 setSize 已派发给原生层、还没执行；新声明在此时插队。
    expect(win.pendingNames()).toEqual(['setSize'])
    growth.setSidebars([{ id: 'right-panel', width: 300 }, ...contactCard])
    await settle()
    expect(win.state.width).toBe(1060)

    // 旧一代的写现在才成功落地：陈旧回调被代际守卫压住（不重试、不回报），但延迟的
    // 测量必须恢复真实归属——窗口带着联系人卡的 325。
    win.flush()
    await settle()
    expect(win.state.width).toBe(1385)
    expect(growth.getAppliedDelta()).toBe(325)

    // 新一代的面板增长两次尝试都失败。联系人卡没要新宽度：它已被那次成功容纳，
    // 绝不进拒绝名单。
    win.failNextSetSize(2)
    await drain(win, growth)
    expect(refused).toEqual([[{ id: 'right-panel', width: 300, accepted: null }]])
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1385, 1690, 1690])
    expect(win.state.width).toBe(1385)

    // DOM 只关面板：那 325 仍归联系人卡，缩回声明零原生写，dispose 干净回到基线。
    growth.setSidebars(contactCard)
    await drain(win, growth)
    expect(refused).toHaveLength(1)
    expect(growth.getAppliedDelta()).toBe(325)
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1385, 1690, 1690])
    growth.dispose()
    await drain(win, growth)
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH)
    expect(growth.getAppliedDelta()).toBe(0)
  })

  test('被取代的迟到成功仍算容纳（先开右侧面板）：后继联系人卡失败只回报它', async () => {
    const { win, growth, refused } = createHarness({ defer: true })
    const panel = [{ id: 'right-panel', width: 300 }]
    growth.setSidebars(panel)
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    growth.setSidebars([...panel, ...contactCard])
    await settle()
    // 面板的增长迟到但成功：窗口 1365、拥有 305，基线记下 right-panel:300。
    win.flush()
    await settle()
    expect(win.state.width).toBe(1365)
    expect(growth.getAppliedDelta()).toBe(305)
    // 声明顺序上面板在最前，但真正要不到宽度的是后开的联系人卡。
    win.failNextSetSize(2)
    await drain(win, growth)
    expect(refused).toEqual([[{ id: 'contact-card', width: 320, accepted: null }]])
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1365, 1690, 1690])
    expect(win.state.width).toBe(1365)
    growth.setSidebars(panel)
    await drain(win, growth)
    expect(refused).toHaveLength(1)
    expect(growth.getAppliedDelta()).toBe(305)
    growth.dispose()
    await drain(win, growth)
    expect(win.state.width).toBe(1060)
  })

  test('变宽的成功趟被取代：已接受基线跟着变宽，后续失败不再要求恢复旧宽度', async () => {
    const { win, growth, refused } = createHarness({ defer: true })
    await drain(win, growth, contactCard)
    expect(win.state.width).toBe(1385)
    // 联系人卡 320 → 360：1425 的写已派发时，面板声明插队。
    growth.setSidebars([{ id: 'contact-card', width: 360 }])
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    growth.setSidebars([{ id: 'contact-card', width: 360 }, { id: 'right-panel', width: 300 }])
    await settle()
    win.flush()
    await settle()
    expect(win.state.width).toBe(1425)
    expect(growth.getAppliedDelta()).toBe(365)
    // 基线里的联系人卡跟着那次成功变成 360；面板失败时它完全不在名单上
    // （修复前会回报 {width:360, accepted:320} 让 DOM 把已经落地的宽度改回去）。
    win.failNextSetSize(2)
    await drain(win, growth)
    expect(refused).toEqual([[{ id: 'right-panel', width: 300, accepted: null }]])
    expect(win.state.width).toBe(1425)
    growth.dispose()
    await drain(win, growth)
    expect(win.state.width).toBe(1060)
  })

  test('钳制成功的陈旧趟也算容纳：基线记声明宽度，不是被工作区挤掉的部分宽度', async () => {
    // 物理工作区只有 2700：联系人卡的 1385 增长被钳成 1350。钳制不是失败（SPEC §4），
    // 所以这次被取代的成功写仍把 contact-card:320（声明值）并入基线。
    const harness = createHarness({
      defer: true,
      origin: { x: 0, y: 50 },
      monitor: createMonitor({ width: 2700, height: 1200, taskbar: 0 }),
    })
    const { win, growth, refused } = harness
    growth.setSidebars(contactCard)
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    growth.setSidebars([...contactCard, { id: 'right-panel', width: 300 }])
    await settle()
    // 面板那一趟读到的已是宽敞的工作区（窗口被拖去了外接屏）：它有真实增长空间，
    // 失败才走真实的拒绝回报路径。先让陈旧的那次钳制写落地（不能让它吃到新装的失败计数）。
    harness.deps.getMonitor = async () => createMonitor()
    win.flush()
    await settle()
    expect(win.state.width).toBe(1350)
    expect(growth.getAppliedDelta()).toBe(290)
    win.failNextSetSize(2)
    await drain(win, growth)
    expect(win.state.width).toBe(1350)
    expect(growth.getAppliedDelta()).toBe(290)
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1350, 1690, 1690])
    // 窗口只给了联系人卡 290 而它声明 320——仍算已容纳：拒绝名单只有面板。
    expect(refused).toEqual([[{ id: 'right-panel', width: 300, accepted: null }]])
    growth.dispose()
    await drain(win, growth)
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH)
  })

  test('迟到的失败趟即使部分改写了窗口，也不进入已接受基线', async () => {
    // setSizeAppliesAnyway：原生调用抛错但窗口其实变宽了。「没有完整的成功就没有
    // 容纳」：拥有量一路按实测记（325 → 630），基线却一步不让——两张新宽卡都被点名，
    // DOM 连联系人卡一起关，关闭趟按真实测量的 630 缩回 1060。
    const { win, growth, refused } = createHarness({ defer: true, setSizeAppliesAnyway: true })
    growth.setSidebars(contactCard)
    await settle()
    growth.setSidebars([...contactCard, { id: 'right-panel', width: 300 }])
    await settle()
    win.failNextSetSize(1)
    win.flush()
    await settle()
    expect(win.state.width).toBe(1385)
    expect(growth.getAppliedDelta()).toBe(325)
    win.failNextSetSize(2)
    await drain(win, growth)
    expect(refused).toEqual([[
      { id: 'contact-card', width: 320, accepted: null },
      { id: 'right-panel', width: 300, accepted: null },
    ]])
    // 新一代的失败写同样部分生效了：实测记账跟上（630），但基线依然为空——修复只
    // 救济「完整成功」的那次写，两张要新宽的卡都被点名。
    expect(win.state.width).toBe(1690)
    expect(growth.getAppliedDelta()).toBe(630)
    growth.setSidebars([])
    await drain(win, growth)
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH)
    expect(refused).toHaveLength(1)
    growth.dispose()
    await settle()
  })

  test('从未派发的陈旧增长：不写原生、不进基线，新卡片失败时两张一起回报', async () => {
    // 对偶情形：旧一代还卡在写前的几何读取上（getMonitor 门闸），一个原生写都没发。
    // 代际守卫必须继续拦住它——修复只救济「已派发且成功」的写。
    const harness = createHarness()
    const { win, growth, refused } = harness
    const gate = barrier()
    const entered = barrier()
    harness.deps.getMonitor = async () => { entered.release(); await gate.promise; return createMonitor() }
    growth.setSidebars(contactCard)
    await entered.promise
    growth.setSidebars([...contactCard, { id: 'right-panel', width: 300 }])
    gate.release()
    win.failNextSetSize(2)
    await settle()
    // 只有新一代的两次失败尝试发过 setSize；陈旧趟连一次都没有。
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1690, 1690])
    expect(requestedWidths(win)).toEqual([1690, 1690])
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
    // 没有任何写真正落地：基线仍是空的，两张要新宽的卡一起被点名。
    expect(refused).toEqual([[
      { id: 'contact-card', width: 320, accepted: null },
      { id: 'right-panel', width: 300, accepted: null },
    ]])
    growth.dispose()
    await settle()
  })

  for (const superseded of [false, true]) {
    for (const panelFirst of [false, true]) {
      test(`delayed successful measurement preserves contact credit: superseded=${superseded}, panelFirst=${panelFirst}`, async () => {
        const { win, growth, refused } = createHarness({ defer: true })
        const both = panelFirst ? [...panelCard, ...contactCard] : [...contactCard, ...panelCard]
        growth.setSidebars(contactCard)
        await settle()
        expect(win.pendingNames()).toEqual(['setSize'])
        if (superseded) growth.setSidebars(both)
        win.failNextInnerSize(1)
        win.flush()
        await settle()
        expect(win.state.width).toBe(1385)
        expect(growth.getAppliedDelta()).toBe(superseded ? 325 : 0)
        expect(refused).toEqual([])
        win.failNextSetSize(2)
        if (!superseded) growth.setSidebars(both)
        await drain(win, growth)
        expect(requestedWidths(win)).toEqual([1385, 1690, 1690])
        expect(win.state.width).toBe(1385)
        expect(growth.getAppliedDelta()).toBe(325)
        expect(win.state.min.width).toBe(925)
        expect(refused).toEqual([[{ id: 'right-panel', width: 300, accepted: null }]])
        await drain(win, growth, contactCard)
        expect(requestedWidths(win)).toEqual([1385, 1690, 1690])
        await disposeToBaseline(win, growth)
      })
    }

    test(`delayed successful widening establishes the 360 rollback baseline: superseded=${superseded}`, async () => {
      const { win, growth, refused } = createHarness({ defer: true })
      const wider = [{ id: 'contact-card', width: 360 }]
      await drain(win, growth, contactCard)
      growth.setSidebars(wider)
      await settle()
      expect(win.pendingNames()).toEqual(['setSize'])
      if (superseded) growth.setSidebars([...panelCard, ...wider])
      win.failNextInnerSize(1)
      win.flush()
      await settle()
      expect(win.state.width).toBe(1425)
      expect(growth.getAppliedDelta()).toBe(superseded ? 365 : 325)
      expect(refused).toEqual([])
      win.failNextSetSize(2)
      if (!superseded) growth.setSidebars([...panelCard, ...wider])
      await drain(win, growth)
      expect(requestedWidths(win)).toEqual([1385, 1425, 1730, 1730])
      expect(win.state.width).toBe(1425)
      expect(growth.getAppliedDelta()).toBe(365)
      expect(refused).toEqual([[{ id: 'right-panel', width: 300, accepted: null }]])
      win.failNextSetSize(2)
      await drain(win, growth, [{ id: 'contact-card', width: 400 }])
      expect(refused).toEqual([
        [{ id: 'right-panel', width: 300, accepted: null }],
        [{ id: 'contact-card', width: 400, accepted: 360 }],
      ])
      expect(requestedWidths(win)).toEqual([1385, 1425, 1730, 1730, 1465, 1465])
      await disposeToBaseline(win, growth)
    })
  }

  for (const delayed of [false, true]) {
    test(`late sibling success retains the existing panel's 300 rollback width: delayed=${delayed}`, async () => {
      const { win, growth, refused } = createHarness({ defer: true })
      await drain(win, growth, panelCard)
      growth.setSidebars([...panelCard, ...contactCard])
      await settle()
      expect(win.pendingNames()).toEqual(['setSize'])
      growth.setSidebars([{ id: 'right-panel', width: 340 }, ...contactCard])
      if (delayed) win.failNextInnerSize(1)
      win.flush()
      await settle()
      expect(win.state.width).toBe(1690)
      expect(growth.getAppliedDelta()).toBe(630)
      expect(refused).toEqual([])
      win.failNextSetSize(2)
      await drain(win, growth)
      expect(refused).toEqual([[{ id: 'right-panel', width: 340, accepted: 300 }]])
      expect(requestedWidths(win)).toEqual([1365, 1690, 1730, 1730])
      expect(win.state.width).toBe(1690)
      expect(growth.getAppliedDelta()).toBe(630)
      await drain(win, growth, [...panelCard, ...contactCard])
      expect(requestedWidths(win)).toEqual([1365, 1690, 1730, 1730])
      expect(refused).toHaveLength(1)
      await disposeToBaseline(win, growth)
    })

    test(`late success covers a narrower replacement, whose later success replaces rather than maximizes history: delayed=${delayed}`, async () => {
      const { win, growth, refused } = createHarness({ defer: true })
      const narrower = [{ id: 'contact-card', width: 280 }]
      growth.setSidebars(contactCard)
      await settle()
      expect(win.pendingNames()).toEqual(['setSize'])
      growth.setSidebars([...panelCard, ...narrower])
      if (delayed) win.failNextInnerSize(1)
      win.flush()
      await settle()
      expect(win.state.width).toBe(1385)
      expect(growth.getAppliedDelta()).toBe(325)
      win.failNextSetSize(2)
      await drain(win, growth)
      expect(refused).toEqual([[{ id: 'right-panel', width: 300, accepted: null }]])
      expect(requestedWidths(win)).toEqual([1385, 1650, 1650])
      await drain(win, growth, narrower)
      expect(win.state.width).toBe(1345)
      expect(growth.getAppliedDelta()).toBe(285)
      expect(win.state.min.width).toBe(885)
      win.failNextSetSize(2)
      await drain(win, growth, [{ id: 'contact-card', width: 300 }])
      expect(refused).toEqual([
        [{ id: 'right-panel', width: 300, accepted: null }],
        [{ id: 'contact-card', width: 300, accepted: 280 }],
      ])
      expect(requestedWidths(win)).toEqual([1385, 1650, 1650, 1345, 1365, 1365])
      await disposeToBaseline(win, growth)
    })

    test(`a late 320 success does not pre-accept a desired 360 replacement: delayed=${delayed}`, async () => {
      const { win, growth, refused } = createHarness({ defer: true })
      growth.setSidebars(contactCard)
      await settle()
      expect(win.pendingNames()).toEqual(['setSize'])
      growth.setSidebars([{ id: 'contact-card', width: 360 }, ...panelCard])
      if (delayed) win.failNextInnerSize(1)
      win.flush()
      await settle()
      expect(refused).toEqual([])
      win.failNextSetSize(2)
      await drain(win, growth)
      expect(refused).toEqual([[
        { id: 'contact-card', width: 360, accepted: 320 },
        { id: 'right-panel', width: 300, accepted: null },
      ]])
      expect(requestedWidths(win)).toEqual([1385, 1730, 1730])
      expect(win.state.width).toBe(1385)
      expect(growth.getAppliedDelta()).toBe(325)
      await disposeToBaseline(win, growth)
    })

    test(`a rejected stale write owns its full mutation without accepting either card: delayed=${delayed}`, async () => {
      const { win, growth, refused } = createHarness({ defer: true, setSizeAppliesAnyway: true })
      growth.setSidebars(contactCard)
      await settle()
      expect(win.pendingNames()).toEqual(['setSize'])
      growth.setSidebars([...panelCard, ...contactCard])
      win.failNextSetSize(1)
      if (delayed) win.failNextInnerSize(1)
      win.flush()
      await settle()
      expect(win.state.width).toBe(1385)
      expect(growth.getAppliedDelta()).toBe(325)
      expect(refused).toEqual([])
      win.failNextSetSize(1)
      await drain(win, growth)
      expect(refused).toEqual([[
        { id: 'right-panel', width: 300, accepted: null },
        { id: 'contact-card', width: 320, accepted: null },
      ]])
      expect(requestedWidths(win)).toEqual([1385, 1690])
      expect(win.state.width).toBe(1690)
      expect(growth.getAppliedDelta()).toBe(630)
      await disposeToBaseline(win, growth)
    })
  }

  test('delayed second rejected partial widening recovers ownership, not the rejected width baseline', async () => {
    const { win, growth, refused } = createHarness({ defer: true, setSizeApplyFraction: 0.5 })
    await drain(win, growth, contactCard)
    win.failNextSetSize(2)
    growth.setSidebars([{ id: 'contact-card', width: 360 }])
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    win.flush()
    await settle()
    expect(win.state.width).toBe(1405)
    expect(growth.getAppliedDelta()).toBe(345)
    expect(win.pendingNames()).toEqual(['setSize'])
    win.failNextInnerSize(1)
    win.flush()
    await settle()
    expect(win.state.width).toBe(1415)
    expect(growth.getAppliedDelta()).toBe(345)
    expect(refused).toEqual([[{ id: 'contact-card', width: 360, accepted: 320 }]])
    win.fireResize()
    await settle()
    expect(growth.getAppliedDelta()).toBe(355)
    expect(win.pendingNames()).toEqual(['setMinSize'])
    win.failNextSetSize(2)
    growth.setSidebars([{ id: 'contact-card', width: 400 }])
    await drain(win, growth)
    expect(refused).toEqual([
      [{ id: 'contact-card', width: 360, accepted: 320 }],
      [{ id: 'contact-card', width: 400, accepted: 320 }],
    ])
    expect(requestedWidths(win)).toEqual([1385, 1425, 1425, 1465, 1465])
    expect(win.state.width).toBe(1452.5)
    expect(growth.getAppliedDelta()).toBe(392.5)
    win.state.height = 900
    await disposeToBaseline(win, growth)
  })

  test('recovered rejected narrowing retains the last successful wider baseline without a growth callback', async () => {
    const { win, growth, refused } = createHarness({ defer: true, setSizeAppliesAnyway: true })
    await drain(win, growth, contactCard)
    growth.setSidebars([{ id: 'contact-card', width: 280 }])
    await settle()
    expect(win.pendingNames()).toEqual(['setMinSize'])
    win.flush()
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    win.failNextSetSize(1)
    win.failNextInnerSize(1)
    win.flush()
    await settle()
    expect(win.state.width).toBe(1345)
    expect(growth.getAppliedDelta()).toBe(325)
    win.fireResize()
    await drain(win, growth)
    expect(growth.getAppliedDelta()).toBe(285)
    expect(win.state.min.width).toBe(885)
    expect(requestedWidths(win)).toEqual([1385, 1345])
    expect(refused).toEqual([])
    win.failNextSetSize(1)
    await drain(win, growth, [{ id: 'contact-card', width: 340 }])
    expect(refused).toEqual([[{ id: 'contact-card', width: 340, accepted: 320 }]])
    expect(requestedWidths(win)).toEqual([1385, 1345, 1405])
    expect(win.state.width).toBe(1405)
    expect(growth.getAppliedDelta()).toBe(345)
    await disposeToBaseline(win, growth)
  })

  for (const fraction of [1, 0.5]) {
    test(`recovered rejection cannot become acceptance through ${fraction === 1 ? 'equality' : 'zero-room clamping'}`, async () => {
      const { win, growth, refused, deps } = createHarness({ defer: true, origin: { x: 0, y: 50 }, setSizeApplyFraction: fraction })
      growth.setSidebars(contactCard)
      await settle()
      expect(win.pendingNames()).toEqual(['setSize'])
      win.failNextSetSize(1)
      win.failNextInnerSize(1)
      win.flush()
      await settle()
      expect(win.state.width).toBe(fraction === 1 ? 1385 : 1222.5)
      expect(growth.getAppliedDelta()).toBe(0)
      expect(refused).toEqual([])
      if (fraction !== 1) deps.getMonitor = async () => createMonitor({ width: 2445 })
      win.fireResize()
      await drain(win, growth)
      expect(requestedWidths(win)).toEqual([1385])
      expect(growth.getAppliedDelta()).toBe(fraction === 1 ? 325 : 162.5)
      expect(refused).toEqual([[{ id: 'contact-card', width: 320, accepted: null }]])
      deps.getMonitor = async () => createMonitor()
      win.failNextSetSize(fraction === 1 ? 1 : 2)
      await drain(win, growth, [...panelCard, ...contactCard])
      expect(refused).toEqual([
        [{ id: 'contact-card', width: 320, accepted: null }],
        [{ id: 'right-panel', width: 300, accepted: null }, { id: 'contact-card', width: 320, accepted: null }],
      ])
      expect(requestedWidths(win)).toEqual(fraction === 1 ? [1385, 1690] : [1385, 1690, 1690])
      expect(win.state.width).toBe(fraction === 1 ? 1690 : 1573.5)
      expect(growth.getAppliedDelta()).toBe(fraction === 1 ? 630 : 513.5)
      await disposeToBaseline(win, growth)
    })
  }

  for (const flag of ['isMaximized', 'isFullscreen'] as const) {
    test(`successful pending declaration survives unknown ${flag} ownership without writing or refusing`, async () => {
      const { win, growth, refused } = createHarness({ defer: true })
      growth.setSidebars(contactCard)
      await settle()
      expect(win.pendingNames()).toEqual(['setSize'])
      const readFlag = win[flag]
      win[flag] = async () => { throw new Error('ownership unavailable') }
      win.flush()
      await settle()
      expect(win.state.width).toBe(1385)
      expect(growth.getAppliedDelta()).toBe(0)
      growth.setSidebars([...panelCard, ...contactCard])
      win.fireResize()
      await drain(win, growth)
      expect(requestedWidths(win)).toEqual([1385])
      expect(win.requests).toHaveLength(1)
      expect(win.state.min.width).toBe(600)
      expect(refused).toEqual([])
      win[flag] = readFlag
      win.failNextSetSize(2)
      win.fireResize()
      await drain(win, growth)
      expect(win.state.width).toBe(1385)
      expect(growth.getAppliedDelta()).toBe(325)
      expect(refused).toEqual([[{ id: 'right-panel', width: 300, accepted: null }]])
      expect(requestedWidths(win)).toEqual([1385, 1690, 1690])
      await disposeToBaseline(win, growth)
    })
  }

  for (const delayed of [false, true]) {
    for (const action of ['replace', 'remove/readd', 'dispose/revive'] as const) {
      test(`pending success cannot transfer credit across ${action}: delayed=${delayed}`, async () => {
        const { win, growth, refused } = createHarness({ defer: true })
        growth.setSidebars(contactCard)
        await settle()
        expect(win.pendingNames()).toEqual(['setSize'])
        if (action === 'remove/readd') growth.setSidebars([])
        if (action === 'dispose/revive') growth.dispose()
        growth.setSidebars(action === 'replace' ? [{ id: 'right-panel', width: 400 }] : [...panelCard, ...contactCard])
        if (delayed) win.failNextInnerSize(1)
        win.flush()
        await settle()
        expect(win.state.width).toBe(1385)
        expect(growth.getAppliedDelta()).toBe(325)
        expect(refused).toEqual([])
        win.failNextSetSize(2)
        await drain(win, growth)
        expect(refused).toEqual([action === 'replace'
          ? [{ id: 'right-panel', width: 400, accepted: null }]
          : [{ id: 'right-panel', width: 300, accepted: null }, { id: 'contact-card', width: 320, accepted: null }]])
        expect(requestedWidths(win)).toEqual(action === 'replace' ? [1385, 1465, 1465] : [1385, 1690, 1690])
        expect(win.state.width).toBe(1385)
        expect(growth.getAppliedDelta()).toBe(325)
        expect(win.listenerCount()).toBe(1)
        expect(win.pendingCount()).toBe(0)
        await disposeToBaseline(win, growth)
      })
    }
  }

  for (const dispose of [false, true]) {
    test(`ending an accepted card lifetime clears both old history and pending success: dispose=${dispose}`, async () => {
      const { win, growth, refused } = createHarness({ defer: true })
      await drain(win, growth, [{ id: 'contact-card', width: 280 }])
      growth.setSidebars(contactCard)
      await settle()
      expect(win.pendingNames()).toEqual(['setSize'])
      if (dispose) growth.dispose()
      else growth.setSidebars([])
      growth.setSidebars([...panelCard, ...contactCard])
      win.failNextInnerSize(1)
      win.flush()
      await settle()
      win.failNextSetSize(2)
      await drain(win, growth)
      expect(refused).toEqual([[
        { id: 'right-panel', width: 300, accepted: null },
        { id: 'contact-card', width: 320, accepted: null },
      ]])
      expect(requestedWidths(win)).toEqual([1345, 1385, 1690, 1690])
      expect(win.state.width).toBe(1385)
      expect(growth.getAppliedDelta()).toBe(325)
      await disposeToBaseline(win, growth)
    })

    test(`unread pending success remains removable without another card: dispose=${dispose}`, async () => {
      const { win, growth, refused } = createHarness({ defer: true })
      growth.setSidebars(contactCard)
      await settle()
      expect(win.pendingNames()).toEqual(['setSize'])
      if (dispose) growth.dispose()
      else growth.setSidebars([])
      win.failNextInnerSize(1)
      await drain(win, growth)
      expect(requestedWidths(win)).toEqual([1385, 1060])
      expect(refused).toEqual([])
      expect(win.state.width).toBe(1060)
      expect(growth.getAppliedDelta()).toBe(0)
      expect(win.state.min.width).toBe(600)
      expect(win.pendingCount()).toBe(0)
      await disposeToBaseline(win, growth)
    })
  }

  for (const room of [0, 290]) {
    test(`${room}px work-area accommodation retains the declared contact width after failed panel growth`, async () => {
      const { win, growth, refused, deps } = createHarness({
        defer: true, origin: { x: 0, y: 50 }, monitor: createMonitor({ width: (1060 + room) * 2 }),
      })
      growth.setSidebars(contactCard)
      await settle()
      if (room > 0) {
        expect(win.pendingNames()).toEqual(['setSize'])
        win.failNextInnerSize(1)
        win.flush()
        await settle()
      } else expect(win.requests).toEqual([])
      expect(win.state.width).toBe(1060 + room)
      expect(refused).toEqual([])
      deps.getMonitor = async () => createMonitor()
      win.failNextSetSize(2)
      await drain(win, growth, [...panelCard, ...contactCard])
      expect(refused).toEqual([[{ id: 'right-panel', width: 300, accepted: null }]])
      expect(win.state.width).toBe(1060 + room)
      expect(growth.getAppliedDelta()).toBe(room)
      expect(requestedWidths(win)).toEqual(room > 0 ? [1350, 1690, 1690] : [1690, 1690])
      await disposeToBaseline(win, growth)
    })
  }

  test('增长请求在途时关闭：迟到的成功按实测记账，再由排队的关闭对账撤销', async () => {
    const { win, growth } = createHarness({ defer: true })
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    // 唯一发出的原生调用还排在延迟队列里：窗口尺寸没变。
    expect(win.pendingNames()).toEqual(['setSize'])
    expect(win.state.width).toBe(1060)
    expect(sizeCalls(win)).toEqual([])

    // 关闭发生在 flush 之前。
    growth.setSidebars([])
    await settle()
    expect(win.pendingCount()).toBe(1)

    // 在途的增长现在才真正落地。迟到的成功不能假装没发生：窗口确实带着 325，
    // 记账按实测，随后排队的关闭对账负责把它收回去（而不是留下 1385 却声称拥有 0）。
    win.flush()
    await settle()
    expect(win.state.width).toBe(1385)
    expect(growth.getAppliedDelta()).toBe(325)

    await drain(win, growth)
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH)
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1385, 1060])
    growth.dispose()
  })

  test('setPosition 失败时不再假定已经左移，按原可用空间缩减增量', async () => {
    const { win, logs, growth } = createHarness({
      setPositionFailures: 1,
      origin: { x: 100, y: 50 },
      monitor: createMonitor({ width: 2400, height: 1200, taskbar: 0 }),
    })
    growth.setSidebars([{ id: 'contact-card', width: 320 }])
    await settle()
    expect(win.state.x).toBe(100)
    expect(sizeCalls(win).at(-1)).toMatchObject({ width: 1100 })
    expect(growth.getAppliedDelta()).toBe(40)
    expect(logs.some((line) => line.includes('setPosition failed'))).toBe(true)
    growth.dispose()
  })

  test('dispose 缩回自己拥有的宽度并解除监听，而不是把增量丢掉', async () => {
    const { win, growth } = createHarness()
    growth.setSidebars([{ id: 'right-panel', width: 320 }])
    await settle()
    expect(win.unlistenCount()).toBe(0)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(BASE_MIN_WIDTH)
    expect(win.unlistenCount()).toBe(1)
    const sizes = sizeCalls(win).length
    growth.setSidebars([])
    await settle()
    expect(sizeCalls(win).length).toBe(sizes)
  })

  test('快速反复开关串行到最后一个状态，宽度不低于基础宽度', async () => {
    const { win, growth } = createHarness()
    growth.setSidebars([{ id: 'right-panel', width: 320 }])
    growth.setSidebars([])
    growth.setSidebars([{ id: 'right-panel', width: 320 }])
    growth.setSidebars([])
    growth.setSidebars([{ id: 'right-panel', width: 320 }])
    await settle()
    expect(growth.getAppliedDelta()).toBe(325)
    expect(win.state.width).toBe(1385)
    for (const call of sizeCalls(win)) expect(call.width!).toBeGreaterThanOrEqual(1060)
    growth.dispose()
  })

  test('failed opening minimum remains repairable without another resize', async () => {
    const { win, growth } = createHarness({ autoResize: true, autoResizeOnFailure: true })
    win.failNextSetMinSize(2)
    growth.setSidebars(contactCard)
    await settle()
    expect(win.state.width).toBe(1385)
    expect(growth.getAppliedDelta()).toBe(325)
    expect(win.state.min.width).toBe(600)
    expect(minCalls(win)).toHaveLength(2)
    win.fireResize()
    await settle()
    expect(win.state.min).toEqual({ width: 925, height: 560 })
    expect(sizeCalls(win)).toHaveLength(1)
    const writes = win.requests.length
    win.fireResize()
    await settle()
    expect(win.requests).toHaveLength(writes)
    growth.dispose()
    await settle()
  })

  for (const recovery of ['resize', 'dispose'] as const) {
    test(`failed closing minimum survives zero ownership and repairs on ${recovery}`, async () => {
      const { win, growth } = createHarness({ autoResize: true, autoResizeOnFailure: true })
      growth.setSidebars(contactCard)
      await settle()
      win.failNextSetMinSize(2)
      growth.setSidebars([])
      await settle()
      expect(win.state.width).toBe(1060)
      expect(growth.getAppliedDelta()).toBe(0)
      expect(win.state.min.width).toBe(925)
      const sizes = sizeCalls(win).length
      if (recovery === 'resize') win.fireResize()
      else growth.dispose()
      await settle()
      expect(win.state.min.width).toBe(600)
      expect(win.state.width).toBe(1060)
      expect(sizeCalls(win)).toHaveLength(sizes)
      growth.dispose()
      await settle()
    })
  }

  test('persistent minimum failure is bounded, including events, disposal and revival', async () => {
    const { win, growth, logs } = createHarness({ width: 600, autoResize: true, autoResizeOnFailure: true })
    growth.setSidebars(contactCard)
    await settle()
    win.failNextSetMinSize(20)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(925)
    expect(win.state.min.width).toBe(925)
    expect(growth.getAppliedDelta()).toBe(325)
    expect(minCalls(win)).toHaveLength(3)
    expect(sizeCalls(win)).toHaveLength(2)
    growth.dispose()
    await settle()
    expect(minCalls(win)).toHaveLength(5)
    expect(sizeCalls(win)).toHaveLength(2)
    expect(logs.some((line) => line.includes('still failing'))).toBe(true)
    win.failNextSetMinSize(0)
    growth.setSidebars([])
    await settle()
    expect(win.state.width).toBe(600)
    expect(win.state.min.width).toBe(600)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(win.listenerCount()).toBe(1)
    growth.dispose()
    await settle()
    expect(win.listenerCount()).toBe(0)
  })

  test('incremental measurement failure retains total ownership until measured, then closes exactly', async () => {
    const { win, growth } = createHarness()
    growth.setSidebars(contactCard)
    await settle()
    const setSize = win.setSize
    let armed = true
    win.setSize = async (size) => {
      await setSize(size)
      if (armed) { armed = false; win.failNextInnerSize(1) }
    }
    growth.setSidebars([{ id: 'contact-card', width: 360 }])
    await settle()
    expect(win.state.width).toBe(1425)
    expect(growth.getAppliedDelta()).toBe(325)
    expect(win.state.min.width).toBe(925)
    win.fireResize()
    await settle()
    expect(growth.getAppliedDelta()).toBe(365)
    expect(win.state.min.width).toBe(965)
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1385, 1425])
    growth.setSidebars([])
    await settle()
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(win.state.min.width).toBe(600)
    growth.dispose()
  })

  test('closing resolves an outstanding incremental measurement before subtracting ownership', async () => {
    const { win, growth } = createHarness()
    growth.setSidebars(contactCard)
    await settle()
    const setSize = win.setSize
    win.setSize = async (size) => { await setSize(size); if (size.width === 1425) win.failNextInnerSize(1) }
    growth.setSidebars([{ id: 'contact-card', width: 360 }])
    await settle()
    expect(growth.getAppliedDelta()).toBe(325)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(win.state.min.width).toBe(600)
  })

  for (const partial of [false, true]) {
    test(`both incremental resize attempts reject, partial application=${partial}, unread measurement stays recoverable`, async () => {
      const { win, growth, refused } = createHarness({ setSizeApplyFraction: partial ? 0.5 : undefined })
      growth.setSidebars(contactCard)
      await settle()
      win.failNextSetSize(2)
      const setSize = win.setSize
      let failures = 0
      win.setSize = async (size) => {
        try { await setSize(size) } catch (cause) {
          failures += 1
          if (failures === 2) win.failNextInnerSize(1)
          throw cause
        }
      }
      growth.setSidebars([{ id: 'contact-card', width: 360 }])
      await settle()
      expect(sizeCalls(win)).toHaveLength(3)
      expect(win.state.width).toBe(partial ? 1415 : 1385)
      expect(growth.getAppliedDelta()).toBe(partial ? 345 : 325)
      expect(win.state.min.width).toBe(925)
      expect(refused).toEqual([[{ id: 'contact-card', width: 360, accepted: 320 }]])
      growth.setSidebars([])
      await settle()
      expect(win.state.width).toBe(1060)
      expect(win.state.min.width).toBe(600)
      expect(growth.getAppliedDelta()).toBe(0)
      growth.dispose()
    })
  }

  test('an unread first failed mutation cannot retry or rebase until geometry recovers', async () => {
    const { win, growth } = createHarness({ setSizeApplyFraction: 0.5 })
    growth.setSidebars(contactCard)
    await settle()
    win.failNextSetSize(1)
    const setSize = win.setSize
    win.setSize = async (size) => {
      try { await setSize(size) } catch (cause) { win.failNextInnerSize(10); throw cause }
    }
    growth.setSidebars([{ id: 'contact-card', width: 360 }])
    await settle()
    expect(win.state.width).toBe(1405)
    expect(growth.getAppliedDelta()).toBe(325)
    win.fireResize()
    await settle()
    expect(sizeCalls(win)).toHaveLength(2)
    expect(win.state.min.width).toBe(925)
    win.failNextInnerSize(0)
    win.fireResize()
    await settle()
    expect(win.state.width).toBe(1425)
    expect(growth.getAppliedDelta()).toBe(365)
    expect(win.state.min.width).toBe(965)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
  })

  for (const dpi of [
    { factor: 1.25, panel: 360, request: 1425, physical: 1781, height: 973, owned: 364.8, closed: 1325 },
    { factor: 1.5, panel: 320, request: 1385, physical: 2078, height: 1167, owned: 325 + 1 / 3, closed: 1590 },
  ]) {
    test(`${dpi.factor}x quantized writes and notifications settle without extra native writes`, async () => {
      const { win, growth } = createHarness({ scaleFactor: dpi.factor, autoResize: true })
      growth.setSidebars([{ id: 'contact-card', width: dpi.panel }])
      await settle()
      expect(sizeCalls(win)).toHaveLength(1)
      expect(sizeCalls(win)[0]?.width).toBe(dpi.request)
      expect(await win.innerSize()).toEqual({ width: dpi.physical, height: dpi.height })
      expect(win.state.width).toBeCloseTo(dpi.physical / dpi.factor, 8)
      expect(growth.getAppliedDelta()).toBeCloseTo(dpi.owned, 8)
      expect(win.state.min.width).toBeCloseTo(600 + dpi.owned, 8)
      const writes = win.requests.length
      for (let n = 0; n < 3; n++) { win.fireResize(); await settle() }
      expect(win.requests).toHaveLength(writes)
      win.state.maximized = true
      win.fireResize()
      await settle()
      expect(growth.getAppliedDelta()).toBe(0)
      win.state.maximized = false
      win.fireResize()
      await settle()
      expect(growth.getAppliedDelta()).toBeCloseTo(dpi.owned, 8)
      expect(sizeCalls(win)).toHaveLength(1)
      growth.setSidebars([])
      await settle()
      expect(await win.innerSize()).toEqual({ width: dpi.closed, height: dpi.height })
      expect(growth.getAppliedDelta()).toBe(0)
      expect(win.state.min.width).toBe(600)
      const closedWrites = win.requests.length
      win.fireResize()
      await settle()
      growth.dispose()
      await settle()
      expect(win.requests).toHaveLength(closedWrites)
    })
  }

  for (const rejects of [false, true]) {
    test(`landed clamped movement, rejection=${rejects}, owns only 90 logical pixels`, async () => {
      const { win, growth, refused } = createHarness({
        positionClampX: 50, setPositionFailures: rejects ? 1 : 0,
        setPositionAppliesAnyway: rejects, autoResize: true,
        monitor: createMonitor({ width: 2400 }),
      })
      growth.setSidebars(contactCard)
      await settle()
      expect(positionCalls(win)[0]).toMatchObject({ x: 0 })
      expect(win.state.x).toBe(50)
      expect(win.state.width).toBe(1150)
      expect((await win.outerPosition()).x + (await win.outerSize()).width).toBe(2400)
      expect(growth.getAppliedDelta()).toBe(90)
      expect(win.state.min.width).toBe(690)
      const writes = win.requests.length
      for (let n = 0; n < 3; n++) { win.fireResize(); await settle() }
      expect(win.requests).toHaveLength(writes)
      expect(refused).toEqual([])
      growth.dispose()
      await settle()
      expect(win.state.width).toBe(1060)
      expect(win.state.min.width).toBe(600)
    })
  }

  test('failed post-move geometry defers growth until all bounds are measurable', async () => {
    const { win, growth, refused } = createHarness({ positionClampX: 50, monitor: createMonitor({ width: 2400 }) })
    const setPosition = win.setPosition
    const outerPosition = win.outerPosition
    let unreadable = false
    win.setPosition = async (position) => { await setPosition(position); unreadable = true }
    win.outerPosition = async () => { if (unreadable) throw new Error('position unavailable'); return outerPosition() }
    growth.setSidebars(contactCard)
    await settle()
    expect(win.state.x).toBe(50)
    expect(win.state.width).toBe(1060)
    expect(sizeCalls(win)).toEqual([])
    expect(growth.getAppliedDelta()).toBe(0)
    expect(refused).toEqual([])
    win.fireResize()
    await settle()
    expect(sizeCalls(win)).toEqual([])
    unreadable = false
    win.setPosition = setPosition
    win.fireResize()
    await settle()
    expect(win.state.width).toBe(1150)
    expect(growth.getAppliedDelta()).toBe(90)
    expect(win.state.min.width).toBe(690)
    growth.dispose()
    await settle()
  })

  test('post-move monitor and scale replace pre-move bounds', async () => {
    const harness = createHarness({ monitor: createMonitor({ width: 2400 }) })
    const { win, growth, deps } = harness
    const setPosition = win.setPosition
    let moved = false
    win.setPosition = async (position) => { await setPosition(position); moved = true }
    deps.getMonitor = async () => createMonitor({ width: moved ? 3000 : 2400 })
    win.scaleFactor = async () => moved ? 1.5 : 2
    const innerSize = win.innerSize
    const outerSize = win.outerSize
    win.innerSize = async () => moved
      ? { width: Math.round(win.state.width * 1.5), height: Math.round(win.state.height * 1.5) }
      : innerSize()
    win.outerSize = async () => moved
      ? { width: Math.round(win.state.width * 1.5), height: Math.round(win.state.height * 1.5) + 30 }
      : outerSize()
    win.setSize = async (size) => {
      win.calls.push({ name: 'setSize', ...size })
      win.state.width = Math.round(size.width * 1.5) / 1.5
      win.state.height = Math.round(size.height * 1.5) / 1.5
    }
    growth.setSidebars(contactCard)
    await settle()
    expect(win.state.x).toBe(0)
    expect((await win.innerSize()).width).toBe(2078)
    expect(growth.getAppliedDelta()).toBeCloseTo(325 + 1 / 3, 8)
    expect(win.state.min.width).toBeCloseTo(925 + 1 / 3, 8)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
  })

  test('physical work-area clamp floors the representable destination', async () => {
    const { win, growth } = createHarness({ scaleFactor: 1.5, origin: { x: 0, y: 50 }, monitor: createMonitor({ width: 1801 }), autoResize: true })
    growth.setSidebars(contactCard)
    await settle()
    expect((await win.innerSize()).width).toBe(1801)
    expect(win.state.width).toBeCloseTo(1801 / 1.5, 8)
    expect(growth.getAppliedDelta()).toBeCloseTo(1801 / 1.5 - 1060, 8)
    expect(win.state.min.width).toBeCloseTo(600 + growth.getAppliedDelta(), 8)
    const writes = win.requests.length
    win.fireResize()
    await settle()
    expect(win.requests).toHaveLength(writes)
    growth.dispose()
    await settle()
    expect((await win.innerSize()).width).toBe(1590)
  })

  test('ordinary user resizing rebases only the card-free width', async () => {
    const { win, growth } = createHarness()
    growth.setSidebars(contactCard)
    await settle()
    win.state.width = 1500
    win.state.height = 900
    win.fireResize()
    await settle()
    expect(growth.getAppliedDelta()).toBe(325)
    expect(win.state.width).toBe(1500)
    expect(sizeCalls(win)).toHaveLength(1)
    growth.setSidebars([])
    await settle()
    expect(win.state.width).toBe(1175)
    expect(win.state.height).toBe(900)
    expect(win.state.min.width).toBe(600)
    growth.dispose()
  })

  test('user resizing during awaited minimum reset is preserved on close', async () => {
    const { win, growth } = createHarness()
    growth.setSidebars(contactCard)
    await settle()
    const gate = barrier()
    const entered = barrier()
    const setMinSize = win.setMinSize
    win.setMinSize = async (size) => { entered.release(); await gate.promise; await setMinSize(size) }
    growth.setSidebars([])
    await entered.promise
    win.state.width = 1500
    win.state.height = 900
    gate.release()
    await settle()
    expect(win.state.width).toBe(1175)
    expect(win.state.height).toBe(900)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(win.state.min.width).toBe(600)
    growth.dispose()
  })

  test('user resizing during awaited monitor read supplies the fresh pre-write dimensions', async () => {
    const { win, growth, deps } = createHarness()
    const gate = barrier()
    const entered = barrier()
    deps.getMonitor = async () => { entered.release(); await gate.promise; return createMonitor() }
    growth.setSidebars(contactCard)
    await entered.promise
    win.state.width = 1200
    win.state.height = 900
    gate.release()
    await settle()
    expect(win.state.width).toBe(1525)
    expect(win.state.height).toBe(900)
    expect(growth.getAppliedDelta()).toBe(325)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1200)
    expect(win.state.height).toBe(900)
  })

  test('resize retry refreshes user dimensions instead of resending a stale size', async () => {
    const { win, growth } = createHarness({ setSizeFailures: 1 })
    const innerSize = win.innerSize
    let postFailureReads = 0
    win.innerSize = async () => {
      if (sizeCalls(win).length === 1 && ++postFailureReads === 2) {
        win.state.width = 1200
        win.state.height = 900
      }
      return innerSize()
    }
    growth.setSidebars(contactCard)
    await settle()
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1385, 1525])
    expect(win.state.width).toBe(1525)
    expect(win.state.height).toBe(900)
    expect(growth.getAppliedDelta()).toBe(325)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1200)
    expect(win.state.height).toBe(900)
  })

  for (const mode of ['maximized', 'fullscreen'] as const) {
    test(`${mode} during geometry prevents every native mutation`, async () => {
      const { win, growth, deps, refused } = createHarness({ monitor: createMonitor({ width: 2400 }) })
      const forbidden = watchSystemWrites(win)
      deps.getMonitor = async () => { win.state[mode] = true; return createMonitor({ width: 2400 }) }
      growth.setSidebars(contactCard)
      await settle()
      expect(win.requests).toEqual([])
      expect(forbidden).toEqual([])
      expect(win.state.width).toBe(1060)
      expect(win.state.min.width).toBe(600)
      expect(growth.getAppliedDelta()).toBe(0)
      expect(refused).toEqual([])
      growth.dispose()
      await settle()
    })

    for (const boundary of ['first failure', 'retry read', 'successful measurement'] as const) {
      test(`${mode} at ${boundary} stops subsequent writes and never owns system geometry`, async () => {
        const { win, growth, refused } = createHarness({ setSizeFailures: boundary === 'successful measurement' ? 0 : 1 })
        const forbidden = watchSystemWrites(win)
        const seen: number[] = []
        growth.subscribe(() => seen.push(growth.getAppliedDelta()))
        const takeover = () => {
          win.state[mode] = true
          win.state.width = 1920
          win.state.height = 1080
          win.fireResize()
        }
        const setSize = win.setSize
        const innerSize = win.innerSize
        if (boundary === 'first failure') {
          win.setSize = async (size) => {
            try { await setSize(size) } catch (cause) { takeover(); throw cause }
          }
        } else {
          let postWriteReads = 0
          win.innerSize = async () => {
            if (sizeCalls(win).length === 1 && ++postWriteReads === (boundary === 'retry read' ? 2 : 1)) takeover()
            return innerSize()
          }
        }
        growth.setSidebars(contactCard)
        await settle()
        expect(sizeCalls(win)).toHaveLength(1)
        expect(minCalls(win)).toEqual([])
        expect(forbidden).toEqual([])
        expect(win.state.width).toBe(1920)
        expect(win.state.height).toBe(1080)
        expect(growth.getAppliedDelta()).toBe(0)
        expect(seen).toEqual([])
        expect(refused).toEqual([])
        win.setSize = setSize
        win.innerSize = innerSize
        win.state[mode] = false
        win.state.width = boundary === 'successful measurement' ? 1385 : 1060
        win.state.height = 778
        win.fireResize()
        await settle()
        expect(win.state.width).toBe(1385)
        expect(growth.getAppliedDelta()).toBe(325)
        expect(win.state.min.width).toBe(925)
        expect(forbidden).toEqual([])
        growth.dispose()
        await settle()
        expect(win.state.width).toBe(1060)
      })
    }

    test(`${mode} after a failed opening minimum prevents its enlarged retry`, async () => {
      const { win, growth } = createHarness()
      const forbidden = watchSystemWrites(win)
      const seen: number[] = []
      growth.subscribe(() => seen.push(growth.getAppliedDelta()))
      win.failNextSetMinSize(1)
      const setMinSize = win.setMinSize
      win.setMinSize = async (size) => {
        try { await setMinSize(size) } catch (cause) {
          win.state[mode] = true
          win.state.width = 1920
          throw cause
        }
      }
      growth.setSidebars(contactCard)
      await settle()
      expect(minCalls(win).map((call) => call.width)).toEqual([925, 600])
      expect(sizeCalls(win)).toHaveLength(1)
      expect(win.state.width).toBe(1920)
      expect(win.state.min.width).toBe(600)
      expect(growth.getAppliedDelta()).toBe(0)
      expect(seen).toEqual([325, 0])
      expect(forbidden).toEqual([])
      growth.dispose()
      await settle()
    })

    for (const rejects of [false, true]) {
      test(`${mode} during restored minimum, rejection=${rejects}, only restores minimum afterward`, async () => {
        const { win, growth } = createHarness()
        const forbidden = watchSystemWrites(win)
        growth.setSidebars(contactCard)
        await settle()
        win.state[mode] = true
        win.fireResize()
        await settle()
        expect(growth.getAppliedDelta()).toBe(0)
        expect(win.state.min.width).toBe(600)
        win.state[mode] = false
        const setMinSize = win.setMinSize
        let flipped = false
        if (rejects) win.failNextSetMinSize(1)
        win.setMinSize = async (size) => {
          try { await setMinSize(size) } finally {
            if (!flipped && size?.width === 925) {
              flipped = true
              win.state[mode] = true
              win.state.width = 1920
              win.fireResize()
            }
          }
        }
        const seen: number[] = []
        growth.subscribe(() => seen.push(growth.getAppliedDelta()))
        const before = win.calls.length
        win.fireResize()
        await settle()
        expect(win.calls.slice(before).map((call) => [call.name, call.width])).toEqual([['setMinSize', 925], ['setMinSize', 600]])
        expect(forbidden).toEqual([])
        expect(win.state.width).toBe(1920)
        expect(win.state.min.width).toBe(600)
        expect(growth.getAppliedDelta()).toBe(0)
        expect(seen).toEqual([325, 0])
        growth.dispose()
        await settle()
      })
    }

    test(`${mode} during closing minimum stops the unsent shrink`, async () => {
      const { win, growth } = createHarness()
      const forbidden = watchSystemWrites(win)
      growth.setSidebars(contactCard)
      await settle()
      const setMinSize = win.setMinSize
      win.setMinSize = async (size) => {
        await setMinSize(size)
        win.state[mode] = true
        win.state.width = 1920
        win.state.height = 1080
      }
      growth.setSidebars([])
      await settle()
      expect(sizeCalls(win)).toHaveLength(1)
      expect(win.state.width).toBe(1920)
      expect(win.state.height).toBe(1080)
      expect(win.state.min.width).toBe(600)
      expect(growth.getAppliedDelta()).toBe(0)
      expect(forbidden).toEqual([])
      growth.dispose()
      await settle()
    })

    test(`${mode} invalidates pending normal measurement and clears ownership once`, async () => {
      const { win, growth, refused } = createHarness()
      const forbidden = watchSystemWrites(win)
      growth.setSidebars(contactCard)
      await settle()
      const setSize = win.setSize
      win.setSize = async (size) => { await setSize(size); win.failNextInnerSize(1) }
      growth.setSidebars([{ id: 'contact-card', width: 360 }])
      await settle()
      expect(win.state.width).toBe(1425)
      expect(growth.getAppliedDelta()).toBe(325)
      const seen: number[] = []
      growth.subscribe(() => seen.push(growth.getAppliedDelta()))
      win.state[mode] = true
      win.state.width = 1920
      win.fireResize()
      await settle()
      win.fireResize()
      await settle()
      expect(seen).toEqual([0])
      expect(win.state.min.width).toBe(600)
      expect(win.state.width).toBe(1920)
      expect(growth.getAppliedDelta()).toBe(0)
      expect(requestedWidths(win)).toEqual([1385, 1425])
      expect(forbidden).toEqual([])
      expect(refused).toEqual([])
      win.state[mode] = false
      win.state.width = 1425
      win.fireResize()
      await settle()
      expect(growth.getAppliedDelta()).toBe(365)
      expect(win.state.min.width).toBe(965)
      expect(sizeCalls(win)).toHaveLength(2)
      expect(forbidden).toEqual([])
      expect(refused).toEqual([])
      win.setSize = setSize
      await disposeToBaseline(win, growth)
    })
  }

  for (const flag of ['isMaximized', 'isFullscreen'] as const) {
    test(`${flag} read failure defers all mutations until a later notification`, async () => {
      const { win, growth } = createHarness()
      const readFlag = win[flag]
      win[flag] = async () => { throw new Error('flag unavailable') }
      growth.setSidebars(contactCard)
      await settle()
      expect(win.requests).toEqual([])
      expect(win.state.width).toBe(1060)
      expect(growth.getAppliedDelta()).toBe(0)
      win[flag] = readFlag
      win.fireResize()
      await settle()
      expect(win.state.width).toBe(1385)
      expect(growth.getAppliedDelta()).toBe(325)
      expect(win.state.min.width).toBe(925)
      growth.dispose()
      await settle()
    })

    test(`${flag} read failure at minimum retry preserves debt without retrying blindly`, async () => {
      const { win, growth } = createHarness()
      const readFlag = win[flag]
      const setMinSize = win.setMinSize
      win.failNextSetMinSize(1)
      win.setMinSize = async (size) => {
        try { await setMinSize(size) } catch (cause) {
          win[flag] = async () => { throw new Error('flag unavailable') }
          throw cause
        }
      }
      growth.setSidebars(contactCard)
      await settle()
      expect(minCalls(win)).toHaveLength(1)
      expect(win.state.width).toBe(1385)
      expect(growth.getAppliedDelta()).toBe(325)
      expect(win.state.min.width).toBe(600)
      win[flag] = readFlag
      win.fireResize()
      await settle()
      expect(win.state.min.width).toBe(925)
      expect(sizeCalls(win)).toHaveLength(1)
      growth.dispose()
      await settle()
    })
  }

  for (const action of ['close', 'dispose'] as const) {
    test(`${action} during deferred pre-growth work cancels unsent native mutations`, async () => {
      const { win, growth, deps, refused } = createHarness()
      const gate = barrier()
      const entered = barrier()
      deps.getMonitor = async () => { entered.release(); await gate.promise; return createMonitor() }
      growth.setSidebars(contactCard)
      await entered.promise
      if (action === 'close') growth.setSidebars([])
      else growth.dispose()
      gate.release()
      await settle()
      expect(win.requests).toEqual([])
      expect(win.state.width).toBe(1060)
      expect(win.state.min.width).toBe(600)
      expect(growth.getAppliedDelta()).toBe(0)
      expect(refused).toEqual([])
      growth.dispose()
      await settle()
    })
  }

  test('stale failed dispatched growth never retries or refuses a replacement card', async () => {
    const { win, growth, refused } = createHarness({ defer: true, setSizeFailures: 1, setSizeAppliesAnyway: true })
    growth.setSidebars(contactCard)
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    growth.setSidebars([{ id: 'right-panel', width: 360 }])
    await drain(win, growth)
    expect(refused).toEqual([])
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1385, 1425])
    expect(win.state.width).toBe(1425)
    expect(growth.getAppliedDelta()).toBe(365)
    expect(win.state.min.width).toBe(965)
    growth.dispose()
    await drain(win, growth)
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)
  })

  test('a closing native clamp retains ownership but never re-enlarges the minimum', async () => {
    const { win, growth } = createHarness({ autoResize: true })
    growth.setSidebars(contactCard)
    await settle()
    const setSize = win.setSize
    win.setSize = (size) => setSize({ width: Math.max(1200, size.width), height: size.height })
    growth.setSidebars([])
    await settle()
    expect(win.state.width).toBe(1200)
    expect(growth.getAppliedDelta()).toBe(140)
    expect(win.state.min.width).toBe(600)
    const writes = win.requests.length
    win.fireResize()
    await settle()
    expect(win.requests).toHaveLength(writes)
    win.setSize = setSize
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(win.state.min.width).toBe(600)
  })

  test('minimum debt repairs with no remaining work-area room and no additional geometry writes', async () => {
    const { win, growth, refused } = createHarness({ monitor: createMonitor({ width: 2400 }), autoResize: true })
    win.failNextSetMinSize(2)
    growth.setSidebars(contactCard)
    await settle()
    expect(win.state.width).toBe(1200)
    expect(growth.getAppliedDelta()).toBe(140)
    expect(win.state.min.width).toBe(600)
    win.fireResize()
    await settle()
    expect(win.state.min.width).toBe(740)
    expect(sizeCalls(win)).toHaveLength(1)
    expect(positionCalls(win)).toHaveLength(1)
    expect(refused).toEqual([])
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(win.state.min.width).toBe(600)
  })

  test('late failure notifications cannot spin minimum retries; an explicit reconciliation can recover', async () => {
    const { win, growth, logs } = createHarness()
    const setMinSize = win.setMinSize
    win.failNextSetMinSize(100)
    win.setMinSize = async (size) => {
      try { await setMinSize(size) } catch (cause) {
        setTimeout(() => win.fireResize(), 0)
        throw cause
      }
    }
    growth.setSidebars(contactCard)
    await settle()
    expect(minCalls(win)).toHaveLength(4)
    expect(sizeCalls(win)).toHaveLength(1)
    expect(win.state.width).toBe(1385)
    expect(win.state.min.width).toBe(600)
    expect(growth.getAppliedDelta()).toBe(325)
    expect(logs.some((line) => line.includes('still failing'))).toBe(true)
    const writes = win.requests.length
    await settle()
    expect(win.requests).toHaveLength(writes)
    win.failNextSetMinSize(0)
    growth.setSidebars(contactCard)
    await settle()
    expect(win.state.min.width).toBe(925)
    expect(sizeCalls(win)).toHaveLength(1)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)
  })

  test('persistent resize failures are bounded and explicit same-card reconciliation recovers', async () => {
    const { win, growth, refused } = createHarness({ setSizeFailures: 100, autoResizeOnFailure: true })
    growth.setSidebars(contactCard)
    await settle()
    expect(sizeCalls(win)).toHaveLength(2)
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(win.state.min.width).toBe(600)
    expect(refused).toEqual([[{ id: 'contact-card', width: 320, accepted: null }]])
    win.fireResize()
    await settle()
    expect(sizeCalls(win)).toHaveLength(2)
    win.failNextSetSize(0)
    growth.setSidebars(contactCard)
    await settle()
    expect(win.state.width).toBe(1385)
    expect(win.state.min.width).toBe(925)
    expect(growth.getAppliedDelta()).toBe(325)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
  })

  test('repeated disposal repairs minimum debt after width ownership has already reached zero', async () => {
    const { win, growth } = createHarness()
    growth.setSidebars(contactCard)
    await settle()
    win.failNextSetMinSize(2)
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(925)
    expect(growth.getAppliedDelta()).toBe(0)
    const sizes = sizeCalls(win).length
    growth.dispose()
    await settle()
    expect(win.state.min.width).toBe(600)
    expect(sizeCalls(win)).toHaveLength(sizes)
    const writes = win.requests.length
    growth.dispose()
    await settle()
    expect(win.requests).toHaveLength(writes)
  })

  test('unknown ownership during post-write measurement retains pending geometry for close', async () => {
    const { win, growth } = createHarness()
    growth.setSidebars(contactCard)
    await settle()
    const setSize = win.setSize
    const isFullscreen = win.isFullscreen
    let unavailable = false
    win.isFullscreen = async () => { if (unavailable) throw new Error('fullscreen unavailable'); return isFullscreen() }
    win.setSize = async (size) => { await setSize(size); unavailable = true }
    growth.setSidebars([{ id: 'contact-card', width: 360 }])
    await settle()
    expect(win.state.width).toBe(1425)
    expect(growth.getAppliedDelta()).toBe(325)
    expect(win.state.min.width).toBe(925)
    unavailable = false
    win.setSize = setSize
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(win.state.min.width).toBe(600)
  })

  test('one representable physical pixel of room is not discarded by a logical half-pixel threshold', async () => {
    const { win, growth, refused } = createHarness({ scaleFactor: 3, origin: { x: 0, y: 50 }, monitor: createMonitor({ width: 3181 }), autoResize: true })
    growth.setSidebars(contactCard)
    await settle()
    expect((await win.innerSize()).width).toBe(3181)
    expect(growth.getAppliedDelta()).toBeCloseTo(1 / 3, 8)
    expect(win.state.min.width).toBeCloseTo(600 + 1 / 3, 8)
    expect(refused).toEqual([])
    const writes = win.requests.length
    win.fireResize()
    await settle()
    expect(win.requests).toHaveLength(writes)
    growth.dispose()
    await settle()
    expect((await win.innerSize()).width).toBe(3180)
    expect(growth.getAppliedDelta()).toBe(0)
    expect(win.state.min.width).toBe(600)
  })

  test('an unchanged open shrink clamp does not oscillate minimums or retry identical sizes', async () => {
    const { win, growth } = createHarness({ autoResize: true })
    growth.setSidebars(contactCard)
    await settle()
    const setSize = win.setSize
    win.setSize = (size) => setSize({ width: Math.max(1385, size.width), height: size.height })
    growth.setSidebars([{ id: 'contact-card', width: 300 }])
    await settle()
    expect(win.state.width).toBe(1385)
    expect(growth.getAppliedDelta()).toBe(325)
    expect(win.state.min.width).toBe(925)
    const writes = win.requests.length
    for (let n = 0; n < 3; n++) { win.fireResize(); await settle() }
    expect(win.requests).toHaveLength(writes)
    win.setSize = setSize
    growth.dispose()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)
    expect(growth.getAppliedDelta()).toBe(0)
  })

  test('非 Tauri（getWindow 返回 null）纯 DOM：零原生调用，增量为 0', async () => {
    const logs: string[] = []
    let monitorReads = 0
    const growth = createSidebarWindowGrowth({
      getWindow: () => null,
      getMonitor: async () => { monitorReads += 1; return null },
      log: (message) => logs.push(message),
    })
    growth.setSidebars([{ id: 'right-panel', width: 320 }])
    await settle()
    expect(growth.getAppliedDelta()).toBe(0)
    expect(logs).toEqual([])
    expect(monitorReads).toBe(0)
    growth.dispose()
  })
})
