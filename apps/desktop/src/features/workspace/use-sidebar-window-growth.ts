import { useEffect, useLayoutEffect, useRef, useSyncExternalStore, type RefObject } from 'react'
import { LogicalPosition, LogicalSize, currentMonitor, getCurrentWindow } from '@tauri-apps/api/window'

/**
 * Shared sidebar-window-growth controller (SPEC item 4).
 *
 * Side cards (the ordinary right panel, the private-chat contact card) are
 * siblings of the content card. Opening one grows the native window to the
 * right so the chat keeps its width; closing shrinks it back. One controller
 * owns the applied delta for all declared cards, so a card is never counted
 * twice and every native call is serialized through one queue.
 *
 * Rules implemented here:
 * - grow rightwards; if the monitor WORK AREA (native `Monitor.workArea`,
 *   taskbar excluded) would be exceeded, first shift the window left inside
 *   the work area, then reduce the actually applied delta (and log) if there is
 *   still not enough room — never cover the taskbar. Clamping is NOT a failure:
 *   SPEC §4 keeps the sidebar at its target width and lets the chat narrow, so
 *   zero or partial room never reports back to the DOM and never closes a card;
 * - all geometry used for clamping is compared in PHYSICAL pixels (monitor
 *   work area, `outerPosition`, `outerSize`), so mixed-DPI multi-monitor
 *   setups and non-zero monitor origins stay consistent; the window chrome is
 *   included because the clamp uses outer geometry; the resulting numbers are
 *   converted to logical pixels with the WINDOW scale factor for the calls
 *   that expect them (`setSize` / `setPosition` / `setMinSize`);
 * - min width grows by the ACTUAL applied delta (base 600x560 from
 *   tauri.conf.json); closing shrinks the CURRENT width by the owned delta,
 *   keeps the current height and restores the min;
 * - maximized / fullscreen never `setSize`, never `setMinSize` growth and
 *   never unmaximize: the owned delta is cleared and the cards simply narrow
 *   the chat. Restoring re-derives the owned delta from the width the window
 *   had with the cards closed, so a system restore to the ALREADY grown width
 *   does not grow a second time. The flag is re-read at the boundary of EVERY
 *   native write, because maximizing can land while this pass awaits geometry;
 * - failed native calls are logged, retried once where that is safe (and the
 *   `setSize` retry re-checks system ownership first), and the owned delta is
 *   always re-MEASURED from the window afterwards: a fulfilled `setSize` promise
 *   does not guarantee the requested geometry, because a min reset that failed
 *   leaves the old enlarged minimum clamping it. Ownership therefore follows the
 *   width that actually landed, so a later close or `dispose()` can still undo
 *   it; only a real native mutation failure reports back, which is the single
 *   case where the DOM rolls the card back;
 * - a refusal names the cards that asked for room the accepted declaration did not
 *   already carry, each with the width that declaration gave them
 *   (`SidebarGrowthRefusal`), so the DOM rolls back the responsible card only —
 *   never a sibling that was already accommodated, whichever order the cards were
 *   opened in. A superseded pass whose ALREADY-DISPATCHED write still lands
 *   SUCCESSFULLY retains each continuously present card's latest successful width
 *   as rollback history, even if its desired width changed (clamping is not failure);
 *   rejected writes own measured geometry but never establish that history;
 * - non-Tauri (browser preview) is DOM-only: no native calls at all.
 *
 * Known limit: `BASE_MIN_WIDTH` / `BASE_MIN_HEIGHT` mirror the PRODUCTION
 * `tauri.conf.json` floor (600x560). The separate kernel-preview acceptance
 * config ships 1200x800 with a 760 min width, and the controller will still only
 * ever raise min above its 600 base, so a sidebar cycle in a preview build can
 * lower that configured min back to 600. Reading arbitrary native window config
 * is not part of this controller (no such getter is in the installed API surface,
 * and SPEC §4 asks for no new capability); the preview geometry stays as
 * configured and only this min-floor difference is accepted, documented here.
 *
 * Lifecycle: one controller per native window (WeakMap registry, refcounted),
 * so a StrictMode remount — or any second consumer of the same window — reuses
 * the very same delta accounting and the very same `onResized` listener instead
 * of stacking a second controller onto one window. The dispose that the last
 * consumer's unmount requests is deferred to a microtask and cancelled if a
 * mount lands first; detaching is idempotent and an `onResized` that resolves
 * late unregisters itself.
 */

