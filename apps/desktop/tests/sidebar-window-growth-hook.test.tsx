// SPEC item 4：`useSidebarWindowGrowth` 的生命周期（挂载 / StrictMode / 卸载重挂 / 拒绝回滚）。
//
// 生产代码只有一个调用点（workbench 声明全部侧栏卡片），所以这里测的是这一个 hook 实例在
// 真实 React 生命周期下的表现：控制器挂在 ref 上而不是模块单例，卸载必须把自己拥有的增长
// 缩回去并解除 onResized 监听，StrictMode 双调用 effect 不能叠加两次增长。
import { afterEach, describe, expect, test } from 'bun:test'
import { StrictMode, useRef, useState } from 'react'
import { act, cleanup, render, renderHook } from '@testing-library/react'
import { sideCardGrowthRollback, useRightPanelGrowthWidth } from '../src/features/chat/workbench-side-cards'
import {
  SIDEBAR_CARD_GAP,
  useSidebarWindowGrowth,
  type SidebarGrowthEntry,
  type SidebarGrowthRefusal,
  type SidebarWindowGrowthDeps,
} from '../src/features/workspace/use-sidebar-window-growth'
import {
  createFakeWindow,
  createMonitor,
  drainDeferred,
  settle as idleNativeQueue,
  sizeCalls,
  type FakeMonitor,
  type FakeWindow,
} from './sidebar-window-growth-harness'

const cards: SidebarGrowthEntry[] = [{ id: 'contact-card', width: 320 }]

/** The harness barrier waits on the controller's promise queue; wrapping it in one
 *  `act` is what makes the React updates the controller notifies from (via
 *  `useSyncExternalStore`) legal, and what lets a test await the queue without
 *  counting timers. On Windows CI a zero-delay timer is clamped to ~15.6ms, so the
 *  previous "60 rounds of setTimeout(0) inside act" cost ~1s per call and the suite
 *  died on the 5s per-test timeout while the Linux run stayed green. */
const settle = () => act(async () => { await idleNativeQueue() })

/** Deferred mode: run the parked native writes, then let the queue go idle — inside
 *  one `act`, so both the native resolution and the React it notifies are flushed. */
async function flushNative(win: FakeWindow) {
  await act(async () => { await drainDeferred(win) })
}

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

function SidebarLayout({ entries, deps, onGrowthRefused }: {
  entries: SidebarGrowthEntry[]
  deps: SidebarWindowGrowthDeps
  onGrowthRefused?: (refusals: SidebarGrowthRefusal[]) => void
}) {
  const layoutRef = useRef<HTMLDivElement>(null)
  const owned = useSidebarWindowGrowth(entries, onGrowthRefused, layoutRef, deps)
  return (
    <div ref={layoutRef} data-testid="layout" data-growth={owned} style={{ display: 'flex', gap: SIDEBAR_CARD_GAP }}>
      <main style={{ flex: 1, minWidth: 0 }}>Chat</main>
      {entries.map((entry) => (
        <aside key={entry.id} data-testid={entry.id} style={{ width: entry.width, flexShrink: 0 }} />
      ))}
    </div>
  )
}

afterEach(cleanup)

