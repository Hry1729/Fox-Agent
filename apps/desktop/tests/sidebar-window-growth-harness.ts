/**
 * Shared fake for the sidebar-window-growth tests.
 *
 * Modelled on the REAL installed `@tauri-apps/api` 2.x window surface (there is no
 * `minSize()` getter, and `currentMonitor` is a module function rather than a Window
 * method, so it is injected separately):
 *  - `innerSize` / `outerSize` / `outerPosition` resolve to PHYSICAL pixels, `outerSize`
 *    including the window chrome;
 *  - `setSize` / `setMinSize` / `setPosition` receive LOGICAL pixels, and a real
 *    window refuses to shrink below its current min — the fake enforces that too,
 *    so a stale min produces the same failure the native layer would.
 *    `failNextSetMinSize()` arms that write failure at any point, which is how
 *    "the min reset fails on the CLOSE pass after a clean open" is reproducible;
 *  - `defer` parks every NATIVE MUTATION (`setSize` / `setMinSize` /
 *    `setPosition`) — with its REQUEST, so a resize parked behind a failed min
 *    reset is clamped at flush time — until the test flushes it, which is how
 *    "closing while a growth is inflight" becomes observable. Reads resolve
 *    immediately: a real window answers a geometry query without blocking on the
 *    resize that is still in flight, and parking reads would stall `reconcile`
 *    before it ever dispatched a mutation;
 *  - assertions read `state` (actual width / height / min / position), not counters.
 */
import {
  BASE_MIN_HEIGHT,
  BASE_MIN_WIDTH,
  type MonitorLike,
  type SidebarGrowthEntry,
  type SidebarWindowGrowthDeps,
  type SidebarWindowLike,
  createSidebarWindowGrowth,
} from '../src/features/workspace/use-sidebar-window-growth'

export const round = (value: number) => Math.round(value)

export interface FakeCall { name: string; width?: number; height?: number; x?: number; y?: number }

export interface FakeState {
  /** Logical inner (content) size. */
  width: number
  height: number
  /** Logical outer top-left. */
  x: number
  y: number
  min: { width: number; height: number }
  maximized: boolean
  fullscreen: boolean
}

export interface FakeOptions {
  scaleFactor?: number
  width?: number
  height?: number
  origin?: { x: number; y: number }
  /** Physical pixels: outer minus inner (window frame + title bar). */
  chrome?: { width: number; height: number }
  monitor?: MonitorLike | null
  setSizeFailures?: number
  /** A failing `setSize` still resized the window (partial native mutation). */
  setSizeAppliesAnyway?: boolean
  /** The window becomes maximized exactly when a `setSize` throws: the retry has
   *  to notice that the system owns the size now and must not re-send normal
   *  dimensions. */
  maximizeOnSetSizeFailure?: boolean
  setPositionFailures?: number
  setPositionAppliesAnyway?: boolean
  positionClampX?: number
  setSizeApplyFraction?: number
  autoResize?: boolean
  autoResizeOnFailure?: boolean
  deferListeners?: boolean
  defer?: boolean
  maximized?: boolean
  fullscreen?: boolean
}

export interface FakeWindow extends SidebarWindowLike {
  state: FakeState
  calls: FakeCall[]
  requests: FakeCall[]
  listenerCount: () => number
  resolveListeners: () => void
  failNextSetSize: (times: number) => void
  failNextInnerSize: (times: number) => void
  /** Deferred mode: run the parked native mutations. */
  flush: () => void
  /** How many native mutations are parked, and which ones — the barrier an
   *  "inflight growth" test needs instead of guessed tick counts. */
  pendingCount: () => number
  pendingNames: () => string[]
  fireResize: () => void
  unlistenCount: () => number
  /** Arm the next N `setMinSize` writes to throw. A stale enlarged min silently
   *  clamps the `setSize` that follows, which is how the real window behaves — and
   *  it has to be armable mid-test, because the failure that matters is the one
   *  on the CLOSE pass after a clean open. */
  failNextSetMinSize: (times: number) => void
}

