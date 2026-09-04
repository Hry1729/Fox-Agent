/**
 * Performance telemetry for the profile build.
 *
 * Framework-agnostic aggregation of React <Profiler> commits, rAF frame gaps and
 * Long Tasks into bounded O(1) ring buffers, producing redacted JSON reports.
 * Only durations, region ids, counts and scenario metadata are stored — never
 * message text, file paths or user data.
 *
 * Metric semantics (do not conflate):
 * - React actualDuration is the React render duration for the commit subtree;
 *   NOT layout, paint or GC.
 * - Frame gaps are rAF intervals; "missed vsyncs" = round(gap/budget)-1. A gap
 *   can exceed 50ms; Long Tasks (>=50ms, W3C) are observed separately.
 * - Long Tasks / frames are GLOBAL; they are attributed to a scenario run by a
 *   time window (Long Tasks use entry.startTime, which is robust to a delayed
 *   PerformanceObserver callback), never claimed for a specific component.
 */

export type ScenarioId = 'history-mount' | 'stream-replay' | 'blocking' | 'real-workbench'
export type ScenarioKind =
  | 'generic' | 'markdown' | 'code-cold' | 'code-warm' | 'tools' | 'mixed' | 'blocking' | 'real'
export type ColdWarm = 'cold' | 'warm' | null
export type SettleStatus = 'settled' | 'timed-out' | 'not-awaited'

export interface RenderSample {
  runId: string
  region: string
  phase: 'mount' | 'update' | 'nested-update'
  actualDuration: number
  baseDuration: number
  startTime: number
  commitTime: number
}

export interface FrameSample {
  runId: string
  gap: number
  frameBudget: number
  missedVsyncs: number
  ts: number
}

interface RawLongTask {
  startTime: number
  duration: number
  assignedRunId: string | null
}

export interface PercentileResult {
  count: number
  p50: number
  p95: number
  max: number
  mean: number
}

export interface RegionRenderReport {
  mount: PercentileResult
  update: PercentileResult
  totalCommits: number
}

export interface ScenarioReport {
  scenarioRunId: string
  scenario: ScenarioId
  kind: ScenarioKind
  coldWarm: ColdWarm
  /** Short, non-sensitive scenario label (e.g. real-stream). Never user content. */
  label: string | null
  size: number | null
  messageCount: number | null
  eventCount: number | null
  startedAt: string
  endedAt: string
  durationMs: number
  renderByRegion: Record<string, RegionRenderReport>
  renderCommitCount: number
  frames: {
    count: number
    overBudgetFrames: number
    overBudgetRatio: number
    totalMissedVsyncs: number
    p50Gap: number
    p95Gap: number
    estimatedRefreshHz: number
  }
  longTasks: {
    supported: boolean
    supportedEntryTypes: string[]
    count: number
    totalMs: number
    p95: number
    max: number
  }
  settle: {
    status: SettleStatus
    waitedMs: number
  }
}

export interface TelemetryReport {
  schemaVersion: 2
  generatedAt: string
  build: {
    buildId: string
    mode: string
    profiling: boolean
  }
  environment: {
    userAgent: string
    viewport: { width: number; height: number }
    devicePixelRatio: number
  }
  thresholds: {
    longTaskMs: number
    provisionalFrameBudgetMs: number
    note: string
  }
  support: {
    longTask: boolean
    supportedEntryTypes: string[]
  }
  estimatedRefreshHz: number
  dropped: { renders: number; frames: number; longTasks: number; reports: number }
  scenarios: ScenarioReport[]
}

const RENDER_CAPACITY = 4000
const FRAME_CAPACITY = 8000
const LONGTASK_CAPACITY = 500
const REPORT_CAPACITY = 50

/** W3C Long Task threshold. */
export const LONG_TASK_MS = 50
/** Provisional investigation trigger only — NOT a hard CI gate (60Hz whole-frame budget). */
export const PROVISIONAL_FRAME_BUDGET_MS = 16.7
/** Gaps at/above this while HIDDEN are background artifacts; foreground freezes are kept. */
const BACKGROUND_GAP_MS = 1000
const COMMON_REFRESH_HZ = [60, 90, 120, 144, 165, 240]

export function percentile(sortedAscending: number[], p: number): number {
  if (sortedAscending.length === 0) return 0
  const index = Math.min(
    sortedAscending.length - 1,
    Math.max(0, Math.ceil((p / 100) * sortedAscending.length) - 1),
  )
  return sortedAscending[index]
}

