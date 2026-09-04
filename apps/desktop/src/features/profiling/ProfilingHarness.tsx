/**
 * Profile-build-only performance harness (exclusively mounted: when it runs,
 * the real App/Workbench is NOT mounted, so no background commits pollute the
 * samples). Lazy-loaded from main.tsx only when __FOX_PROFILING__ is true;
 * normal releases DCE the entry entirely.
 *
 * It renders the real RuntimeTimeline and drives streams through the SAME
 * production primitives — enqueueRuntimeEvent / takeRuntimeEventFrame (RAF
 * batching) / reduceRuntimeNotifications — but invoked manually from this
 * harness, not via the useRuntimeEventStream subscription. All data is
 * synthetic and in-memory; nothing touches the user database.
 */
import { Profiler, useCallback, useEffect, useRef, useState, type CSSProperties } from 'react'
import { flushSync } from 'react-dom'
import { TooltipProvider } from '@/components/ui/tooltip'
import type { ConversationDetail, RunEventRecord, ConversationMessage } from '@/features/conversations/model/types'
import { reduceRuntimeNotifications } from '@/features/conversations/model/runtime-event-reducer'
import { enqueueRuntimeEvent, takeRuntimeEventFrame } from '@/features/conversations/model/runtime-event-queue'
import { RuntimeTimeline } from '@/features/chat/workbench'
import type { RuntimeEventNotification } from '@/features/conversations/model/types'
import {
  buildHistoryDetail,
  buildStreamScenario,
  buildStreamStartDetail,
  FIXTURE_STREAM_RUN_ID,
  type FixtureKind,
} from './fixtures'
import { profilingCollector, type ScenarioKind, type ScenarioReport, type ColdWarm } from './telemetry'

const HISTORY_SIZES = [100, 500, 1000] as const

function frame(): Promise<void> {
  return new Promise((resolve) => requestAnimationFrame(() => resolve()))
}

/** Wait until no new Profiler commits arrive for `quietFrames`, or timeout. */
async function waitForQuiet(
  getCommitCount: () => number,
  { quietFrames = 30, maxMs = 20_000, minCommits = 1 }: { quietFrames?: number; maxMs?: number; minCommits?: number } = {},
): Promise<{ status: 'settled' | 'timed-out'; waitedMs: number }> {
  const start = performance.now()
  let last = getCommitCount()
  let quiet = 0
  while (performance.now() - start < maxMs) {
    await frame()
    const current = getCommitCount()
    if (current !== last) {
      quiet = 0
      last = current
    } else {
      quiet += 1
    }
    if (quiet >= quietFrames && current >= minCommits) {
      return { status: 'settled', waitedMs: performance.now() - start }
    }
  }
  return { status: 'timed-out', waitedMs: performance.now() - start }
}

function kindToScenarioKind(kind: FixtureKind, coldWarm: ColdWarm): ScenarioKind {
  if (kind === 'code') return coldWarm === 'warm' ? 'code-warm' : 'code-cold'
  if (kind === 'tools') return 'tools'
  if (kind === 'mixed') return 'mixed'
  return 'markdown'
}