export const SIDEBAR_CARD_GAP = 5
/** Keep in sync with apps/desktop/src-tauri/tauri.conf.json. The separate
 *  `tauri.kernel-preview.conf.json` acceptance build keeps its own 1200×800 /
 *  min 760 geometry; the controller only ever raises min above this base. */
export const BASE_MIN_WIDTH = 600
export const BASE_MIN_HEIGHT = 560

export interface SidebarGrowthEntry { id: string; width: number }

/** One card held responsible for a refused growth, paired with the width the
 *  ACCEPTED declaration carried for it.
 *
 *  `onGrowthRefused` reports the CARDS THAT ASKED FOR NEW ROOM and nothing else:
 *  the declaration is a set of siblings, so "the last entry" is a fixed render
 *  order, not the requester. A card the accepted declaration already carries (same
 *  id, same or smaller width) is not part of the failure, and an already-open card
 *  that only grew wider is rolled back to `accepted` rather than closed. */
export interface SidebarGrowthRefusal {
  id: string
  /** The width this pass asked the native layer to make room for. */
  width: number
  /** The width the accepted declaration gave this card, or `null` when the card
   *  was not open in it — the case that has to close. */
  accepted: number | null
}

export interface SizeLike { width: number; height: number }
export interface PointLike { x: number; y: number }

/** The native monitor record, narrowed to what the clamp needs. Both the
 *  position and the size of `workArea` are PHYSICAL pixels (taskbar excluded). */
export interface MonitorLike {
  workArea: { position: PointLike; size: SizeLike }
}

/** The slice of the Tauri `Window` API this controller uses, in the units the
 *  real API documents: sizes/positions are physical, `setSize`/`setPosition`/
 *  `setMinSize` receive logical values. Fakeable in tests. */
export interface SidebarWindowLike {
  innerSize(): Promise<SizeLike>
  outerSize(): Promise<SizeLike>
  outerPosition(): Promise<PointLike>
  scaleFactor(): Promise<number>
  setSize(size: SizeLike): Promise<void>
  setMinSize(size: SizeLike | null): Promise<void>
  setPosition(position: PointLike): Promise<void>
  isMaximized(): Promise<boolean>
  isFullscreen(): Promise<boolean>
  onResized(handler: () => void): Promise<() => void>
}

export interface SidebarWindowGrowthDeps {
  getWindow: () => SidebarWindowLike | null
  /** `currentMonitor()` — an exported function of the window module, not a method. */
  getMonitor: () => Promise<MonitorLike | null>
  log?: (message: string) => void
  /** Called when a growth could not be applied, so the UI can roll the DOM back.
   *  Receives ONLY the cards that asked for new room (see `SidebarGrowthRefusal`),
   *  never a sibling the window already reflects. */
  onGrowthRefused?: (refusals: SidebarGrowthRefusal[]) => void
  loadGrowth?: () => { width: number; delta: number } | null
  saveGrowth?: (growth: { width: number; delta: number } | null) => void
}

export interface SidebarWindowGrowth {
  setSidebars: (entries: SidebarGrowthEntry[]) => Promise<void>
  getAppliedDelta: () => number
  canResize: () => boolean
  subscribe: (listener: () => void) => () => void
  dispose: () => void
}

function sameEntries(a: SidebarGrowthEntry[], b: SidebarGrowthEntry[]) {
  return a.length === b.length && a.every((entry, index) => entry.id === b[index].id && entry.width === b[index].width)
}

/** The cards of `entries` that asked for room `accepted` did not already cover,
 *  each paired with the width the window really reflects for it.
 *
 *  Narrowed and removed cards are never reported — they asked for LESS, so they
 *  cannot be responsible for a growth the native layer refused — and the answer
 *  is keyed by id, so the declaration order cannot decide who gets rolled back. */
function refusalsFor(accepted: readonly SidebarGrowthEntry[], entries: readonly SidebarGrowthEntry[]): SidebarGrowthRefusal[] {
  const baseline = new Map(accepted.map((entry) => [entry.id, entry.width]))
  return entries
    .filter((entry) => entry.width > (baseline.get(entry.id) ?? 0))
    .map((entry) => ({ id: entry.id, width: entry.width, accepted: baseline.get(entry.id) ?? null }))
}

function describe(cause: unknown) {
  return cause instanceof Error ? cause.message : String(cause)
}

/** A Tauri `UnlistenFn` may return a promise; failing here only means the
 *  listener is already gone, which is the state the caller wanted. */
function detach(cleanup: (() => void) | null) {
  if (!cleanup) return
  try {
    const result = cleanup() as unknown
    if (result && typeof (result as Promise<void>).catch === 'function') void (result as Promise<void>).catch(() => undefined)
  } catch { /* already removed */ }
}

