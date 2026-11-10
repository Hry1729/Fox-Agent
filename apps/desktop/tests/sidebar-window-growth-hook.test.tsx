// SPEC item 4：`useSidebarWindowGrowth` 的生命周期（挂载 / StrictMode / 卸载重挂 / 拒绝回滚）。
//
// 生产代码只有一个调用点（workbench 声明全部侧栏卡片），所以这里测的是这一个 hook 实例在
// 真实 React 生命周期下的表现：控制器挂在 ref 上而不是模块单例，卸载必须把自己拥有的增长
// 缩回去并解除 onResized 监听，StrictMode 双调用 effect 不能叠加两次增长。
import { afterEach, describe, expect, test } from 'bun:test'
import { StrictMode } from 'react'
import { act, cleanup, renderHook } from '@testing-library/react'
import {
  SIDEBAR_CARD_GAP,
  useSidebarWindowGrowth,
  type SidebarGrowthEntry,
  type SidebarWindowGrowthDeps,
} from '../src/features/workspace/use-sidebar-window-growth'
import {
  createFakeWindow,
  createMonitor,
  settle,
  sizeCalls,
  type FakeMonitor,
  type FakeWindow,
} from './sidebar-window-growth-harness'

const cards: SidebarGrowthEntry[] = [{ id: 'contact-card', width: 320 }]

// The refusal callback always comes from the hook argument: `createShared` wraps
// the box, so a callback baked into `deps` would be ignored and mislead the reader.
function createDeps(
  win: FakeWindow,
  options: { monitor?: FakeMonitor | null } = {},
): SidebarWindowGrowthDeps {
  return {
    getWindow: () => win,
    getMonitor: async () => (options.monitor === undefined ? createMonitor() : options.monitor),
    log: () => undefined,
  }
}

async function flushNative(win: FakeWindow) {
  for (let i = 0; i < 60; i++) {
    win.flush()
    await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
  }
}

afterEach(cleanup)