export function ProfilingHarness() {
  const [detail, setDetail] = useState<ConversationDetail | null>(null)
  const [streaming, setStreaming] = useState(false)
  const [streamRunId, setStreamRunId] = useState<string | null>(null)
  const [reports, setReports] = useState<ScenarioReport[]>([])
  const [busy, setBusy] = useState<string | null>(null)
  const [timelineKey, setTimelineKey] = useState(0)
  const [autoReport, setAutoReport] = useState<string | null>(null)
  const [blocker, setBlocker] = useState(0)

  const detailRef = useRef<ConversationDetail | null>(null)
  detailRef.current = detail
  const commitCountRef = useRef(0)

  const pushReport = useCallback((report: ScenarioReport) => {
    setReports((current) => [...current, report])
  }, [])

  // Profiler onRender: count locally (for quiet-window detection) and forward to collector.
  const handleRender = useCallback((...args: Parameters<typeof profilingCollector.onRender>) => {
    commitCountRef.current += 1
    profilingCollector.onRender(...args)
  }, [])

  const runHistoryMount = useCallback(async (kind: FixtureKind, messageCount: number, coldWarm: ColdWarm) => {
    const label = `history:${kind}:${messageCount}${coldWarm ? `:${coldWarm}` : ''}`
    setBusy(label)
    profilingCollector.startSampling()
    try {
      // Unmount first so the next render is a genuine fresh mount.
      flushSync(() => {
        setDetail(null)
        setStreaming(false)
        setStreamRunId(null)
      })
      await frame()

      commitCountRef.current = 0
      profilingCollector.beginScenario('history-mount', kindToScenarioKind(kind, coldWarm), {
        size: messageCount,
        coldWarm,
        messageCount,
      })
      // Establish the frame baseline FIRST (one rAF after the window opens), so
      // the heavy mount is measured as a post-baseline gap rather than lost.
      await frame()
      // The workload runs in a LATER task than beginScenario: force remount and
      // render the big history; the following rAF captures its gap/long-task.
      flushSync(() => {
        setTimelineKey((key) => key + 1)
        setDetail(buildHistoryDetail(kind, messageCount))
      })
      // Wait for lazy content (Shiki highlighter, MessageResponse) to settle.
      const settle = await waitForQuiet(() => commitCountRef.current)
      profilingCollector.recordSettle(settle.status, Math.round(settle.waitedMs))
      const report = profilingCollector.endScenario()
      pushReport(report)
    } finally {
      profilingCollector.stopSampling()
      setBusy(null)
    }
  }, [pushReport])

  const runStreamReplay = useCallback(async (kind: FixtureKind) => {
    const label = `stream:${kind}`
    setBusy(label)
    const scenario = buildStreamScenario(kind)
    profilingCollector.startSampling()
    try {
      flushSync(() => {
        setDetail(buildStreamStartDetail())
        setStreaming(true)
        setStreamRunId(scenario.runId)
        setTimelineKey((key) => key + 1)
      })
      await frame()

      commitCountRef.current = 0
      profilingCollector.beginScenario('stream-replay', kindToScenarioKind(kind, null), {
        size: scenario.eventCount,
        eventCount: scenario.eventCount,
      })
      // Establish the frame baseline after the window opens; the first event
      // batch is then flushed in a later task so its gap is captured.
      await frame()

      const queue: RuntimeEventNotification[] = []
      let index = 0
      const perFrame = 4
      while (index < scenario.notifications.length) {
        for (let added = 0; added < perFrame && index < scenario.notifications.length; added += 1) {
          enqueueRuntimeEvent(queue, scenario.notifications[index])
          index += 1
        }
        const batch = takeRuntimeEventFrame(queue)
        if (batch.length > 0) {
          const before = detailRef.current
          flushSync(() => setDetail(reduceRuntimeNotifications(before!, batch)))
        }
        await frame()
      }
      const remaining = takeRuntimeEventFrame(queue)
      if (remaining.length > 0) {
        flushSync(() => setDetail(reduceRuntimeNotifications(detailRef.current!, remaining)))
      }
      // Let lazy renderers settle before closing the scenario.
      const settle = await waitForQuiet(() => commitCountRef.current)
      profilingCollector.recordSettle(settle.status, Math.round(settle.waitedMs))

      const report = profilingCollector.endScenario()
      pushReport(report)

      flushSync(() => {
        setStreaming(false)
        setStreamRunId(null)
      })
    } finally {
      profilingCollector.stopSampling()
      setBusy(null)
    }
  }, [pushReport])

  // Cold/warm code pair: cold MUST run on a fresh page (Shiki not yet loaded).
  // Triggered via ?foxPerf=codecold which reloads the page first.
  const runColdWarmPair = useCallback(async () => {
    await runHistoryMount('code', 200, 'cold')
    await frame()
    await runHistoryMount('code', 200, 'warm')
  }, [runHistoryMount])

  // Controllable blocking scenario: after the frame baseline is established, a
  // single later task performs a heavy React commit AND synchronously blocks the
  // main thread ~95ms. This must produce, from real browser timing, a React
  // commit, an over-budget frame (missed vsyncs) and (when supported) a Long Task.
  const runBlockingScenario = useCallback(async () => {
    setBusy('blocking')
    profilingCollector.startSampling()
    try {
      flushSync(() => {
        setBlocker(0)
        setDetail(null)
      })
      await frame()
      commitCountRef.current = 0
      profilingCollector.beginScenario('blocking', 'blocking', { size: null })
      // Establish baseline first...
      await frame()
      // ...then block + heavy commit in a LATER task.
      const start = performance.now()
      flushSync(() => setBlocker(95))
      while (performance.now() - start < 95) {
        // Synchronous main-thread block: guarantees a long task + missed vsyncs.
      }
      const settle = await waitForQuiet(() => commitCountRef.current, { minCommits: 1 })
      profilingCollector.recordSettle(settle.status, Math.round(settle.waitedMs))
      const report = profilingCollector.endScenario()
      pushReport(report)
      flushSync(() => setBlocker(0))
    } finally {
      profilingCollector.stopSampling()
      setBusy(null)
    }
  }, [pushReport])

  useEffect(() => {
    const params = new URLSearchParams(window.location.search)
    const mode = params.get('foxPerf')
    if (mode === 'codecold') {
      window.history.replaceState({}, '', window.location.pathname)
      void runColdWarmPair()
      return
    }
    if (mode !== 'auto') return
    let cancelled = false
    async function runAuto() {
      try {
        // Code-cold MUST be the first scenario: on a fresh harness page no code
        // block has rendered yet, so the Shiki dynamic import is genuinely cold.
        await runHistoryMount('code', 200, 'cold')
        await frame()
        await runHistoryMount('code', 200, 'warm')
        await runHistoryMount('mixed', 500, null)
        await runStreamReplay('markdown')
        await runBlockingScenario()
        if (!cancelled) setAutoReport(profilingCollector.exportJson())
      } catch (err) {
        if (!cancelled) setAutoReport(JSON.stringify({ error: String(err) }))
      }
    }
    void runAuto()
    return () => {
      cancelled = true
    }
  }, [runHistoryMount, runStreamReplay, runColdWarmPair, runBlockingScenario])

  const handleExport = useCallback(() => {
    const blob = new Blob([profilingCollector.exportJson()], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = 'fox-perf-report.json'
    anchor.click()
    URL.revokeObjectURL(url)
  }, [])

  const handleClear = useCallback(() => {
    profilingCollector.clear()
    setReports([])
    commitCountRef.current = 0
    flushSync(() => {
      setDetail(null)
      setStreaming(false)
      setStreamRunId(null)
    })
  }, [])

  const reloadForCold = useCallback(() => {
    const url = new URL(window.location.href)
    url.searchParams.set('foxPerf', 'codecold')
    window.location.assign(url.toString())
  }, [])

  const messages: ConversationMessage[] = detail?.messages ?? []
  const events: RunEventRecord[] = detail?.runtimeEvents ?? []

  return (
    <TooltipProvider>
      <div style={overlay}>
        <div style={toolbar}>
          <strong>Fox 性能夹具</strong>
          <span style={hint}>历史挂载（消息总数）：</span>
          {HISTORY_SIZES.map((size) => (
            <button key={size} type="button" disabled={busy !== null} onClick={() => void runHistoryMount('mixed', size, null)} style={btn}>混合 {size}</button>
          ))}
          <button type="button" disabled={busy !== null} onClick={() => void runHistoryMount('markdown', 500, null)} style={btn}>Markdown 500</button>
          <button type="button" disabled={busy !== null} onClick={() => void runHistoryMount('tools', 500, null)} style={btn}>工具 500</button>
          <button type="button" disabled={busy !== null} onClick={reloadForCold} style={btn}>代码 cold→warm（刷新页）</button>
          <span style={hint}>流式回放：</span>
          <button type="button" disabled={busy !== null} onClick={() => void runStreamReplay('markdown')} style={btn}>Markdown 流</button>
          <button type="button" disabled={busy !== null} onClick={() => void runStreamReplay('tools')} style={btn}>工具流</button>
          <button type="button" disabled={busy !== null} onClick={() => void runStreamReplay('code')} style={btn}>代码流</button>
          <button type="button" disabled={busy !== null} onClick={() => void runBlockingScenario()} style={btn}>阻塞场景（95ms）</button>
          <button type="button" disabled={busy !== null} onClick={handleClear} style={btn}>清空</button>
          <button type="button" disabled={busy !== null} onClick={handleExport} style={{ ...btn, background: '#2563eb', color: '#fff' }}>导出 JSON</button>
          {busy && <span style={hint}>运行中：{busy}</span>}
        </div>
        <div style={{ flex: 1, overflow: 'auto' }}>
          {detail && (
            <Profiler id="conversation-timeline" onRender={handleRender}>
              <RuntimeTimeline
                key={timelineKey}
                messages={messages}
                attachments={[]}
                artifacts={[]}
                events={events}
                activeRunId={streamRunId ?? FIXTURE_STREAM_RUN_ID}
                runtimeRunning={streaming}
                state="complete"
                streamingText={streamingTextOf(detail, streamRunId ?? FIXTURE_STREAM_RUN_ID, streaming)}
                runtimeError={null}
                runtimeErrorDetails={null}
                assistantName="Fox 性能助手"
                activeRunModel="fixture-model"
                hasEarlierMessages={false}
                loadingEarlierMessages={false}
                onLoadEarlierMessages={() => undefined}
                onRetry={() => undefined}
                onRerun={async () => true}
              />
            </Profiler>
          )}
          {blocker > 0 && (
            <Profiler id="blocking-workload" onRender={handleRender}>
              <BlockingWorkload count={blocker * 200} />
            </Profiler>
          )}
        </div>
        <ReportTable reports={reports} />
        {autoReport && <pre id="fox-perf-result" style={{ display: 'none' }}>{autoReport}</pre>}
      </div>
    </TooltipProvider>
  )
}

/** Pure render load: many nodes so the flushSync commit is itself expensive. */
function BlockingWorkload({ count }: { count: number }) {
  return (
    <div style={{ display: 'none' }} aria-hidden>
      {Array.from({ length: count }, (_, index) => (
        <span key={index}>{index}</span>
      ))}
    </div>
  )
}

function streamingTextOf(detail: ConversationDetail, runId: string, running: boolean): string {
  if (!running) return ''
  const message = [...detail.messages].reverse().find((item) => item.role === 'assistant' && item.runId === runId)
  return message?.content ?? ''
}

const overlay: CSSProperties = { position: 'fixed', inset: 0, zIndex: 99999, display: 'flex', flexDirection: 'column', background: 'var(--background, #fff)', color: 'var(--foreground, #111)' }
const toolbar: CSSProperties = { display: 'flex', gap: 8, padding: 10, alignItems: 'center', flexWrap: 'wrap', borderBottom: '1px solid rgba(128,128,128,0.3)', fontFamily: 'monospace', fontSize: 12 }
const hint: CSSProperties = { opacity: 0.7 }
const btn: CSSProperties = { padding: '4px 8px', fontSize: 12, fontFamily: 'monospace', border: '1px solid rgba(128,128,128,0.5)', borderRadius: 4, background: 'transparent', color: 'inherit', cursor: 'pointer' }

function ReportTable({ reports }: { reports: ScenarioReport[] }) {
  if (reports.length === 0) {
    return <div style={{ padding: 10, fontFamily: 'monospace', fontSize: 12, opacity: 0.7 }}>运行场景后在此查看 P50/P95。导出 JSON 只含时长/计数/元数据，不含消息正文或用户数据。</div>
  }
  return (
    <div style={{ maxHeight: 240, overflow: 'auto', borderTop: '1px solid rgba(128,128,128,0.3)', fontFamily: 'monospace', fontSize: 11 }}>
      <table style={{ borderCollapse: 'collapse', width: '100%' }}>
        <thead>
          <tr>
            <th style={th}>runId</th><th style={th}>场景</th><th style={th}>类型</th><th style={th}>冷/热</th><th style={th}>消息</th>
            <th style={th}>mount P95</th><th style={th}>update P95</th><th style={th}>commit</th>
            <th style={th}>超预算帧</th><th style={th}>丢vsync</th><th style={th}>帧P95</th><th style={th}>LongTask</th><th style={th}>Hz</th>
          </tr>
        </thead>
        <tbody>
          {reports.map((report) => {
            const timeline = report.renderByRegion['conversation-timeline']
            return (
              <tr key={report.scenarioRunId}>
                <td style={td}>{report.scenarioRunId.slice(-6)}</td>
                <td style={td}>{report.scenario}</td>
                <td style={td}>{report.kind}</td>
                <td style={td}>{report.coldWarm ?? '-'}</td>
                <td style={td}>{report.messageCount ?? report.size ?? '-'}</td>
                <td style={td}>{timeline ? `${timeline.mount.p95.toFixed(1)}` : '-'}</td>
                <td style={td}>{timeline ? `${timeline.update.p95.toFixed(1)}` : '-'}</td>
                <td style={td}>{report.renderCommitCount}</td>
                <td style={td}>{(report.frames.overBudgetRatio * 100).toFixed(0)}%</td>
                <td style={td}>{report.frames.totalMissedVsyncs}</td>
                <td style={td}>{report.frames.p95Gap.toFixed(1)}</td>
                <td style={td}>{report.longTasks.supported ? `${report.longTasks.count}/${report.longTasks.totalMs.toFixed(0)}ms` : 'N/A'}</td>
                <td style={td}>{report.frames.estimatedRefreshHz}</td>
              </tr>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}

const th: CSSProperties = { padding: '4px 8px', textAlign: 'right', borderBottom: '1px solid rgba(128,128,128,0.3)', position: 'sticky', top: 0, background: 'rgba(128,128,128,0.1)' }
const td: CSSProperties = { padding: '3px 8px', textAlign: 'right', whiteSpace: 'nowrap' }