export function summarize(durations: readonly number[]): PercentileResult {
  if (durations.length === 0) return { count: 0, p50: 0, p95: 0, max: 0, mean: 0 }
  const sorted = [...durations].sort((a, b) => a - b)
  const sum = sorted.reduce((total, value) => total + value, 0)
  return {
    count: sorted.length,
    p50: percentile(sorted, 50),
    p95: percentile(sorted, 95),
    max: sorted[sorted.length - 1],
    mean: sum / sorted.length,
  }
}

function emptyPercentile(): PercentileResult {
  return { count: 0, p50: 0, p95: 0, max: 0, mean: 0 }
}

/**
 * Bounded FIFO buffer with O(1) push (no Array.splice on overflow). Writes into
 * a fixed ring; oldest entries are overwritten once capacity is reached.
 */
export class RingBuffer<T> {
  private buffer: T[]
  private head = 0
  private count = 0

  constructor(private readonly capacity: number) {
    this.buffer = new Array<T>(capacity)
  }

  push(item: T): void {
    if (this.count < this.capacity) {
      this.buffer[(this.head + this.count) % this.capacity] = item
      this.count += 1
    } else {
      this.buffer[this.head] = item
      this.head = (this.head + 1) % this.capacity
    }
  }

  get size(): number {
    return this.count
  }

  toArray(): T[] {
    const out: T[] = []
    for (let i = 0; i < this.count; i += 1) {
      out.push(this.buffer[(this.head + i) % this.capacity])
    }
    return out
  }

  clear(): void {
    this.buffer = new Array<T>(this.capacity)
    this.head = 0
    this.count = 0
  }
}

function supportedPerformanceEntryTypes(): string[] {
  if (typeof PerformanceObserver === 'undefined' || !Array.isArray(PerformanceObserver.supportedEntryTypes)) {
    return []
  }
  return [...PerformanceObserver.supportedEntryTypes]
}

export class ProfilingCollector {
  private renders = new RingBuffer<RenderSample>(RENDER_CAPACITY)
  private frames = new RingBuffer<FrameSample>(FRAME_CAPACITY)
  private longTasks = new RingBuffer<RawLongTask>(LONGTASK_CAPACITY)
  private completedReports = new RingBuffer<ScenarioReport>(REPORT_CAPACITY)

  private activeRunId: string | null = null
  private activeScenario: ScenarioId | null = null
  private activeKind: ScenarioKind = 'generic'
  private activeColdWarm: ColdWarm = null
  private activeSize: number | null = null
  private activeMessageCount: number | null = null
  private activeEventCount: number | null = null
  private activeLabel: string | null = null
  private activeSettle: { status: SettleStatus; waitedMs: number } = { status: 'not-awaited', waitedMs: 0 }
  private activeStartPerf = 0
  private activeStartWall = ''
  private scenarioWindows: Array<{ runId: string; start: number; end: number | null }> = []

  private sampling = false
  private rafId = 0
  private lastFrameTs = 0
  private frameDeltaSamples: number[] = []
  private frameBudget = PROVISIONAL_FRAME_BUDGET_MS
  private observer: PerformanceObserver | null = null
  private readonly entryTypes = supportedPerformanceEntryTypes()
  private readonly longTaskSupported = this.entryTypes.includes('longtask')

  private droppedRenders = 0
  private droppedFrames = 0
  private droppedLongTasks = 0
  private droppedReports = 0
  private runCounter = 0

  private visibilityHandler = (): void => {
    // Reset the baseline on any visibility/focus change so the first rAF after
    // resuming does not register the backgrounded interval as a frozen frame.
    this.lastFrameTs = 0
  }

  /** React <Profiler onRender>. Records only while sampling with an active scenario. */
  readonly onRender = (
    id: string,
    phase: 'mount' | 'update' | 'nested-update',
    actualDuration: number,
    baseDuration: number,
    startTime: number,
    commitTime: number,
  ): void => {
    if (!this.sampling || this.activeRunId === null) return
    if (this.renders.size >= RENDER_CAPACITY) this.droppedRenders += 1
    this.renders.push({
      runId: this.activeRunId,
      region: id,
      phase,
      actualDuration,
      baseDuration,
      startTime,
      commitTime,
    })
  }

