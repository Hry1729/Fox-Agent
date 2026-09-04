// Verifies the REAL-App profile controller (not the synthetic harness):
// loads the app page WITHOUT ?foxPerf, confirms the RealProfilingController
// mounts, starts collection, drives real App renders, stops, and checks that a
// real-workbench scenario captured commits. Run with bun.
import { spawn } from 'node:child_process'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const CHROME = process.env.CHROME_BIN || 'C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe'
const targetUrl = process.argv[2]
if (!targetUrl) { console.error('usage: bun realapp-smoke.mjs <http://host/>'); process.exit(2) }
const sleep = (ms) => new Promise((r) => setTimeout(r, ms))
const PORT = 9334

function cdp(ws) {
  let id = 0
  const pending = new Map()
  ws.addEventListener('message', (e) => {
    const m = JSON.parse(e.data)
    if (m.id && pending.has(m.id)) { const { resolve, reject } = pending.get(m.id); pending.delete(m.id); m.error ? reject(new Error(JSON.stringify(m.error))) : resolve(m.result) }
  })
  const send = (method, params = {}) => new Promise((resolve, reject) => { const mid = ++id; pending.set(mid, { resolve, reject }); ws.send(JSON.stringify({ id: mid, method, params })) })
  return { send }
}

async function main() {
  const userData = join(tmpdir(), `fox-real-${Date.now()}`)
  const chrome = spawn(CHROME, ['--headless=new', `--remote-debugging-port=${PORT}`, '--disable-gpu', '--no-first-run', '--no-default-browser-check', '--window-size=1400,1000', `--user-data-dir=${userData}`, 'about:blank'], { stdio: 'ignore' })
  try {
    let version = null
    for (let i = 0; i < 50; i++) { await sleep(400); try { const r = await fetch(`http://127.0.0.1:${PORT}/json/version`); if (r.ok) { version = await r.json(); break } } catch {} }
    if (!version) throw new Error('chrome devtools did not come up')
    const tab = await fetch(`http://127.0.0.1:${PORT}/json/new?${encodeURIComponent(targetUrl)}`, { method: 'PUT' }).then((r) => r.json())
    const ws = new WebSocket(tab.webSocketDebuggerUrl)
    await new Promise((res, rej) => { ws.addEventListener('open', res, { once: true }); ws.addEventListener('error', () => rej(new Error('ws failed')), { once: true }) })
    const { send } = cdp(ws)
    await send('Page.enable'); await send('Runtime.enable')
    const evalJs = async (expression) => (await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })).result?.value

    await sleep(2500) // let App + lazy controller load
    // Skip onboarding (fresh profile would show it, where Workbench Profilers don't mount).
    await evalJs(`localStorage.setItem('fox.onboarding.status','done'); location.reload()`)
    await sleep(3500)

    // The RealProfilingController fab (real app) shows "PERF"; the harness button shows "HARNESS".
    const fabState = await evalJs(`(() => {
      const btns = [...document.querySelectorAll('button')].map(b => b.textContent.trim())
      return JSON.stringify(btns.filter(t => /PERF|HARNESS/.test(t)))
    })()`)
    console.log('floating buttons:', fabState)
    if (!/PERF/.test(fabState)) throw new Error('RealProfilingController PERF fab not found on real App page')

    // Open controller.
    await evalJs(`[...document.querySelectorAll('button')].find(b => b.textContent.trim().startsWith('PERF'))?.click()`)
    await sleep(500)
    const panelText = await evalJs(`document.body.innerText.includes('Fox 真实采集')`)
    if (!panelText) throw new Error('controller panel did not open')

    // Start collection.
    await evalJs(`[...document.querySelectorAll('button')].find(b => b.textContent.trim() === '开始采集')?.click()`)
    await sleep(300)
    const recording = await evalJs(`document.body.innerText.includes('采集中')`)
    if (!recording) throw new Error('did not enter recording state')

    // Focus the composer textarea, then drive real Workbench renders via CDP
    // text input (fires React's synthetic onChange, unlike a raw value set).
    const focused = await evalJs(`(() => {
      const ta = document.querySelector('textarea');
      if (!ta) return 'no-textarea';
      ta.focus();
      return 'focused';
    })()`)
    console.log('composer textarea:', focused)
    await send('DOM.enable')
    for (let i = 0; i < 10; i++) {
      await send('Input.insertText', { text: `真实采集冒烟输入 ${i} 段文本用于触发 Workbench 提交 ` })
      await sleep(180)
    }
    // Also toggle interactions that re-render Workbench shell.
    await evalJs(`window.dispatchEvent(new Event('resize'))`)
    await sleep(600)
    await evalJs(`window.dispatchEvent(new Event('resize'))`)
    await sleep(600)

    // Stop and end scenario.
    await evalJs(`[...document.querySelectorAll('button')].find(b => b.textContent.trim().startsWith('停止'))?.click()`)
    await sleep(500)
    const summary = await evalJs(`(() => { const t = document.body.innerText; const m = t.match(/上次：[^\\n]*/); return m ? m[0] : '(no summary found)\\n' + t.slice(-300) })()`)
    console.log('real-app scenario summary:', summary)
    if (!/commits=\d+/.test(summary)) throw new Error('no real-workbench scenario summary after stop')
    if (/commits=0/.test(summary)) throw new Error('real-workbench scenario captured 0 commits')

    console.log('REAL APP CONTROLLER SMOKE OK')
  } finally {
    chrome.kill()
  }
}
main().catch((e) => { console.error('REAL APP SMOKE FAILED:', e.message); process.exit(1) })
