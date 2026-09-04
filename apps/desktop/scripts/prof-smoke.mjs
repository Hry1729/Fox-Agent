// Headless smoke for the profile harness. Drives Chrome over the DevTools
// Protocol (no playwright dependency): opens <url> (which must be the profile
// build with ?foxPerf=auto), waits for the harness to run mixed-500 / markdown
// stream / code cold-warm, then reads the #fox-perf-result JSON and writes it.
//
// Run with bun:  bun scripts/prof-smoke.mjs <http://.../?foxPerf=auto> out.json
import { spawn } from 'node:child_process'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { writeFileSync } from 'node:fs'

const CHROME = process.env.CHROME_BIN || 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe'
const targetUrl = process.argv[2]
const outPath = process.argv[3] || 'perf-smoke.json'
if (!targetUrl) {
  console.error('usage: bun prof-smoke.mjs <url-with-foxPerf=auto> [out.json]')
  process.exit(2)
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
const PORT = 9333

function cdp(ws) {
  let id = 0
  const pending = new Map()
  ws.addEventListener('message', (event) => {
    const msg = JSON.parse(event.data)
    if (msg.id && pending.has(msg.id)) {
      const { resolve, reject } = pending.get(msg.id)
      pending.delete(msg.id)
      if (msg.error) reject(new Error(JSON.stringify(msg.error)))
      else resolve(msg.result)
    }
  })
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const mid = ++id
      pending.set(mid, { resolve, reject })
      ws.send(JSON.stringify({ id: mid, method, params }))
    })
  return { send }
}