export function createFakeWindow(options: FakeOptions = {}): FakeWindow {
  const factor = options.scaleFactor ?? 2
  const chrome = options.chrome ?? { width: 0, height: 30 }
  const quantize = (value: number) => round(value * factor) / factor
  const state: FakeState = {
    width: quantize(options.width ?? 1060),
    height: quantize(options.height ?? 778),
    x: quantize(options.origin?.x ?? 100),
    y: quantize(options.origin?.y ?? 50),
    min: { width: BASE_MIN_WIDTH, height: BASE_MIN_HEIGHT },
    maximized: options.maximized ?? false,
    fullscreen: options.fullscreen ?? false,
  }
  const calls: FakeCall[] = []
  const requests: FakeCall[] = []
  const parked: Array<{ name: string; run: () => void }> = []
  let setSizeFailures = options.setSizeFailures ?? 0
  let setMinSizeFailures = 0
  let innerSizeFailures = 0
  let setPositionFailures = options.setPositionFailures ?? 0
  const pending: Array<() => void> = []
  const pendingListeners: Array<() => void> = []
  const handlers: Array<() => void> = []
  let unlistens = 0
  const fireResize = () => { for (const handler of [...handlers]) handler() }

  const write = (call: FakeCall, apply: () => void) => {
    requests.push(call)
    const execute = () => {
      try {
        apply()
        if (options.autoResize) fireResize()
      } catch (cause) {
        if (options.autoResizeOnFailure) fireResize()
        throw cause
      }
    }
    return options.defer
      ? new Promise<void>((resolve, reject) => {
        const run = () => { try { execute(); resolve() } catch (cause) { reject(cause) } }
        parked.push({ name: call.name, run })
        pending.push(run)
      })
      : Promise.resolve().then(execute)
  }
  /** Geometry reads answer immediately, exactly like a real window whose resize
   *  is still in flight; parking them would stall `reconcile` before it ever
   *  dispatched a mutation, so an "inflight" test would test nothing. */
  const read = <T>(value: () => T): Promise<T> => Promise.resolve().then(value)

  return {
    state,
    calls,
    requests,
    listenerCount: () => handlers.length,
    resolveListeners: () => { for (const resolve of pendingListeners.splice(0)) resolve() },
    failNextSetSize: (times) => { setSizeFailures = Math.max(0, Math.trunc(times)) },
    failNextInnerSize: (times) => { innerSizeFailures = Math.max(0, Math.trunc(times)) },
    flush: () => {
      const items = pending.splice(0, pending.length)
      parked.length = 0
      for (const item of items) item()
    },
    pendingCount: () => pending.length,
    pendingNames: () => parked.map((call) => call.name),
    failNextSetMinSize: (times) => { setMinSizeFailures = Math.max(0, Math.trunc(times)) },
    fireResize,
    unlistenCount: () => unlistens,
    innerSize: () => read(() => {
      if (innerSizeFailures > 0) { innerSizeFailures -= 1; throw new Error('native innerSize failed') }
      return { width: round(state.width * factor), height: round(state.height * factor) }
    }),
    outerSize: () => read(() => ({ width: round(state.width * factor) + chrome.width, height: round(state.height * factor) + chrome.height })),
    outerPosition: () => read(() => ({ x: round(state.x * factor), y: round(state.y * factor) })),
    scaleFactor: () => Promise.resolve(factor),
    setSize: (size) => write({ name: 'setSize', width: size.width, height: size.height }, () => {
      calls.push({ name: 'setSize', width: size.width, height: size.height })
      if (setSizeFailures > 0) {
        setSizeFailures -= 1
        if (options.maximizeOnSetSizeFailure) {
          // The user hit maximize in the instant the failed resize was in flight:
          // from here on the system, not this controller, owns the width.
          state.maximized = true
        }
        if (options.setSizeAppliesAnyway || options.setSizeApplyFraction !== undefined) {
          const fraction = options.setSizeApplyFraction ?? 1
          state.width = quantize(Math.max(state.width + (size.width - state.width) * fraction, state.min.width))
          state.height = quantize(Math.max(size.height, state.min.height))
        }
        throw new Error('native resize failed')
      }
      state.width = quantize(Math.max(size.width, state.min.width))
      state.height = quantize(Math.max(size.height, state.min.height))
    }),
    setMinSize: (size) => write({ name: 'setMinSize', width: size?.width, height: size?.height }, () => {
      calls.push({ name: 'setMinSize', width: size?.width, height: size?.height })
      if (setMinSizeFailures > 0) {
        setMinSizeFailures -= 1
        // The min write failed: the previously committed min stays in force and
        // keeps clamping every later resize.
        throw new Error('native setMinSize failed')
      }
      state.min = size ? { width: size.width, height: size.height } : { width: BASE_MIN_WIDTH, height: BASE_MIN_HEIGHT }
    }),
    setPosition: (position) => write({ name: 'setPosition', x: position.x, y: position.y }, () => {
      calls.push({ name: 'setPosition', x: position.x, y: position.y })
      const applyPosition = () => {
        state.x = quantize(Math.max(position.x, options.positionClampX ?? -Infinity))
        state.y = quantize(position.y)
      }
      if (setPositionFailures > 0) {
        setPositionFailures -= 1
        if (options.setPositionAppliesAnyway) applyPosition()
        throw new Error('native move failed')
      }
      applyPosition()
    }),
    isMaximized: () => Promise.resolve(state.maximized),
    isFullscreen: () => Promise.resolve(state.fullscreen),
    onResized: (handler) => {
      handlers.push(handler)
      const unlisten = () => {
        unlistens += 1
        const index = handlers.indexOf(handler)
        if (index >= 0) handlers.splice(index, 1)
      }
      return options.deferListeners
        ? new Promise<() => void>((resolve) => { pendingListeners.push(() => resolve(unlisten)) })
        : Promise.resolve(unlisten)
    },
  }
}