export function createSidebarWindowGrowth(deps: SidebarWindowGrowthDeps): SidebarWindowGrowth {
  type Mode = 'normal' | 'system' | 'unknown'
  type Card = Readonly<SidebarGrowthEntry & { lifetime: symbol }>
  type Failure = Readonly<{ cause: unknown }>
  type Outcome = { status: 'pending' } | { status: 'fulfilled' } | { status: 'rejected'; failure: Failure }
  type Operation = { generation: number; entries: readonly Card[]; wanted: number; deferred: boolean; minimums: Set<number>; fromResize: boolean; geometry: Geometry | null }
  type Geometry = { inner: SizeLike; factor: number; width: number; height: number; outer: SizeLike | null; position: PointLike | null; monitor: MonitorLike | null }
  type PendingResize = Readonly<{ width: number; owned: number; base: number; generation: number; entries: readonly Card[]; outcome: Readonly<Outcome> }>

  const log = deps.log ?? (() => undefined)
  let desired: Card[] = []
  // Rollback history is the latest success per continuous card lifetime, not its maximum width or native allocation.
  let accepted: Card[] = []
  const rejectedAccommodation = new Map<symbol, Failure>()
  let declarationGeneration = 0
  let appliedDelta = 0
  let savedGrowth = deps.loadGrowth?.() ?? null
  let closedBase: number | null = null
  let suppressed = false
  let mode: Mode = 'unknown'
  let pendingResize: PendingResize | null = null
  let pendingMovement = false
  let lastMinimum = BASE_MIN_WIDTH
  let minimumUncertain = false
  let minimumRevision = 0
  let minimumEventFailure: string | null = null
  let blockedGeometry: { key: string; failed: boolean } | null = null
  let active = true
  let reconciling = false
  let queue: Promise<void> = Promise.resolve()
  let unlisten: (() => void) | null = null
  let listening = false
  let attachGeneration = 0
  const listeners = new Set<() => void>()

  const current = (operation: Operation) => operation.generation === declarationGeneration
  const present = (card: Card) => desired.some((entry) => entry.lifetime === card.lifetime)
  function accommodate(entries: readonly Card[], successfulWrite = false): Failure | null {
    const baseline = new Map(accepted.map((entry) => [entry.lifetime, entry]))
    let failure: Failure | null = null
    for (const entry of entries) {
      if (!present(entry)) continue
      const rejected = rejectedAccommodation.get(entry.lifetime)
      // Matching geometry after a rejection is not independent evidence of accommodation.
      if (!successfulWrite && rejected && entry.width !== baseline.get(entry.lifetime)?.width) {
        failure = rejected
        continue
      }
      baseline.set(entry.lifetime, entry)
      rejectedAccommodation.delete(entry.lifetime)
    }
    accepted = [...baseline.values()]
    return failure
  }
  const setOwned = (owned: number) => {
    if (owned === appliedDelta) return
    appliedDelta = owned
    listeners.forEach((listener) => listener())
  }
  const enqueue = (operation: () => Promise<void>) => {
    queue = queue.then(() => operation().catch((cause) => {
      log(`[sidebar-growth] operation failed: ${describe(cause)}`)
    }))
    return queue
  }
  async function read<T>(name: string, getter: () => Promise<T>): Promise<T | null> {
    try { return await getter() } catch (cause) { log(`[sidebar-growth] ${name} read failed: ${describe(cause)}`); return null }
  }
  async function readMode(appWindow: SidebarWindowLike): Promise<Mode> {
    const [maximized, fullscreen] = await Promise.all([
      read('maximize', () => appWindow.isMaximized()),
      read('fullscreen', () => appWindow.isFullscreen()),
    ])
    mode = maximized === true || fullscreen === true ? 'system'
      : maximized === null || fullscreen === null ? 'unknown' : 'normal'
    return mode
  }
  function observeMode(mode: Mode) {
    if (mode !== 'system') return
    suppressed = true
    pendingResize = null
    rejectedAccommodation.clear()
    pendingMovement = false
    blockedGeometry = null
    setOwned(0)
  }
  async function normal(appWindow: SidebarWindowLike, operation: Operation, settleDispatched = false) {
    if ((!settleDispatched && !current(operation)) || operation.deferred) return false
    const mode = await readMode(appWindow)
    observeMode(mode)
    if (mode === 'unknown') operation.deferred = true
    return mode === 'normal' && (settleDispatched || current(operation))
  }
  async function readGeometry(appWindow: SidebarWindowLike, operation: Operation): Promise<Geometry | null> {
    const monitor = await read('currentMonitor', deps.getMonitor)
    const [inner, factor, outer, position] = await Promise.all([
      read('size', () => appWindow.innerSize()),
      read('scale', () => appWindow.scaleFactor()),
      read('outer size', () => appWindow.outerSize()),
      read('position', () => appWindow.outerPosition()),
    ])
    if (!inner || factor === null || !(factor > 0)) return null
    const geometry = { inner, factor, width: inner.width / factor, height: inner.height / factor, outer, position, monitor }
    operation.geometry = geometry
    return geometry
  }
  const hasBounds = (geometry: Geometry) => geometry.outer !== null && geometry.position !== null && geometry.monitor !== null
  const destination = (geometry: Geometry, wanted: number) => geometry.width + wanted - appliedDelta
  const physicalDestination = (geometry: Geometry, wanted: number) => Math.round(destination(geometry, wanted) * geometry.factor)
  const geometryKey = (geometry: Geometry, operation: Operation) => JSON.stringify([
    operation.generation, operation.wanted, appliedDelta, minimumRevision,
    geometry.inner, geometry.factor, geometry.outer, geometry.position, geometry.monitor?.workArea,
  ])

  async function syncMinimum(appWindow: SidebarWindowLike, operation: Operation, width: number): Promise<boolean> {
    if (!minimumUncertain && lastMinimum === width) return true
    const failureKey = () => JSON.stringify([width, operation.generation, minimumRevision, suppressed, operation.geometry && geometryKey(operation.geometry, operation)])
    // Late native failure events get one recovery pass, not an unbounded write/event cycle.
    if (operation.fromResize && minimumEventFailure === failureKey()) return false
    if (operation.minimums.has(width)) return false
    operation.minimums.add(width)
    let failure: unknown
    for (let attempt = 0; attempt < 2; attempt++) {
      if (!current(operation) || operation.deferred) return false
      const mode = await readMode(appWindow)
      observeMode(mode)
      if (mode === 'unknown') operation.deferred = true
      if (!current(operation) || mode === 'unknown' || (mode === 'system' && width !== BASE_MIN_WIDTH)) return false
      if (attempt > 0) log(`[sidebar-growth] setMinSize failed (${describe(failure)}); retrying once`)
      minimumUncertain = true
      try {
        await appWindow.setMinSize(new LogicalSize(width, BASE_MIN_HEIGHT))
        lastMinimum = width
        minimumUncertain = false
        minimumEventFailure = null
        minimumRevision += 1
        const landedMode = await readMode(appWindow)
        observeMode(landedMode)
        if (landedMode === 'unknown') operation.deferred = true
        return current(operation) && !operation.deferred && (landedMode === 'normal' || width === BASE_MIN_WIDTH)
      } catch (cause) {
        failure = cause
        log(`[sidebar-growth] setMinSize failed: ${describe(cause)}`)
      }
    }
    if (operation.fromResize) minimumEventFailure = failureKey()
    log(`[sidebar-growth] setMinSize still failing after retry: ${describe(failure)}`)
    return false
  }

  async function resolveResize(appWindow: SidebarWindowLike, operation: Operation): Promise<Geometry | null> {
    if (!(await normal(appWindow, operation, true))) return null
    const geometry = await readGeometry(appWindow, operation)
    if (!(await normal(appWindow, operation, true))) return null
    if (!geometry) {
      log('[sidebar-growth] width not measurable after resize; retaining pending ownership')
      return null
    }
    const pending = pendingResize
    if (pending) {
      pendingResize = null
      closedBase = pending.base
      setOwned(Math.max(0, pending.owned + geometry.width - pending.width))
      if (pending.outcome.status === 'fulfilled') accommodate(pending.entries, true)
      else if (pending.outcome.status === 'rejected') {
        for (const entry of pending.entries) {
          if (present(entry) && entry.width !== accepted.find((card) => card.lifetime === entry.lifetime)?.width) {
            rejectedAccommodation.set(entry.lifetime, pending.outcome.failure)
          }
        }
      }
    }
    return geometry
  }

  async function reconcile(force = false, fromResize = false) {
    if (!active && !force) return
    const appWindow = deps.getWindow()
    if (!appWindow) return
    if (active) ensureResizeListener(appWindow)
    const operation: Operation = {
      generation: declarationGeneration,
      entries: desired.slice(),
      wanted: desired.reduce((sum, entry) => sum + entry.width + SIDEBAR_CARD_GAP, 0),
      deferred: false,
      minimums: new Set(),
      fromResize,
      geometry: null,
    }
    const settledMinimum = () => BASE_MIN_WIDTH + (operation.entries.length === 0 ? 0 : appliedDelta)
    const refuse = (cause: unknown, growing: boolean) => {
      if (!current(operation) || operation.deferred || suppressed) return
      log(`[sidebar-growth] ${growing ? 'growth' : 'shrink'} refused after retry (${describe(cause)}); keeping the measured size`)
      // Only the cards that asked for NEW room are rolled back; a sibling the window
      // already carries keeps its DOM state, whichever order it was declared in.
      if (growing) deps.onGrowthRefused?.(refusalsFor(accepted, operation.entries))
    }
    reconciling = true
    try {
      if (!(await normal(appWindow, operation))) return
      let geometry = pendingResize ? await resolveResize(appWindow, operation) : await readGeometry(appWindow, operation)
      if (!(await normal(appWindow, operation)) || !geometry) return
      if (savedGrowth) {
        if (Math.abs(geometry.width - savedGrowth.width) < 2) {
          setOwned(savedGrowth.delta)
          minimumUncertain = true
        }
        savedGrowth = null
      }
      if (pendingMovement && !hasBounds(geometry)) return
      pendingMovement = false
      if (suppressed) {
        suppressed = false
        if (closedBase !== null) {
          const representableOwned = Math.max(0, Math.round((closedBase + operation.wanted) * geometry.factor) / geometry.factor - closedBase)
          setOwned(Math.min(Math.max(0, geometry.width - closedBase), representableOwned))
        }
      }
      closedBase = geometry.width - appliedDelta
      if (blockedGeometry?.key === geometryKey(geometry, operation)) {
        await syncMinimum(appWindow, operation, settledMinimum())
        if (blockedGeometry?.key === geometryKey(geometry, operation)) return
      }
      const shrinking = physicalDestination(geometry, operation.wanted) < geometry.inner.width
      const minReset = await syncMinimum(appWindow, operation, shrinking ? BASE_MIN_WIDTH + operation.wanted : settledMinimum())
      if (!(await normal(appWindow, operation))) return
      geometry = await readGeometry(appWindow, operation)
      if (!(await normal(appWindow, operation)) || !geometry) return
      closedBase = geometry.width - appliedDelta
      if (blockedGeometry?.key === geometryKey(geometry, operation)) return

      if (physicalDestination(geometry, operation.wanted) > geometry.inner.width) {
        const { outer, position, monitor, factor } = geometry
        if (outer && position && monitor) {
          const right = monitor.workArea.position.x + monitor.workArea.size.width
          const overflow = position.x + outer.width + physicalDestination(geometry, operation.wanted) - geometry.inner.width - right
          const nextX = Math.ceil(Math.max(monitor.workArea.position.x, position.x - Math.max(0, overflow)))
          if (nextX < position.x) {
            if (!(await normal(appWindow, operation))) return
            pendingMovement = true
            try {
              await appWindow.setPosition(new LogicalPosition(nextX / factor, position.y / factor))
            } catch (cause) {
              log(`[sidebar-growth] setPosition failed: ${describe(cause)}`)
            }
            geometry = await readGeometry(appWindow, operation)
            if (!(await normal(appWindow, operation)) || !geometry || !hasBounds(geometry)) return
            pendingMovement = false
          }
        } else if (!monitor) {
          log('[sidebar-growth] no current monitor: growing without the work-area clamp')
        }
      }

      let failure: Failure | null = null
      const growing = operation.wanted > appliedDelta
      for (let attempt = 0; attempt < 2; attempt++) {
        if (attempt > 0) {
          geometry = await readGeometry(appWindow, operation)
          if (!(await normal(appWindow, operation)) || !geometry) return
        }
        closedBase = geometry.width - appliedDelta
        let width = destination(geometry, operation.wanted)
        let physicalWidth = Math.round(width * geometry.factor)
        const { outer, position, monitor } = geometry
        if (physicalWidth > geometry.inner.width && outer && position && monitor) {
          const right = monitor.workArea.position.x + monitor.workArea.size.width
          const limit = Math.floor(geometry.inner.width + right - position.x - outer.width)
          if (physicalWidth > limit) {
            physicalWidth = Math.max(geometry.inner.width, limit)
            width = physicalWidth / geometry.factor
            log(`[sidebar-growth] work area too small: applying ${Math.round(width - geometry.width)}px of the requested ${Math.round(operation.wanted - appliedDelta)}px`)
          }
        }
        if (!(await normal(appWindow, operation))) return
        if (physicalWidth === geometry.inner.width) {
          if (failure) refuse(failure.cause, growing)
          else {
            failure = accommodate(operation.entries)
            if (failure) refuse(failure.cause, refusalsFor(accepted, operation.entries).length > 0)
          }
          if (physicalDestination(geometry, operation.wanted) > geometry.inner.width) {
            log('[sidebar-growth] no room left in the work area; the card keeps its width and narrows the chat')
          }
          await syncMinimum(appWindow, operation, settledMinimum())
          blockedGeometry = { key: geometryKey(geometry, operation), failed: failure !== null }
          return
        }
        if (attempt > 0) log(`[sidebar-growth] setSize failed (${describe(failure?.cause)}); retrying once`)
        const pending: PendingResize = {
          width: geometry.width, owned: appliedDelta, base: closedBase,
          generation: operation.generation, entries: operation.entries, outcome: { status: 'pending' },
        }
        pendingResize = pending
        failure = null
        try {
          await appWindow.setSize(new LogicalSize(width, geometry.height))
        } catch (cause) {
          failure = { cause }
          log(`[sidebar-growth] setSize failed: ${describe(cause)}`)
        }
        pendingResize = { ...pending, outcome: failure ? { status: 'rejected', failure } : { status: 'fulfilled' } }
        const landed = await resolveResize(appWindow, operation)
        if (!current(operation)) return
        if (!(await normal(appWindow, operation))) return
        if (!landed) {
          if (attempt === 1 && failure) refuse(failure.cause, growing)
          return
        }
        geometry = landed
        if (failure && attempt === 0) continue
        if (failure) refuse(failure.cause, growing)
        await syncMinimum(appWindow, operation, settledMinimum())
        if (!(await normal(appWindow, operation))) return
        if (physicalDestination(geometry, operation.wanted) !== geometry.inner.width) {
          blockedGeometry = { key: geometryKey(geometry, operation), failed: failure !== null }
          if (!growing) log(`[sidebar-growth] shrink clamped: still owning ${Math.round(appliedDelta - operation.wanted)}px (min reset ${minReset ? 'succeeded' : 'FAILED'})`)
        }
        return
      }
    } finally {
      if (current(operation) && (suppressed || operation.entries.length === 0)) await syncMinimum(appWindow, operation, BASE_MIN_WIDTH)
      if (current(operation) && operation.geometry && !suppressed) {
        deps.saveGrowth?.(appliedDelta > 0 ? { width: operation.geometry.width, delta: appliedDelta } : null)
      }
      reconciling = false
    }
  }

  function ensureResizeListener(appWindow: SidebarWindowLike) {
    if (listening) return
    listening = true
    const generation = attachGeneration
    void appWindow.onResized(() => {
      // In-flight native notifications are consumed by the guarded post-write reads, not retries.
      if (generation === attachGeneration && !reconciling) enqueue(() => reconcile(false, true))
    })
      .then((cleanup) => {
        if (generation !== attachGeneration) { detach(cleanup); return }
        unlisten = cleanup
      })
      .catch(() => { if (generation === attachGeneration) listening = false })
  }

  function stopListening() {
    if (!listening && unlisten === null) return
    attachGeneration += 1
    listening = false
    const cleanup = unlisten
    unlisten = null
    detach(cleanup)
  }

  return {
    setSidebars(entries) {
      if (blockedGeometry?.failed) blockedGeometry = null
      if (!active || !sameEntries(entries, desired)) {
        declarationGeneration += 1
        blockedGeometry = null
      }
      active = true
      const lifetimes = new Map(desired.map((entry) => [entry.id, entry.lifetime]))
      desired = entries.map((entry) => ({ ...entry, lifetime: lifetimes.get(entry.id) ?? Symbol(entry.id) }))
      accepted = accepted.filter(present)
      const presentLifetimes = new Set(desired.map((entry) => entry.lifetime))
      for (const lifetime of rejectedAccommodation.keys()) {
        if (!presentLifetimes.has(lifetime)) rejectedAccommodation.delete(lifetime)
      }
      return enqueue(() => reconcile())
    },
    getAppliedDelta: () => appliedDelta,
    canResize: () => mode === 'normal',
    subscribe(listener) {
      listeners.add(listener)
      return () => { listeners.delete(listener) }
    },
    dispose() {
      if (blockedGeometry?.failed) blockedGeometry = null
      if (active) declarationGeneration += 1
      active = false
      desired = []
      accepted = []
      rejectedAccommodation.clear()
      stopListening()
      enqueue(() => reconcile(true))
    },
  }
}

