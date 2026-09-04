/**
 * Profile-build-only lightweight controller for collecting metrics from the
 * REAL running App/Workbench (not the synthetic harness). It drives the same
 * singleton collector; Workbench's <Profiler onRender> already forwards to it
 * when __FOX_PROFILING__ is true.
 *
 * Only a short non-sensitive label is recorded (real-conversation/real-stream/
 * real-scroll/real-composer). Never message text, titles, paths or user input.
 *
 * Mounted only on the real App branch (main.tsx), so it is mutually exclusive
 * with the synthetic Harness. DCE'd entirely from normal releases.
 */
import { useCallback, useState, type CSSProperties } from 'react'
import { profilingCollector, type ScenarioReport } from './telemetry'

const LABELS = ['real-conversation', 'real-stream', 'real-scroll', 'real-composer'] as const
type Label = (typeof LABELS)[number]

export function RealProfilingController() {
  const [open, setOpen] = useState(false)
  const [label, setLabel] = useState<Label>('real-stream')
  const [recording, setRecording] = useState(false)
  const [last, setLast] = useState<ScenarioReport | null>(null)

  const start = useCallback(() => {
    profilingCollector.startSampling()
    profilingCollector.beginScenario('real-workbench', 'real', { label })
    setRecording(true)
    setLast(null)
  }, [label])

  const stop = useCallback(() => {
    const report = profilingCollector.endScenario()
    profilingCollector.stopSampling()
    setRecording(false)
    setLast(report)
  }, [])

  const clear = useCallback(() => {
    profilingCollector.clear()
    setLast(null)
  }, [])

  const exportJson = useCallback(() => {
    const blob = new Blob([profilingCollector.exportJson()], { type: 'application/json' })
    const url = URL.createObjectURL(blob)
    const anchor = document.createElement('a')
    anchor.href = url
    anchor.download = 'fox-perf-real.json'
    anchor.click()
    URL.revokeObjectURL(url)
  }, [])

  if (!open) {
    return (
      <button type="button" onClick={() => setOpen(true)} style={fab} title="Fox 性能采集（profile 构建）">
        PERF{recording ? ' ●' : ''}
      </button>
    )
  }

  return (
    <div style={panel}>
      <div style={header}>
        <strong>Fox 真实采集</strong>
        <button type="button" style={mini} onClick={() => setOpen(false)}>×</button>
      </div>
      <div style={row}>
        <label style={lbl}>场景标签</label>
        <select value={label} disabled={recording} onChange={(event) => setLabel(event.target.value as Label)} style={select}>
          {LABELS.map((item) => <option key={item} value={item}>{item}</option>)}
        </select>
      </div>
      <div style={row}>
        {!recording
          ? <button type="button" style={{ ...btn, background: '#16a34a', color: '#fff' }} onClick={start}>开始采集</button>
          : <button type="button" style={{ ...btn, background: '#dc2626', color: '#fff' }} onClick={stop}>停止并结束场景</button>}
        <button type="button" style={btn} onClick={clear} disabled={recording}>清空</button>
        <button type="button" style={btn} onClick={exportJson}>导出 JSON</button>
      </div>
      <div style={status}>
        {recording
          ? '● 采集中：在 App 内进行会话切换 / 流式回答 / 滚动，然后停止。'
          : last
            ? `上次：${last.scenarioRunId} commits=${last.renderCommitCount} overFrames=${last.frames.overBudgetFrames} longTasks=${last.longTasks.count} settle=${last.settle.status}`
            : '空闲。开始后操作真实 App，再停止并导出。'}
      </div>
      <div style={footnote}>仅记录时长/计数/标签；不含消息正文、标题、路径或输入。</div>
    </div>
  )
}

const fab: CSSProperties = { position: 'fixed', right: 12, bottom: 12, zIndex: 99999, padding: '4px 10px', fontSize: 12, fontFamily: 'monospace', borderRadius: 6, border: '1px solid rgba(128,128,128,0.5)', background: 'rgba(0,0,0,0.7)', color: '#fff', cursor: 'pointer' }
const panel: CSSProperties = { position: 'fixed', right: 12, bottom: 12, zIndex: 99999, width: 320, padding: 12, borderRadius: 8, border: '1px solid rgba(128,128,128,0.4)', background: 'var(--background, #fff)', color: 'var(--foreground, #111)', fontFamily: 'monospace', fontSize: 12, boxShadow: '0 8px 30px rgba(0,0,0,0.3)' }
const header: CSSProperties = { display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 8 }
const row: CSSProperties = { display: 'flex', gap: 6, alignItems: 'center', marginBottom: 8, flexWrap: 'wrap' }
const lbl: CSSProperties = { opacity: 0.7 }
const select: CSSProperties = { flex: 1, padding: '3px 6px', fontSize: 12, fontFamily: 'monospace' }
const btn: CSSProperties = { padding: '4px 8px', fontSize: 12, fontFamily: 'monospace', border: '1px solid rgba(128,128,128,0.5)', borderRadius: 4, background: 'transparent', color: 'inherit', cursor: 'pointer' }
const mini: CSSProperties = { border: 'none', background: 'transparent', color: 'inherit', cursor: 'pointer', fontSize: 16, lineHeight: 1 }
const status: CSSProperties = { opacity: 0.85, marginBottom: 6, lineHeight: 1.5 }
const footnote: CSSProperties = { opacity: 0.55, fontSize: 11 }