describe('useSidebarWindowGrowth hook', () => {
  test('调整右栏只改变栏宽；关闭后缩回，重新打开按新栏宽增长一次', async () => {
    const win = createFakeWindow()
    const deps = createDeps(win)
    const hook = renderHook(() => {
      const [open, setOpen] = useState(false)
      const [width, setWidth] = useState(300)
      const paidWidth = useRightPanelGrowthWidth(open, width)
      const entries = paidWidth === null ? [] : [{ id: 'right-panel', width: paidWidth }]
      useSidebarWindowGrowth(entries, undefined, undefined, deps)
      return { setOpen, setWidth, width }
    })
    await settle()
    act(() => hook.result.current.setOpen(true))
    await settle()
    expect(win.state.width).toBe(1365)
    const callsAfterOpen = sizeCalls(win).length
    act(() => hook.result.current.setWidth(420))
    await settle()
    expect(hook.result.current.width).toBe(420)
    expect(win.state.width).toBe(1365)
    expect(sizeCalls(win)).toHaveLength(callsAfterOpen)
    act(() => hook.result.current.setWidth(280))
    await settle()
    expect(win.state.width).toBe(1365)
    expect(sizeCalls(win)).toHaveLength(callsAfterOpen)
    act(() => hook.result.current.setOpen(false))
    await settle()
    expect(win.state.width).toBe(1060)
    act(() => hook.result.current.setOpen(true))
    await settle()
    expect(win.state.width).toBe(1345)
    act(() => hook.result.current.setOpen(false))
    await settle()
    expect(win.state.width).toBe(1060)
    hook.unmount()
  })

  test('DOM width locks before native open and close finish, then returns to auto width', async () => {
    const win = createFakeWindow({ defer: true })
    const deps = createDeps(win)
    const view = render(<SidebarLayout entries={[]} deps={deps} />)
    const layout = view.getByTestId('layout')
    layout.getBoundingClientRect = () => new DOMRect(0, 0, win.state.width, win.state.height)
    await settle()
    expect(layout.style.width).toBe('')
    expect(win.pendingCount()).toBe(0)

    view.rerender(<SidebarLayout entries={cards} deps={deps} />)
    expect(view.getByTestId('contact-card').style.width).toBe('320px')
    expect(layout.style.width).toBe('1385px')
    expect(win.state.width).toBe(1060)
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    expect(layout.style.width).toBe('1385px')

    await act(async () => { win.flush(); await idleNativeQueue() })
    expect(win.state.width).toBe(1385)
    expect(layout.dataset.growth).toBe('325')
    expect(win.pendingNames()).toEqual(['setMinSize'])
    expect(layout.style.width).toBe('1385px')
    await flushNative(win)
    expect(layout.style.width).toBe('')

    view.rerender(<SidebarLayout entries={[]} deps={deps} />)
    expect(view.queryByTestId('contact-card')).toBeNull()
    expect(layout.style.width).toBe('1060px')
    expect(win.state.width).toBe(1385)
    await settle()
    expect(win.pendingNames()).toEqual(['setMinSize'])
    await act(async () => { win.flush(); await idleNativeQueue() })
    expect(win.pendingNames()).toEqual(['setSize'])
    expect(layout.style.width).toBe('1060px')
    expect(win.state.width).toBe(1385)
    await flushNative(win)
    expect(win.state.width).toBe(1060)
    expect(layout.dataset.growth).toBe('0')
    expect(layout.style.width).toBe('')
    view.unmount()
    await flushNative(win)
  })

  test('rapid open/close/open keeps the latest DOM lock when an old resize settles', async () => {
    const win = createFakeWindow({ defer: true })
    const deps = createDeps(win)
    const view = render(<SidebarLayout entries={[]} deps={deps} />)
    const layout = view.getByTestId('layout')
    layout.getBoundingClientRect = () => new DOMRect(0, 0, win.state.width, win.state.height)
    await settle()

    view.rerender(<SidebarLayout entries={cards} deps={deps} />)
    expect(layout.style.width).toBe('1385px')
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    view.rerender(<SidebarLayout entries={[]} deps={deps} />)
    expect(layout.style.width).toBe('1060px')
    view.rerender(<SidebarLayout entries={cards} deps={deps} />)
    expect(layout.style.width).toBe('1385px')
    expect(win.state.width).toBe(1060)

    await act(async () => { win.flush(); await idleNativeQueue() })
    expect(win.state.width).toBe(1385)
    expect(win.pendingNames()).toEqual(['setMinSize'])
    expect(layout.style.width).toBe('1385px')
    await flushNative(win)
    expect(layout.style.width).toBe('')
    expect(layout.dataset.growth).toBe('325')
    expect(sizeCalls(win).map((call) => call.width)).toEqual([1385])
    view.unmount()
    await flushNative(win)
    expect(win.state.width).toBe(1060)
  })

  test('unmount clears the detached DOM lock before an in-flight resize completes', async () => {
    const win = createFakeWindow({ defer: true })
    const deps = createDeps(win)
    const view = render(<SidebarLayout entries={[]} deps={deps} />)
    const layout = view.getByTestId('layout')
    layout.getBoundingClientRect = () => new DOMRect(0, 0, win.state.width, win.state.height)
    await settle()
    view.rerender(<SidebarLayout entries={cards} deps={deps} />)
    expect(layout.style.width).toBe('1385px')
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])

    view.unmount()
    expect(layout.isConnected).toBe(false)
    expect(layout.style.width).toBe('')
    await flushNative(win)
    expect(layout.style.width).toBe('')
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)
    expect(win.pendingCount()).toBe(0)
    expect(win.listenerCount()).toBe(0)
  })

  test.each(['maximized', 'fullscreen'] as const)('%s never locks DOM width', async (mode) => {
    const win = createFakeWindow({ defer: true, [mode]: true })
    const deps = createDeps(win)
    const view = render(<SidebarLayout entries={[]} deps={deps} />)
    const layout = view.getByTestId('layout')
    layout.getBoundingClientRect = () => new DOMRect(0, 0, win.state.width, win.state.height)
    await settle()

    view.rerender(<SidebarLayout entries={cards} deps={deps} />)
    expect(layout.style.width).toBe('')
    await settle()
    expect(layout.style.width).toBe('')
    expect(layout.dataset.growth).toBe('0')
    expect(view.queryByTestId('contact-card')).not.toBeNull()
    view.rerender(<SidebarLayout entries={[]} deps={deps} />)
    expect(layout.style.width).toBe('')
    await settle()
    expect(layout.style.width).toBe('')
    expect(win.requests).toEqual([])
    view.unmount()
    await flushNative(win)
    expect(win.listenerCount()).toBe(0)
  })

  test('browser-only cards never lock DOM width or call native writes', async () => {
    const win = createFakeWindow({ defer: true })
    const deps = { ...createDeps(win), getWindow: () => null }
    const view = render(<SidebarLayout entries={[]} deps={deps} />)
    const layout = view.getByTestId('layout')
    layout.getBoundingClientRect = () => new DOMRect(0, 0, win.state.width, win.state.height)
    await settle()

    view.rerender(<SidebarLayout entries={cards} deps={deps} />)
    expect(layout.style.width).toBe('')
    await settle()
    expect(layout.style.width).toBe('')
    expect(layout.dataset.growth).toBe('0')
    expect(view.queryByTestId('contact-card')).not.toBeNull()
    view.rerender(<SidebarLayout entries={[]} deps={deps} />)
    expect(layout.style.width).toBe('')
    await settle()
    expect(layout.style.width).toBe('')
    view.unmount()
    await settle()
    expect(win.requests).toEqual([])
    expect(win.calls).toEqual([])
    expect(win.listenerCount()).toBe(0)
  })

  test('failed native growth rolls the real card DOM back without leaving a fixed width', async () => {
    const win = createFakeWindow({ defer: true, setSizeFailures: 2 })
    const deps = createDeps(win)
    const refused: SidebarGrowthRefusal[][] = []
    function RollbackLayout() {
      const [entries, setEntries] = useState<SidebarGrowthEntry[]>([])
      return (
        <>
          <button onClick={() => setEntries(cards)}>Open contact</button>
          <SidebarLayout entries={entries} deps={deps} onGrowthRefused={(failures) => {
            refused.push(failures)
            if (sideCardGrowthRollback(failures).closeContactCard) setEntries([])
          }} />
        </>
      )
    }
    const view = render(<RollbackLayout />)
    const layout = view.getByTestId('layout')
    layout.getBoundingClientRect = () => new DOMRect(0, 0, win.state.width, win.state.height)
    await settle()
    act(() => { view.getByRole('button', { name: 'Open contact' }).click() })
    expect(view.queryByTestId('contact-card')).not.toBeNull()
    expect(layout.style.width).toBe('1385px')
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    await act(async () => { win.flush(); await idleNativeQueue() })
    expect(win.pendingNames()).toEqual(['setSize'])
    expect(layout.style.width).toBe('1385px')
    expect(refused).toEqual([])

    await flushNative(win)
    await settle()
    expect(refused).toEqual([[{ id: 'contact-card', width: 320, accepted: null }]])
    expect(view.queryByTestId('contact-card')).toBeNull()
    expect(layout.style.width).toBe('')
    expect(layout.dataset.growth).toBe('0')
    expect(sizeCalls(win)).toHaveLength(2)
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)
    view.unmount()
    await flushNative(win)
  })

  test.each([
    { workAreaWidth: 2400, nativeWidth: 1200 },
    { workAreaWidth: 2000, nativeWidth: 1060 },
  ])('work-area clamp at $workAreaWidth releases the DOM lock without closing the card', async ({ workAreaWidth, nativeWidth }) => {
    const win = createFakeWindow({ defer: true })
    const monitor = createMonitor()
    monitor.workArea.size.width = workAreaWidth
    const deps = createDeps(win, { monitor })
    const refused: SidebarGrowthRefusal[][] = []
    const onGrowthRefused = (failures: SidebarGrowthRefusal[]) => { refused.push(failures) }
    const view = render(<SidebarLayout entries={[]} deps={deps} onGrowthRefused={onGrowthRefused} />)
    const layout = view.getByTestId('layout')
    layout.getBoundingClientRect = () => new DOMRect(0, 0, win.state.width, win.state.height)
    await settle()

    view.rerender(<SidebarLayout entries={cards} deps={deps} onGrowthRefused={onGrowthRefused} />)
    expect(layout.style.width).toBe('1385px')
    expect(win.state.width).toBe(1060)
    await settle()
    expect(win.pendingNames()).toEqual(['setPosition'])
    expect(layout.style.width).toBe('1385px')
    await flushNative(win)
    expect(win.state.width).toBe(nativeWidth)
    expect(layout.dataset.growth).toBe(String(nativeWidth - 1060))
    expect(layout.style.width).toBe('')
    expect(view.getByTestId('contact-card').style.width).toBe('320px')
    expect(refused).toEqual([])
    expect(win.pendingCount()).toBe(0)

    view.rerender(<SidebarLayout entries={[]} deps={deps} onGrowthRefused={onGrowthRefused} />)
    await flushNative(win)
    expect(win.state.width).toBe(1060)
    expect(layout.style.width).toBe('')
    view.unmount()
    await flushNative(win)
  })

  test('挂载即增长，声明变化即调整，卸载缩回并解除监听', async () => {
    const win = createFakeWindow()
    const deps = createDeps(win)
    const { result, rerender, unmount } = renderHook(
      ({ entries }: { entries: SidebarGrowthEntry[] }) => useSidebarWindowGrowth(entries, undefined, undefined, deps),
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
    const { result, unmount } = renderHook(() => useSidebarWindowGrowth(cards, undefined, undefined, createDeps(win)), { wrapper: StrictMode })
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
      ({ entries }: { entries: SidebarGrowthEntry[] }) => useSidebarWindowGrowth(entries, undefined, undefined, deps),
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
    const first = renderHook(() => useSidebarWindowGrowth(cards, undefined, undefined, deps))
    await settle()
    expect(win.state.width).toBe(1385)
    first.unmount()
    await settle()
    expect(win.state.width).toBe(1060)

    const second = renderHook(() => useSidebarWindowGrowth(cards, undefined, undefined, deps))
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
    const refused: SidebarGrowthRefusal[][] = []
    // 工作区只有 2000 物理宽，而窗口外框本身就占 2120：完全没有增长空间。
    const monitor = {
      name: 'MOCK',
      size: { width: 2560, height: 1440 },
      position: { x: 0, y: 0 },
      workArea: { position: { x: 0, y: 0 }, size: { width: 2000, height: 1344 } },
      scaleFactor: 1,
    } satisfies FakeMonitor
    const deps = createDeps(win, { monitor })
    const { result } = renderHook(() => useSidebarWindowGrowth(cards, (refusals) => { refused.push(refusals) }, undefined, deps))
    await settle()
    expect(result.current).toBe(0)
    expect(sizeCalls(win)).toEqual([])
    // SPEC §4 的最后手段是聊天变窄，卡片保持声明宽度；DOM 一旦回滚就把卡片关掉了。
    expect(refused).toEqual([])
  })

  test('原生 setSize 两次都失败：只回报真正要不到宽度的卡片，DOM 才回滚它', async () => {
    const win = createFakeWindow({ setSizeFailures: 2 })
    const refused: SidebarGrowthRefusal[][] = []
    const deps = createDeps(win)
    const { result } = renderHook(() => useSidebarWindowGrowth(cards, (refusals) => { refused.push(refusals) }, undefined, deps))
    await settle()
    expect(refused).toEqual([[{ id: 'contact-card', width: 320, accepted: null }]])
    expect(result.current).toBe(0)
    expect(win.state.width).toBe(1060)
  })

  test('非 Tauri（浏览器预览）：零原生调用，增量为 0，卡片靠压缩聊天存在', async () => {
    const win = createFakeWindow()
    const { result } = renderHook(() => useSidebarWindowGrowth(cards, undefined, undefined, { getWindow: () => null, getMonitor: async () => null, log: () => undefined }))
    await settle()
    expect(result.current).toBe(0)
    expect(win.calls).toEqual([])
  })

  test('real remount during cleanup cancels an unsent shrink and keeps forwarding ownership', async () => {
    const win = createFakeWindow({ defer: true })
    const deps = createDeps(win)
    const first = renderHook(() => useSidebarWindowGrowth(cards, undefined, undefined, deps))
    await flushNative(win)
    expect(first.result.current).toBe(325)
    expect(win.state.width).toBe(1385)
    expect(win.state.min.width).toBe(925)
    first.unmount()
    await settle()
    expect(win.pendingNames()).toEqual(['setMinSize'])
    const second = renderHook(() => useSidebarWindowGrowth(cards, undefined, undefined, deps))
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
    const first = renderHook(() => useSidebarWindowGrowth(cards, undefined, undefined, deps))
    await settle()
    expect(first.result.current).toBe(325)
    win.failNextSetMinSize(2)
    first.unmount()
    await settle()
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(925)
    expect(win.listenerCount()).toBe(0)
    const sizes = sizeCalls(win).length
    const second = renderHook(() => useSidebarWindowGrowth([], undefined, undefined, deps))
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
    const first = renderHook(() => useSidebarWindowGrowth(cards, undefined, undefined, deps), { wrapper: StrictMode })
    await settle()
    expect(first.result.current).toBe(325)
    expect(win.state.width).toBe(1385)
    expect(win.listenerCount()).toBe(1)
    first.unmount()
    await settle()
    expect(win.state.width).toBe(1060)
    const second = renderHook(
      ({ entries }: { entries: SidebarGrowthEntry[] }) => useSidebarWindowGrowth(entries, undefined, undefined, deps),
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
    win.fireResize()
    await settle()
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
    const first = renderHook(() => useSidebarWindowGrowth(cards, undefined, undefined, deps))
    const second = renderHook(() => useSidebarWindowGrowth(cards, undefined, undefined, deps))
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
    win.state.maximized = true
    win.fireResize()
    await settle()
    expect(second.result.current).toBe(0)
    expect(win.state.min.width).toBe(600)
    win.state.maximized = false
    win.fireResize()
    await settle()
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
    const refused: SidebarGrowthRefusal[][] = []
    const hook = renderHook(
      ({ entries }: { entries: SidebarGrowthEntry[] }) => useSidebarWindowGrowth(entries, (failed) => { refused.push(failed) }, undefined, deps),
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

  test('refusal names the requester, not the last declared card', async () => {
    const win = createFakeWindow()
    const deps = createDeps(win)
    const refused: SidebarGrowthRefusal[][] = []
    const { result, rerender } = renderHook(
      ({ entries }: { entries: SidebarGrowthEntry[] }) => useSidebarWindowGrowth(entries, (refusals) => { refused.push(refusals) }, undefined, deps),
      { initialProps: { entries: cards } },
    )
    await settle()
    expect(win.state.width).toBe(1385)

    // The contact card is already paid for; the panel that opens next is the one
    // asking for room, even though it is merely the last entry of the declaration.
    win.failNextSetSize(2)
    rerender({ entries: [...cards, { id: 'right-panel', width: 300 }] })
    await settle()
    expect(refused).toEqual([[{ id: 'right-panel', width: 300, accepted: null }]])
    expect(result.current).toBe(325)
    expect(win.state.width).toBe(1385)

    // A refused pass never becomes the baseline the DOM may rely on: widening the
    // card that DID land is reported with the width it really has, so the UI can
    // restore it instead of closing it.
    win.failNextSetSize(2)
    rerender({ entries: [{ id: 'contact-card', width: 420 }] })
    await settle()
    expect(refused).toEqual([
      [{ id: 'right-panel', width: 300, accepted: null }],
      [{ id: 'contact-card', width: 420, accepted: 320 }],
    ])
    expect(result.current).toBe(325)
    expect(win.state.width).toBe(1385)
  })

  test('a superseded late SUCCESS keeps the accommodated card out of the refusal', async () => {
    // 审查 P2 的确切交错（reviewer 的 deferred 复现），走真实共享 hook：
    // 联系人卡的 resize 已派发时面板插队声明；旧一代的写随后成功落地（1385 / 拥有 325），
    // 面板两次失败时的拒绝名单里绝不能有联系人卡——否则 Workbench 会连带关掉这张已经
    // 被容纳、且 DOM 无意关闭的卡。
    const win = createFakeWindow({ defer: true })
    const deps = createDeps(win)
    const refused: SidebarGrowthRefusal[][] = []
    const { result, rerender, unmount } = renderHook(
      ({ entries }: { entries: SidebarGrowthEntry[] }) => useSidebarWindowGrowth(entries, (refusals) => { refused.push(refusals) }, undefined, deps),
      { initialProps: { entries: cards } },
    )
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    rerender({ entries: [{ id: 'right-panel', width: 300 }, ...cards] })
    await settle()
    await act(async () => { win.flush() })
    await settle()
    expect(win.state.width).toBe(1385)
    expect(result.current).toBe(325)
    win.failNextSetSize(2)
    await flushNative(win)
    expect(refused).toEqual([[{ id: 'right-panel', width: 300, accepted: null }]])
    expect(result.current).toBe(325)
    expect(win.state.width).toBe(1385)
    unmount()
    await flushNative(win)
    expect(win.state.width).toBe(1060)
    expect(win.state.min.width).toBe(600)
  })

  test('delayed successful measurement keeps contact credit through the real refusal callback', async () => {
    const win = createFakeWindow({ defer: true })
    const deps = createDeps(win)
    const refused: SidebarGrowthRefusal[][] = []
    const { result, rerender, unmount } = renderHook(
      ({ entries }: { entries: SidebarGrowthEntry[] }) => useSidebarWindowGrowth(entries, (failures) => { refused.push(failures) }, undefined, deps),
      { initialProps: { entries: cards } },
    )
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    win.failNextInnerSize(1)
    await act(async () => { win.flush(); await idleNativeQueue() })
    expect(win.state.width).toBe(1385)
    expect(result.current).toBe(0)
    expect(win.pendingCount()).toBe(0)
    expect(refused).toEqual([])
    win.failNextSetSize(2)
    rerender({ entries: [{ id: 'right-panel', width: 300 }, ...cards] })
    await flushNative(win)
    expect(refused).toEqual([[{ id: 'right-panel', width: 300, accepted: null }]])
    expect(result.current).toBe(325)
    expect(win.state.width).toBe(1385)
    expect(win.state.min.width).toBe(925)
    expect(win.requests.filter((call) => call.name === 'setSize').map((call) => call.width)).toEqual([1385, 1690, 1690])
    expect(win.listenerCount()).toBe(1)
    rerender({ entries: cards })
    await flushNative(win)
    expect(sizeCalls(win)).toHaveLength(3)
    unmount()
    await flushNative(win)
    expect(win.state.width).toBe(1060)
    expect(win.state.height).toBe(778)
    expect(win.state.min.width).toBe(600)
    expect(win.pendingCount()).toBe(0)
    expect(win.listenerCount()).toBe(0)
    expect(win.unlistenCount()).toBe(1)
  })

  test('delayed old sibling success rolls panel 340 back to 300 through the real hook without closing either card', async () => {
    const win = createFakeWindow({ defer: true })
    const deps = createDeps(win)
    const refused: SidebarGrowthRefusal[][] = []
    const { result, unmount } = renderHook(() => {
      const [panelWidth, setPanelWidth] = useState(300)
      const [panelOpen, setPanelOpen] = useState(true)
      const [contactOpen, setContactOpen] = useState(false)
      const entries: SidebarGrowthEntry[] = []
      if (panelOpen) entries.push({ id: 'right-panel', width: panelWidth })
      if (contactOpen) entries.push(...cards)
      const owned = useSidebarWindowGrowth(entries, (failures) => {
        refused.push(failures)
        const rollback = sideCardGrowthRollback(failures)
        if (rollback.closeContactCard) setContactOpen(false)
        if (rollback.closeRightPanel) setPanelOpen(false)
        else if (rollback.restoreRightPanelWidth !== null) setPanelWidth(rollback.restoreRightPanelWidth)
      }, undefined, deps)
      return { owned, entries, panelWidth, panelOpen, contactOpen, setPanelWidth, setContactOpen }
    })
    await flushNative(win)
    expect(win.state.width).toBe(1365)
    expect(result.current.owned).toBe(305)
    act(() => { result.current.setContactOpen(true) })
    await settle()
    expect(win.pendingNames()).toEqual(['setSize'])
    act(() => { result.current.setPanelWidth(340) })
    win.failNextInnerSize(1)
    await act(async () => { win.flush(); await idleNativeQueue() })
    expect(win.state.width).toBe(1690)
    expect(result.current.owned).toBe(630)
    expect(result.current.panelWidth).toBe(340)
    expect(refused).toEqual([])
    win.failNextSetSize(2)
    await flushNative(win)
    await settle()
    expect(refused).toEqual([[{ id: 'right-panel', width: 340, accepted: 300 }]])
    expect(result.current.panelWidth).toBe(300)
    expect(result.current.panelOpen).toBe(true)
    expect(result.current.contactOpen).toBe(true)
    expect(result.current.entries).toEqual([{ id: 'right-panel', width: 300 }, ...cards])
    expect(result.current.owned).toBe(630)
    expect(win.state.width).toBe(1690)
    expect(win.state.min.width).toBe(1230)
    expect(win.requests.filter((call) => call.name === 'setSize').map((call) => call.width)).toEqual([1365, 1690, 1730, 1730])
    expect(win.pendingCount()).toBe(0)
    unmount()
    await flushNative(win)
    expect(win.state.width).toBe(1060)
    expect(win.state.height).toBe(778)
    expect(win.state.min.width).toBe(600)
    expect(win.pendingCount()).toBe(0)
    expect(win.listenerCount()).toBe(0)
  })

  test('在途增长 + 立即卸载：卸载仍把真实落地的增长缩回来', async () => {
    const win = createFakeWindow({ defer: true })
    const deps = createDeps(win)
    const { unmount } = renderHook(() => useSidebarWindowGrowth(cards, undefined, undefined, deps))
    await settle()
    // 真实的“在途”：增长调用已经发给原生层，还压在延迟队列里没执行。
    expect(win.pendingNames()).toEqual(['setSize'])
    expect(win.state.width).toBe(1060)
    expect(sizeCalls(win)).toEqual([])

    unmount()
    await flushNative(win)
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