/** Bound to the real `@tauri-apps/api` window object — no invented members. */
function createNativeWindow(): SidebarWindowLike | null {
  try {
    const appWindow = getCurrentWindow()
    return {
      innerSize: () => appWindow.innerSize(),
      outerSize: () => appWindow.outerSize(),
      outerPosition: () => appWindow.outerPosition(),
      scaleFactor: () => appWindow.scaleFactor(),
      setSize: (size) => appWindow.setSize(new LogicalSize(size.width, size.height)),
      setMinSize: (size) => appWindow.setMinSize(size ? new LogicalSize(size.width, size.height) : null),
      setPosition: (position) => appWindow.setPosition(new LogicalPosition(position.x, position.y)),
      isMaximized: () => appWindow.isMaximized(),
      isFullscreen: () => appWindow.isFullscreen(),
      onResized: (handler) => appWindow.onResized(handler),
    }
  } catch {
    return null // Browser preview: DOM-only.
  }
}

let nativeWindow: SidebarWindowLike | null | undefined
const NATIVE_DEPS: SidebarWindowGrowthDeps = {
  getWindow: () => {
    if (nativeWindow === undefined) nativeWindow = createNativeWindow()
    return nativeWindow
  },
  getMonitor: async () => await currentMonitor(),
  log: (message) => console.warn(message),
  loadGrowth: () => {
    try {
      const saved = JSON.parse(sessionStorage.getItem('fox.sidebar-window-growth') ?? 'null')
      return saved && Number.isFinite(saved.width) && Number.isFinite(saved.delta) && saved.delta > 0 && saved.delta < saved.width ? saved : null
    } catch { return null }
  },
  saveGrowth: (growth) => {
    try {
      if (growth) sessionStorage.setItem('fox.sidebar-window-growth', JSON.stringify(growth))
      else sessionStorage.removeItem('fox.sidebar-window-growth')
    } catch { /* Storage is unavailable in some webviews. */ }
  },
}