export type FakeMonitor = MonitorLike & { size: { width: number; height: number }; position: { x: number; y: number }; scaleFactor: number }

/** 3840×2160 physical (logical 1920×1080 at 2x) with a 96 physical taskbar:
 *  `workArea` = 3840×2064, i.e. the monitor is NOT the clamp — the work area is. */
export function createMonitor(options: { x?: number; y?: number; width?: number; height?: number; taskbar?: number } = {}): FakeMonitor {
  const x = options.x ?? 0
  const y = options.y ?? 0
  const width = options.width ?? 3840
  const height = options.height ?? 2160
  const taskbar = options.taskbar ?? 96
  return {
    name: 'MOCK',
    size: { width, height },
    position: { x, y },
    workArea: { position: { x, y }, size: { width, height: height - taskbar } },
    scaleFactor: 1,
  } as FakeMonitor
}

export interface Harness {
  win: FakeWindow
  logs: string[]
  refused: SidebarGrowthEntry[][]
  growth: ReturnType<typeof createSidebarWindowGrowth>
  monitor: MonitorLike | null
  deps: SidebarWindowGrowthDeps
}

export function createHarness(options: FakeOptions = {}): Harness {
  const win = createFakeWindow(options)
  const logs: string[] = []
  const refused: SidebarGrowthEntry[][] = []
  const monitor = options.monitor === undefined ? createMonitor() : options.monitor
  const deps: SidebarWindowGrowthDeps = {
    getWindow: () => win,
    getMonitor: async () => monitor,
    log: (message) => logs.push(message),
    onGrowthRefused: (entries) => { refused.push(entries) },
  }
  return { win, logs, refused, monitor, deps, growth: createSidebarWindowGrowth(deps) }
}

/** Drain the native-call promise queue. */
export const settle = async () => {
  for (let i = 0; i < 80; i++) await new Promise((resolve) => setTimeout(resolve, 0))
}

/** Deferred mode: run parked native calls, then drain the queue — repeatedly. */
export async function drain(win: FakeWindow, growth: ReturnType<typeof createSidebarWindowGrowth>, entries: SidebarGrowthEntry[] | null = null) {
  if (entries) growth.setSidebars(entries)
  for (let i = 0; i < 60; i++) {
    win.flush()
    await new Promise((resolve) => setTimeout(resolve, 0))
  }
}

export const sizeCalls = (win: FakeWindow) => win.calls.filter((call) => call.name === 'setSize')
export const minCalls = (win: FakeWindow) => win.calls.filter((call) => call.name === 'setMinSize')
export const positionCalls = (win: FakeWindow) => win.calls.filter((call) => call.name === 'setPosition')