describe('useSidebarWindowGrowth hook', () => {
  test('挂载即增长，声明变化即调整，卸载缩回并解除监听', async () => {
    const win = createFakeWindow()
    const deps = createDeps(win)
    const { result, rerender, unmount } = renderHook(
      ({ entries }: { entries: SidebarGrowthEntry[] }) => useSidebarWindowGrowth(entries, undefined, deps),
      { initialProps: { entries: cards } },
    )
    await settle()
    expect(result.current).toBe(320 + SIDEBAR_CARD_GAP)
    expect(win.state.width).toBe(1385)

    rerender({ entries: [{ id: 'contact-card', width: 320 }, { id: 'right-panel', width: 300 }] })
    await settle()
    // 两张卡各自带 5px 间隙：325 + 305，不是 320 + 300。
    expect(result.current).toBe(630)
    expect(win.state.width).toBe(1690)
    expect(win.state.height).toBe(778)
    expect(win.state.min.width).toBe(600 + 630)

    rerender({ entries: [] })
    await settle()
    expect(result.current).toBe(0)
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)

    rerender({ entries: cards })
    await settle()
    expect(win.state.width).toBe(1385)

    unmount()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)
    expect(win.unlistenCount()).toBe(1)
  })

  test('StrictMode：effect 双调用只增长一次，卸载也不残留', async () => {
    const win = createFakeWindow()
    const { result, unmount } = renderHook(() => useSidebarWindowGrowth(cards, undefined, createDeps(win)), { wrapper: StrictMode })
    await settle()
    expect(result.current).toBe(325)
    expect(win.state.width).toBe(1385)
    expect(sizeCalls(win).length).toBe(1)
    unmount()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(win.unlistenCount()).toBe(1)
  })

  test('StrictMode 重挂之后仍然收得到增长通知：转发通道不被身份短路', async () => {
    const win = createFakeWindow()
    const deps = createDeps(win)
    const { result, rerender, unmount } = renderHook(
      ({ entries }: { entries: SidebarGrowthEntry[] }) => useSidebarWindowGrowth(entries, undefined, deps),
      { initialProps: { entries: cards }, wrapper: StrictMode },
    )
    await settle()
    expect(result.current).toBe(325)

    // StrictMode 的 cleanup 解掉转发却保留 growth 指针；若重挂时 bindGrowth 因此
    // 短路返回，这条通道就永远聋了：声明再变也不会通知 React，hook 返回值停在旧值。
    rerender({ entries: [{ id: 'contact-card', width: 320 }, { id: 'right-panel', width: 300 }] })
    await settle()
    expect(result.current).toBe(630)
    expect(win.state.width).toBe(1690)

    rerender({ entries: [] })
    await settle()
    expect(result.current).toBe(0)
    expect(win.state.width).toBe(1060)

    unmount()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(win.unlistenCount()).toBe(1)
  })

  test('卸载后重新挂载：从缩回来的宽度重新增长，而不是叠加', async () => {
    const win = createFakeWindow()
    const deps = createDeps(win)
    const first = renderHook(() => useSidebarWindowGrowth(cards, undefined, deps))
    await settle()
    expect(win.state.width).toBe(1385)
    first.unmount()
    await settle()
    expect(win.state.width).toBe(1060)

    const second = renderHook(() => useSidebarWindowGrowth(cards, undefined, deps))
    await settle()
    expect(second.result.current).toBe(325)
    expect(win.state.width).toBe(1385)
    // 第二次挂载是重新增长，不是在 1385 上再叠一层。
    expect(sizeCalls(win).filter((call) => call.width === 1710)).toEqual([])
    second.unmount()
    await settle()
    expect(win.state.width).toBe(1060)
  })

  test('工作区不够：不是原生失败，绝不回报（卡片只能压缩聊天）', async () => {
    const win = createFakeWindow()
    const refused: SidebarGrowthEntry[][] = []
    // 工作区只有 2000 物理宽，而窗口外框本身就占 2120：完全没有增长空间。
    const monitor = {
      name: 'MOCK',
      size: { width: 2560, height: 1440 },
      position: { x: 0, y: 0 },
      workArea: { position: { x: 0, y: 0 }, size: { width: 2000, height: 1344 } },
      scaleFactor: 1,
    } satisfies FakeMonitor
    const deps = createDeps(win, { monitor })
    const { result } = renderHook(() => useSidebarWindowGrowth(cards, (entries) => { refused.push(entries) }, deps))
    await settle()
    expect(result.current).toBe(0)
    expect(sizeCalls(win)).toEqual([])
    // SPEC §4 的最后手段是聊天变窄，卡片保持声明宽度；DOM 一旦回滚就把卡片关掉了。
    expect(refused).toEqual([])
  })

  test('原生 setSize 两次都失败：回报声明，DOM 才回滚卡片', async () => {
    const win = createFakeWindow({ setSizeFailures: 2 })
    const refused: SidebarGrowthEntry[][] = []
    const deps = createDeps(win)
    const { result } = renderHook(() => useSidebarWindowGrowth(cards, (entries) => { refused.push(entries) }, deps))
    await settle()
    expect(refused).toEqual([cards])
    expect(result.current).toBe(0)
    expect(win.state.width).toBe(1060)
  })

  test('非 Tauri（浏览器预览）：零原生调用，增量为 0，卡片靠压缩聊天存在', async () => {
    const win = createFakeWindow()
    const { result } = renderHook(() => useSidebarWindowGrowth(cards, undefined, { getWindow: () => null, getMonitor: async () => null, log: () => undefined }))
    await settle()
    expect(result.current).toBe(0)
    expect(win.calls).toEqual([])
  })

  test('real remount during cleanup cancels an unsent shrink and keeps forwarding ownership', async () => {
    const win = createFakeWindow({ defer: true })
    const deps = createDeps(win)
    const first = renderHook(() => useSidebarWindowGrowth(cards, undefined, deps))
    await flushNative(win)
    expect(first.result.current).toBe(325)
    expect(win.state.width).toBe(1385)
    expect(win.state.min.width).toBe(925)
    first.unmount()
    await settle()
    expect(win.pendingNames()).toEqual(['setMinSize'])
    const second = renderHook(() => useSidebarWindowGrowth(cards, undefined, deps))
    await flushNative(win)
    expect(second.result.current).toBe(325)
    expect(win.state.width).toBe(1385)
    expect(win.state.min.width).toBe(925)
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1385])
    expect(win.listenerCount()).toBe(1)
    expect(win.pendingCount()).toBe(0)
    second.unmount()
    await flushNative(win)
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)
    expect(win.listenerCount()).toBe(0)
    expect(win.unlistenCount()).toBe(2)
  })

  test('real remount repairs cleanup minimum debt even with zero size ownership', async () => {
    const win = createFakeWindow()
    const deps = createDeps(win)
    const first = renderHook(() => useSidebarWindowGrowth(cards, undefined, deps))
    await settle()
    expect(first.result.current).toBe(325)
    win.failNextSetMinSize(2)
    first.unmount()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(925)
    expect(win.listenerCount()).toBe(0)
    const sizes = sizeCalls(win).length
    const second = renderHook(() => useSidebarWindowGrowth([], undefined, deps))
    await settle()
    expect(second.result.current).toBe(0)
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)
    expect(sizeCalls(win)).toHaveLength(sizes)
    expect(win.listenerCount()).toBe(1)
    second.unmount()
    await settle()
    expect(win.listenerCount()).toBe(0)
  })

  test('late listener registration cleans up across real remount without doubled growth', async () => {
    const win = createFakeWindow({ deferListeners: true, autoResize: true })
    const deps = createDeps(win)
    const first = renderHook(() => useSidebarWindowGrowth(cards, undefined, deps), { wrapper: StrictMode })
    await settle()
    expect(first.result.current).toBe(325)
    expect(win.state.width).toBe(1385)
    expect(win.listenerCount()).toBe(1)
    first.unmount()
    await settle()
    expect(win.state.width).toBe(1060)
    const second = renderHook(
      ({ entries }: { entries: SidebarGrowthEntry[] }) => useSidebarWindowGrowth(entries, undefined, deps),
      { initialProps: { entries: cards }, wrapper: StrictMode },
    )
    await settle()
    expect(second.result.current).toBe(325)
    expect(win.state.width).toBe(1385)
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1385, 1060, 1385])
    win.resolveListeners()
    await settle()
    expect(win.unlistenCount()).toBe(1)
    expect(win.listenerCount()).toBe(1)
    win.state.width = 1500
    win.state.height = 900
    await act(async () => { win.fireResize(); await settle() })
    expect(second.result.current).toBe(325)
    expect(win.state.width).toBe(1500)
    second.rerender({ entries: [] })
    await settle()
    expect(second.result.current).toBe(0)
    expect(win.state.width).toBe(1175)
    expect(win.state.height).toBe(900)
    expect(win.state.min.width).toBe(600)
    second.unmount()
    await settle()
    expect(win.listenerCount()).toBe(0)
    expect(win.unlistenCount()).toBe(2)
  })

  test('same-declaration consumers share real geometry and one listener until the last unmount', async () => {
    const win = createFakeWindow({ autoResize: true })
    const deps = createDeps(win)
    const first = renderHook(() => useSidebarWindowGrowth(cards, undefined, deps))
    const second = renderHook(() => useSidebarWindowGrowth(cards, undefined, deps))
    await settle()
    expect(first.result.current).toBe(325)
    expect(second.result.current).toBe(325)
    expect(win.state.width).toBe(1385)
    expect(win.state.min.width).toBe(925)
    expect(sizeCalls(win)).toHaveLength(1)
    expect(win.listenerCount()).toBe(1)
    first.unmount()
    await settle()
    expect(win.state.width).toBe(1385)
    expect(win.listenerCount()).toBe(1)
    await act(async () => { win.state.maximized = true; win.fireResize(); await settle() })
    expect(second.result.current).toBe(0)
    expect(win.state.min.width).toBe(600)
    await act(async () => { win.state.maximized = false; win.fireResize(); await settle() })
    expect(second.result.current).toBe(325)
    expect(win.state.width).toBe(1385)
    expect(win.state.min.width).toBe(925)
    second.unmount()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)
    expect(win.listenerCount()).toBe(0)
  })

  test('a late failure from the old card never reaches the replacement card callback', async () => {
    const win = createFakeWindow({ defer: true, setSizeFailures: 1, setSizeAppliesAnyway: true })
    const deps = createDeps(win)
    const refused: SidebarGrowthEntry[][] = []
    const hook = renderHook(
      ({ entries }: { entries: SidebarGrowthEntry[] }) => useSidebarWindowGrowth(entries, (failed) => { refused.push(failed) }, deps),
      { initialProps: { entries: cards } },
    )
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    hook.rerender({ entries: [{ id: 'right-panel', width: 360 }] })
    await flushNative(win)
    expect(refused).toEqual([])
    expect(hook.result.current).toBe(365)
    expect(win.state.width).toBe(1425)
    expect(win.state.min.width).toBe(965)
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1385, 1425])
    hook.unmount()
    await flushNative(win)
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)
    expect(win.listenerCount()).toBe(0)
  })

  test('在途增长 + 立即卸载：卸载仍把真实落地的增长缩回来', async () => {
    const win = createFakeWindow({ defer: true })
    const deps = createDeps(win)
    const { unmount } = renderHook(() => useSidebarWindowGrowth(cards, undefined, deps))
    await settle()
    // 真实的“在途”：增长调用已经发给原生层，还压在延迟队列里没执行。
    expect(win.pendingNames()).toEqual(['setSize'])
    expect(win.state.width).toBe(1060)
    expect(sizeCalls(win)).toEqual([])

    unmount()
    for (let i = 0; i < 60; i++) {
      win.flush()
      await act(async () => { await new Promise((resolve) => setTimeout(resolve, 0)) })
    }
    // 增长先落地（1385），排在其后的 dispose 对账再把它缩回 1060。
    expect(sizeCalls(win).some((call) => call.width === 1385)).toBe(true)
    expect(sizeCalls(win).at(-1)).toMatchObject({ width: 1060, height: 778 })
    expect(win.state.width).toBe(1060)
    expect(win.state.height).toBe(778)
    expect(win.state.min.width).toBe(600)
    expect(win.pendingCount()).toBe(0)
    expect(win.unlistenCount()).toBe(1)
  })
})