  beginScenario(
    scenario: ScenarioId,
    kind: ScenarioKind,
    options: {
      size?: number | null
      coldWarm?: ColdWarm
      messageCount?: number | null
      eventCount?: number | null
      label?: string | null
    } = {},
  ): string {
    this.runCounter += 1
    const runId = `run-${this.runCounter.toString(36)}-${Math.round(
      typeof performance !== 'undefined' ? performance.now() : 0,
    ).toString(36)}`
    this.activeRunId = runId
    this.activeScenario = scenario
    this.activeKind = kind
    this.activeColdWarm = options.coldWarm ?? null
    this.activeSize = options.size ?? null
    this.activeMessageCount = options.messageCount ?? null
    this.activeEventCount = options.eventCount ?? null
    // Non-sensitive free-form label only; callers must never put content in it.
    this.activeLabel = options.label ?? null
    this.activeStartPerf = typeof performance !== 'undefined' ? performance.now() : 0
    this.activeStartWall = new Date().toISOString()
    this.lastFrameTs = 0
    this.activeSettle = { status: 'not-awaited', waitedMs: 0 }
    this.scenarioWindows.push({ runId, start: this.activeStartPerf, end: null })
    return runId
  }

  /** Record how the workload's post-render settle wait resolved. */
  recordSettle(status: SettleStatus, waitedMs: number): void {
    this.activeSettle = { status, waitedMs }
  }

  /** Close the active scenario and finalize its report. */
  endScenario(): ScenarioReport {
    const endPerf = typeof performance !== 'undefined' ? performance.now() : 0
    const runId = this.activeRunId
    const startPerf = this.activeStartPerf

    // Close this run's window, then flush any Long Task entries the observer has
    // not delivered yet. ingestLongTask attributes by startTime against the
    // (now closed) window, so end-of-scenario tasks are not lost and late
    // callbacks still land on the correct run.
    const window = this.scenarioWindows.find((item) => item.runId === runId)
    if (window) window.end = endPerf
    this.flushPendingLongTasks()

    const report = this.buildScenarioReport(
      runId ?? 'unknown',
      this.activeScenario ?? 'history-mount',
      this.activeKind,
      this.activeColdWarm,
      this.activeLabel,
      this.activeSize,
      this.activeMessageCount,
      this.activeEventCount,
      this.activeSettle,
      this.activeStartWall,
      new Date().toISOString(),
      Math.max(0, endPerf - startPerf),
    )

    if (this.completedReports.size >= REPORT_CAPACITY) {
      this.droppedReports += 1
      // Prune the closed window belonging to the oldest evicted report so the
      // window array stays bounded alongside completedReports.
      const evicted = this.completedReports.toArray()[0]
      if (evicted) this.scenarioWindows = this.scenarioWindows.filter((item) => item.runId !== evicted.scenarioRunId)
    }
    this.completedReports.push(report)

    this.activeRunId = null
    this.activeScenario = null
    this.activeKind = 'generic'
    this.activeColdWarm = null
    this.activeSize = null
    this.activeMessageCount = null
    this.activeEventCount = null
    this.activeLabel = null
    return report
  }

  /**
   * Record a Long Task by (startTime, duration). Attributed at INGEST time to
   * the scenario whose time window contains startTime — this is robust to a
   * PerformanceObserver callback that arrives after endScenario (late delivery
   * still lands on the just-finished run). Testable without a PerformanceObserver.
   */
  ingestLongTask(startTime: number, duration: number): void {
    let assigned: string | null = null
    for (let i = this.scenarioWindows.length - 1; i >= 0; i -= 1) {
      const window = this.scenarioWindows[i]
      if (startTime >= window.start && (window.end === null || startTime <= window.end)) {
        assigned = window.runId
        break
      }
    }
    if (assigned === null) return
    if (this.longTasks.size >= LONGTASK_CAPACITY) this.droppedLongTasks += 1
    this.longTasks.push({ startTime, duration, assignedRunId: assigned })
  }