async function main() {
  const userData = join(tmpdir(), `fox-prof-${Date.now()}`)
  const chrome = spawn(CHROME, [
    '--headless=new',
    `--remote-debugging-port=${PORT}`,
    '--disable-gpu',
    '--no-first-run',
    '--no-default-browser-check',
    '--window-size=1400,1000',
    `--user-data-dir=${userData}`,
    'about:blank',
  ], { stdio: 'ignore' })

  try {
    let version = null
    for (let i = 0; i < 50; i += 1) {
      await sleep(400)
      try {
        const res = await fetch(`http://127.0.0.1:${PORT}/json/version`)
        if (res.ok) { version = await res.json(); break }
      } catch { /* retry */ }
    }
    if (!version) throw new Error('chrome devtools did not come up')

    // Open a new tab already at the target URL.
    const created = await fetch(`http://127.0.0.1:${PORT}/json/new?${encodeURIComponent(targetUrl)}`, { method: 'PUT' })
      .then((r) => r.json())
      .catch(() => null)
    if (!created || !created.webSocketDebuggerUrl) throw new Error('could not create target tab')

    const ws = new WebSocket(created.webSocketDebuggerUrl)
    await new Promise((resolve, reject) => {
      ws.addEventListener('open', resolve, { once: true })
      ws.addEventListener('error', () => reject(new Error('ws failed')), { once: true })
    })
    const { send } = cdp(ws)
    await send('Page.enable')
    await send('Runtime.enable')

    // Poll for the harness result element (real timers; up to ~3 min).
    let report = null
    for (let i = 0; i < 180; i += 1) {
      await sleep(1000)
      const { result } = await send('Runtime.evaluate', {
        expression: 'document.querySelector("#fox-perf-result")?.textContent || ""',
        returnByValue: true,
      })
      const text = result?.value || ''
      if (text) { report = text; break }
    }

    if (!report) {
      // Capture console errors / page text for diagnosis.
      const { result } = await send('Runtime.evaluate', {
        expression: 'document.body ? document.body.innerText.slice(0, 800) : "no body"',
        returnByValue: true,
      })
      throw new Error(`harness produced no result. Page text:\n${result?.value || ''}`)
    }

    writeFileSync(outPath, report)
    const parsed = JSON.parse(report)
    if (parsed.error) throw new Error(`harness reported error: ${parsed.error}`)

    // ---- Hard assertions (not just "JSON exists") ----
    const asserts = []
    const check = (condition, message) => {
      if (!condition) asserts.push(message)
    }
    check(parsed.build?.profiling === true, `build.profiling must be true (got ${parsed.build?.profiling})`)
    const scenarios = parsed.scenarios || []
    check(scenarios.length === 5, `expected exactly 5 scenarios, got ${scenarios.length}`)

    const runIds = scenarios.map((s) => s.scenarioRunId)
    check(new Set(runIds).size === runIds.length, 'scenarioRunIds must be unique')

    for (const s of scenarios) {
      check(s.renderCommitCount > 0, `${s.scenario}/${s.kind} has 0 commits`)
    }

    const byKind = (kind) => scenarios.filter((s) => s.kind === kind)
    const cold = byKind('code-cold')
    const warm = byKind('code-warm')
    check(cold.length === 1 && warm.length === 1, 'expected one code-cold and one code-warm run')
    if (cold.length && warm.length) {
      check(cold[0].scenarioRunId !== warm[0].scenarioRunId, 'cold/warm must be distinct runs')
    }
    const mixed = scenarios.find((s) => s.scenario === 'history-mount' && s.kind === 'mixed')
    check(mixed && mixed.messageCount === 500, `mixed 500 messageCount must be 500 (got ${mixed?.messageCount})`)

    const blocking = scenarios.find((s) => s.scenario === 'blocking')
    check(blocking, 'missing blocking scenario')
    if (blocking) {
      check(blocking.renderCommitCount > 0, 'blocking: no React commit')
      check(blocking.frames.overBudgetFrames >= 1 || blocking.frames.totalMissedVsyncs >= 1,
        `blocking: expected over-budget frame/missed vsync (frames=${blocking.frames.count} over=${blocking.frames.overBudgetFrames} missed=${blocking.frames.totalMissedVsyncs})`)
      if (blocking.longTasks.supported) {
        check(blocking.longTasks.count >= 1, `blocking: expected a Long Task (got ${blocking.longTasks.count})`)
      }
    }

    // All five synthetic scenarios must settle (quiet window reached, no timeout).
    const unsettled = scenarios.filter((s) => s.settle?.status !== 'settled')
    check(unsettled.length === 0,
      `all scenarios should settle; non-settled: ${unsettled.map((s) => `${s.kind}:${s.settle?.status}`).join(', ')}`)

    // Dropped samples: every bounded buffer must be 0 in this small run.
    const dropped = parsed.dropped || {}
    for (const key of ['renders', 'frames', 'longTasks', 'reports']) {
      check((dropped[key] ?? 0) === 0, `dropped.${key} should be 0 (got ${dropped[key]})`)
    }

    // Redaction: no fixture content or paths.
    const redacted = !report.includes('请处理第') && !report.includes('src/mod-') && !report.includes('fixture-conversation')
    check(redacted, 'exported JSON must not contain fixture content or paths')

    if (asserts.length) throw new Error('smoke assertions failed:\n  - ' + asserts.join('\n  - '))

    const lines = scenarios.map((s) =>
      `  ${s.scenarioRunId} ${s.scenario}/${s.kind}/${s.coldWarm ?? '-'} msgs=${s.messageCount ?? '-'} ` +
      `commits=${s.renderCommitCount} overFrames=${s.frames.overBudgetFrames} missedVsync=${s.frames.totalMissedVsyncs} ` +
      `longTasks=${s.longTasks.count} settle=${s.settle?.status} hz=${s.frames.estimatedRefreshHz}`)
    console.log('PROFILE SMOKE OK')
    console.log(`build: ${parsed.build?.buildId} profiling=${parsed.build?.profiling} refreshHz=${parsed.estimatedRefreshHz}`)
    console.log(lines.join('\n'))
  } finally {
    chrome.kill()
  }
}

main().catch((err) => {
  console.error('PROFILE SMOKE FAILED:', err.message)
  process.exit(1)
})