type RefusalHandler = (refusals: SidebarGrowthRefusal[]) => void

/** ONE controller per native window, shared by every hook instance mounted on it.
 *  The window has a single right edge, so two controllers driving the same window
 *  would fight over the delta; StrictMode's cleanup + remount therefore reuses the
 *  existing (possibly just-disposed, which `setSidebars` revives) controller. */
interface SharedGrowth {
  growth: SidebarWindowGrowth
  /** Latest refusal callback of the mounted instance, kept in a box so the
   *  controller never goes stale. */
  refused: { current: RefusalHandler | null }
  users: number
  disposeQueued: boolean
}

const sharedGrowths = new WeakMap<SidebarWindowLike, SharedGrowth>()

function createShared(appWindow: SidebarWindowLike, deps: SidebarWindowGrowthDeps): SharedGrowth {
  const refused: SharedGrowth['refused'] = { current: null }
  const shared: SharedGrowth = {
    refused,
    users: 0,
    disposeQueued: false,
    growth: createSidebarWindowGrowth({ ...deps, onGrowthRefused: (refusals) => { refused.current?.(refusals) } }),
  }
  sharedGrowths.set(appWindow, shared)
  return shared
}

/** `null` in browser preview, where there is no native window to key on. */
function acquireGrowth(deps: SidebarWindowGrowthDeps): SharedGrowth | null {
  const appWindow = deps.getWindow()
  if (!appWindow) return null
  const shared = sharedGrowths.get(appWindow) ?? createShared(appWindow, deps)
  shared.users += 1
  // A remount inside the same task beats the deferred dispose queued by the
  // matching unmount.
  shared.disposeQueued = false
  return shared
}