  startSampling(): void {
    if (this.sampling) return
    // Enable the recording gate first; rAF/observer/listeners attach only where
    // the environment supports them (keeps the collector unit-testable without
    // a browser while onRender still requires an active sampling session).
    this.sampling = true
    this.lastFrameTs = 0

    if (typeof window === 'undefined' || typeof requestAnimationFrame !== 'function') return

    const tick = (ts: number): void => {
      if (!this.sampling) return
      const visible = typeof document === 'undefined' || document.visibilityState === 'visible'
      if (visible && this.lastFrameTs > 0 && this.activeRunId !== null) {
        const gap = ts - this.lastFrameTs
        // Only treat as a vsync sample (for refresh estimation) when near a
        // normal interval; foreground freezes are still recorded as frames.
        if (gap > 0 && gap < 100) this.frameDeltaSamples.push(gap)
        if (this.frameDeltaSamples.length > 180) this.frameDeltaSamples.shift()
        this.frameBudget = this.estimateFrameBudget()
        const missedVsyncs = Math.max(0, Math.round(gap / this.frameBudget) - 1)
        if (this.frames.size >= FRAME_CAPACITY) this.droppedFrames += 1
        this.frames.push({
          runId: this.activeRunId,
          gap,
          frameBudget: this.frameBudget,
          missedVsyncs,
          ts,
        })
      }
      this.lastFrameTs = visible ? ts : 0
      this.rafId = requestAnimationFrame(tick)
    }
    this.rafId = requestAnimationFrame(tick)

    if (this.longTaskSupported) {
      try {
        this.observer = new PerformanceObserver((list) => {
          if (typeof document !== 'undefined' && document.visibilityState !== 'visible') return
          for (const entry of list.getEntries()) {
            if (entry.entryType !== 'longtask') continue
            this.ingestLongTask(entry.startTime, entry.duration)
          }
        })
        this.observer.observe({ entryTypes: ['longtask'] })
      } catch {
        this.observer = null
      }
    }

    document.addEventListener('visibilitychange', this.visibilityHandler)
    window.addEventListener('blur', this.visibilityHandler)
    window.addEventListener('focus', this.visibilityHandler)
  }

  stopSampling(): void {
    this.sampling = false
    if (this.rafId && typeof cancelAnimationFrame === 'function') cancelAnimationFrame(this.rafId)
    this.rafId = 0
    this.observer?.disconnect()
    this.observer = null
    if (typeof document !== 'undefined') document.removeEventListener('visibilitychange', this.visibilityHandler)
    if (typeof window !== 'undefined') {
      window.removeEventListener('blur', this.visibilityHandler)
      window.removeEventListener('focus', this.visibilityHandler)
    }
    this.lastFrameTs = 0
  }

  private flushPendingLongTasks(): void {
    if (!this.observer) return
    try {
      for (const entry of this.observer.takeRecords()) {
        if (entry.entryType !== 'longtask') continue
        if (typeof document !== 'undefined' && document.visibilityState !== 'visible') continue
        this.ingestLongTask(entry.startTime, entry.duration)
      }
    } catch {
      // takeRecords is best-effort.
    }
  }

  /** Snap the estimated vsync interval to common refresh rates (60..240Hz). */
  private estimateFrameBudget(): number {
    if (this.frameDeltaSamples.length < 5) return this.frameBudget
    const sorted = [...this.frameDeltaSamples].sort((a, b) => a - b)
    const median = percentile(sorted, 50)
    const estimatedHz = 1000 / median
    let best = 60
    let bestDistance = Number.POSITIVE_INFINITY
    for (const hz of COMMON_REFRESH_HZ) {
      const distance = Math.abs(Math.log(estimatedHz / hz))
      if (distance < bestDistance) {
        bestDistance = distance
        best = hz
      }
    }
    return 1000 / best
  }

  get estimatedRefreshHz(): number {
    return Math.round(1000 / this.frameBudget)
  }

  private buildScenarioReport(
    runId: string,
    scenario: ScenarioId,
    kind: ScenarioKind,
    coldWarm: ColdWarm,
    label: string | null,
    size: number | null,
    messageCount: number | null,
    eventCount: number | null,
    settle: { status: SettleStatus; waitedMs: number },
    startedAt: string,
    endedAt: string,
    durationMs: number,
  ): ScenarioReport {
    const renders = this.renders.toArray().filter((sample) => sample.runId === runId)
    const frames = this.frames.toArray().filter((sample) => sample.runId === runId)
    const tasks = this.longTasks.toArray().filter((entry) => entry.assignedRunId === runId)

    const regionMounts = new Map<string, number[]>()
    const regionUpdates = new Map<string, number[]>()
    const regions = new Set<string>()
    for (const sample of renders) {
      regions.add(sample.region)
      const bucket = sample.phase === 'mount' ? regionMounts : regionUpdates
      const list = bucket.get(sample.region) ?? []
      list.push(sample.actualDuration)
      bucket.set(sample.region, list)
    }
    const renderByRegion: Record<string, RegionRenderReport> = {}
    for (const region of regions) {
      const regionRenders = renders.filter((sample) => sample.region === region)
      renderByRegion[region] = {
        mount: summarize(regionMounts.get(region) ?? []),
        update: summarize(regionUpdates.get(region) ?? []),
        totalCommits: regionRenders.length,
      }
    }

    const gaps = frames.map((frame) => frame.gap)
    const overBudgetFrames = frames.filter((frame) => frame.missedVsyncs >= 1).length
    const totalMissedVsyncs = frames.reduce((total, frame) => total + frame.missedVsyncs, 0)
    const gapSummary = summarize(gaps)
    const taskDurations = tasks.map((task) => task.duration)

    return {
      scenarioRunId: runId,
      scenario,
      kind,
      coldWarm,
      label,
      size,
      messageCount,
      eventCount,
      startedAt,
      endedAt,
      durationMs,
      renderByRegion,
      renderCommitCount: renders.length,
      frames: {
        count: frames.length,
        overBudgetFrames,
        overBudgetRatio: frames.length ? overBudgetFrames / frames.length : 0,
        totalMissedVsyncs,
        p50Gap: gapSummary.p50,
        p95Gap: gapSummary.p95,
        estimatedRefreshHz: this.estimatedRefreshHz,
      },
      longTasks: {
        supported: this.longTaskSupported,
        supportedEntryTypes: this.entryTypes,
        count: tasks.length,
        totalMs: taskDurations.reduce((total, value) => total + value, 0),
        p95: summarize(taskDurations).p95,
        max: summarize(taskDurations).max,
      },
      settle,
    }
  }

  buildReport(): TelemetryReport {
    return {
      schemaVersion: 2,
      generatedAt: new Date().toISOString(),
      build: {
        buildId: typeof __FOX_BUILD_ID__ !== 'undefined' ? __FOX_BUILD_ID__ : 'unknown',
        mode: (typeof import.meta !== 'undefined' && import.meta.env?.MODE) || 'unknown',
        profiling: typeof __FOX_PROFILING__ !== 'undefined' ? __FOX_PROFILING__ : false,
      },
      environment: {
        userAgent: typeof navigator !== 'undefined' ? navigator.userAgent : 'unknown',
        viewport: {
          width: typeof window !== 'undefined' ? window.innerWidth : 0,
          height: typeof window !== 'undefined' ? window.innerHeight : 0,
        },
        devicePixelRatio: typeof window !== 'undefined' ? window.devicePixelRatio || 1 : 1,
      },
      thresholds: {
        longTaskMs: LONG_TASK_MS,
        provisionalFrameBudgetMs: PROVISIONAL_FRAME_BUDGET_MS,
        note:
          'Provisional investigation trigger, not a hard CI gate. Freeze machine, window size, WebView version, warmup and repeat count before setting a gate. Frame gaps, missed vsyncs and Long Tasks are distinct signals.',
      },
      support: {
        longTask: this.longTaskSupported,
        supportedEntryTypes: this.entryTypes,
      },
      estimatedRefreshHz: this.estimatedRefreshHz,
      dropped: {
        renders: this.droppedRenders,
        frames: this.droppedFrames,
        longTasks: this.droppedLongTasks,
        reports: this.droppedReports,
      },
      scenarios: this.completedReports.toArray(),
    }
  }

  /** Redacted JSON: durations/regions/metadata only, no user content. */
  exportJson(): string {
    return JSON.stringify(this.buildReport(), null, 2)
  }

  clear(): void {
    this.renders.clear()
    this.frames.clear()
    this.longTasks.clear()
    this.completedReports.clear()
    this.frameDeltaSamples = []
    this.droppedRenders = 0
    this.droppedFrames = 0
    this.droppedLongTasks = 0
    this.droppedReports = 0
    this.activeRunId = null
    this.activeScenario = null
    this.activeLabel = null
    this.scenarioWindows = []
  }
}

export const profilingCollector = new ProfilingCollector()

/** True only in the profile build (statically replaced by vite define). */
export function profilingBuildEnabled(): boolean {
  return typeof __FOX_PROFILING__ !== 'undefined' && __FOX_PROFILING__ === true
}