function releaseGrowth(shared: SharedGrowth | null) {
  if (!shared || shared.users === 0) return
  shared.users -= 1
  if (shared.users > 0 || shared.disposeQueued) return
  shared.disposeQueued = true
  // Deferred: StrictMode unmounts and remounts in one task, and a real dispose
  // there would shrink and re-grow the window (and double-unlisten) for nothing.
  queueMicrotask(() => {
    if (!shared.disposeQueued || shared.users > 0) return
    shared.disposeQueued = false
    shared.growth.dispose()
  })
}

/** Per-instance view of a controller: the store React subscribes to survives the
 *  controller being swapped (shared entry disposed and revived by a remount). */
interface GrowthBinding {
  growth: SidebarWindowGrowth | null
  forward: (() => void) | null
  listeners: Set<() => void>
  emit: () => void
  /** Stable identities, so React does not resubscribe on every render. */
  subscribe: (listener: () => void) => () => void
  getSnapshot: () => number
}

function createBinding(): GrowthBinding {
  const binding: GrowthBinding = {
    growth: null,
    forward: null,
    listeners: new Set<() => void>(),
    emit: () => { for (const listener of [...binding.listeners]) listener() },
    subscribe: (listener) => {
      binding.listeners.add(listener)
      return () => { binding.listeners.delete(listener) }
    },
    getSnapshot: () => binding.growth?.getAppliedDelta() ?? 0,
  }
  return binding
}

/** Points the binding at `growth`, (re)forwarding its notifications. Re-biding
 *  the SAME controller matters: StrictMode's cleanup drops the subscription but
 *  keeps the pointer, and an identity shortcut there would leave the hook
 *  permanently deaf — the applied delta would never reach React again. */
function bindGrowth(binding: GrowthBinding, growth: SidebarWindowGrowth | null) {
  if (binding.growth === growth && binding.forward) return
  unbindGrowth(binding)
  binding.growth = growth
  binding.forward = growth ? growth.subscribe(binding.emit) : null
  binding.emit()
}

/** Releases this instance's subscription into the (possibly longer-lived)
 *  controller, so unmounting never leaves a listener behind. The snapshot itself
 *  stays readable: the instance is going away, there is nothing to re-render. */
function unbindGrowth(binding: GrowthBinding) {
  binding.forward?.()
  binding.forward = null
}

/** Declares the currently open side cards; the controller reconciles the window.
 *  Returns the actually applied width delta (0 in browser preview or when
 *  maximized/fullscreen, where the cards narrow the chat instead).
 *  `deps` exists so behaviour tests can drive a fake native window; production
 *  uses the real Tauri API. */
export function useSidebarWindowGrowth(
  entries: SidebarGrowthEntry[],
  onGrowthRefused?: (refusals: SidebarGrowthRefusal[]) => void,
  layoutRef?: RefObject<HTMLElement | null>,
  deps: SidebarWindowGrowthDeps = NATIVE_DEPS,
) {
  const bindingRef = useRef<GrowthBinding | null>(null)
  if (bindingRef.current === null) bindingRef.current = createBinding()
  const binding = bindingRef.current
  const sharedRef = useRef<SharedGrowth | null>(null)
  const refusedRef = useRef<RefusalHandler | null>(null)
  const appliedDelta = useSyncExternalStore(binding.subscribe, binding.getSnapshot, binding.getSnapshot)

  useEffect(() => {
    refusedRef.current = onGrowthRefused ?? null
    if (sharedRef.current) sharedRef.current.refused.current = refusedRef.current
  })

  // Declared before the entries effect, so the controller exists when it runs.
  useLayoutEffect(() => {
    const shared = acquireGrowth(deps)
    sharedRef.current = shared
    if (shared) {
      shared.refused.current = refusedRef.current
      bindGrowth(binding, shared.growth)
      return () => {
        shared.refused.current = null
        sharedRef.current = null
        unbindGrowth(binding)
        releaseGrowth(shared)
      }
    }
    // Browser preview: a private, native-call-free controller.
    const refused = { current: null as RefusalHandler | null }
    const growth = createSidebarWindowGrowth({ ...deps, onGrowthRefused: (refusals) => { refused.current?.(refusals) } })
    refused.current = refusedRef.current
    bindGrowth(binding, growth)
    return () => {
      refused.current = null
      unbindGrowth(binding)
      growth.dispose()
    }
    // Mount-only: `deps` is captured like the controller used to be, and the
    // refcount must follow real mount / unmount, not callback identity.
  }, [])

  const layoutWidth = useRef<number | null>(null)
  const previousCardWidth = useRef(0)
  const key = JSON.stringify(entries)
  useLayoutEffect(() => {
    const declaration = JSON.parse(key) as SidebarGrowthEntry[]
    const wanted = declaration.reduce((sum, entry) => sum + entry.width + SIDEBAR_CARD_GAP, 0)
    const change = wanted - previousCardWidth.current
    previousCardWidth.current = wanted
    const container = layoutRef?.current
    const growth = binding.growth
    let cancelled = false

    // Keep the chat still between React's card commit and the native window resize.
    if (container && growth?.canResize() && (change !== 0 || layoutWidth.current !== null)) {
      layoutWidth.current = Math.max(0, (layoutWidth.current ?? container.getBoundingClientRect().width) + change)
      container.style.width = `${layoutWidth.current}px`
    }
    void growth?.setSidebars(declaration).then(() => {
      if (cancelled) return
      layoutWidth.current = null
      container?.style.removeProperty('width')
    })
    return () => {
      cancelled = true
      container?.style.removeProperty('width')
    }
  }, [binding, key, layoutRef])

  return appliedDelta
}
