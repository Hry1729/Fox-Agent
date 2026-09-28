// Real-browser check for the two diagram questions that happy-dom cannot answer:
// does a Mermaid block actually produce a rendered diagram inside a real answer, and does it
// render again after a remount (the history-reopen analog)?
//
// The component, the plugin, beautiful-mermaid and Streamdown are all the real ones; only the
// Tauri host and a live model are absent. The harness page is created here and removed in a
// finally block, so nothing temporary is left in the tree.
//
// Usage (from the repo root): node apps/desktop/scripts/mermaid-browser-check.mjs
import { spawn } from 'node:child_process'
import { existsSync, mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const desktopRoot = fileURLToPath(new URL('..', import.meta.url))
const repoRoot = fileURLToPath(new URL('../../..', import.meta.url))
const htmlPath = join(desktopRoot, 'mermaid-browser-check.html')
const tsxPath = join(desktopRoot, 'mermaid-browser-check.tsx')
const PORT = Number(process.env.MERMAID_CHECK_PORT || 1449)
const CDP_PORT = Number(process.env.MERMAID_CHECK_CDP || 9353)
const OUT = process.env.MERMAID_CHECK_OUT || join(repoRoot, 'output', 'visual-answers-browser')
const CHROME = process.env.CHROME_PATH || 'C:/Program Files/Google/Chrome/Application/chrome.exe'

const FLOWCHART = 'flowchart TD\n  A[开始] --> B{通过?}\n  B -->|是| C[完成]\n  B -->|否| D[退回]'
const PIE = 'pie title 占比\n  "甲" : 60\n  "乙" : 40'

const html = `<!doctype html><html><head><meta charset="utf-8"><title>mermaid browser check</title>
<style>html,body{margin:0;background:#fff;font-family:system-ui,sans-serif}#root{padding:24px;max-width:760px}</style>
</head><body><div id="root"></div><script type="module" src="/mermaid-browser-check.tsx"></script></body></html>`

const tsx = `import { createRoot } from "react-dom/client";
import { MessageResponse } from "./src/components/ai-elements/message-response";

const kind = new URLSearchParams(location.search).get("case") ?? "flowchart";
const fence = (body: string) => "\\u0060\\u0060\\u0060mermaid\\n" + body + "\\n\\u0060\\u0060\\u0060";
const pie = 'pie title 占比\\n  "甲" : 60\\n  "乙" : 40';
const flow = 'flowchart TD\\n  A[开始] --> B{通过?}\\n  B -->|是| C[完成]\\n  B -- 否 --> D[退回]';
const sequence = 'sequenceDiagram\\n  participant C as 客户端\\n  C->>G: 请求';
// The flowchart case carries two diagram blocks on purpose: that is the multi-diagram
// on-screen case, and their ids must not collide.
const markdown = kind === "pie"
  ? "前面的话。\\n\\n" + fence(pie) + "\\n\\n后面的话。\\n"
  : "前面的话。\\n\\n" + fence(flow) + "\\n\\n中间的说明。\\n\\n" + fence(sequence) + "\\n\\n后面的话。\\n";

const report = (value: unknown) => { (window as unknown as { __mermaidCheck?: unknown }).__mermaidCheck = value };
createRoot(document.getElementById("root") as HTMLElement).render(<MessageResponse>{markdown}</MessageResponse>);

const collect = () => ({
  case: kind,
  hasSvg: !!document.querySelector("#root svg"),
  markers: Array.from(document.querySelectorAll("#root marker")).map((element) => element.getAttribute("id") ?? ""),
  ids: Array.from(document.querySelectorAll("#root [id]")).map((element) => element.id),
  hasStart: (document.querySelector("#root")?.textContent ?? "").includes("开始"),
  hasClient: (document.querySelector("#root")?.textContent ?? "").includes("客户端"),
  proseBefore: (document.body.textContent ?? "").includes("前面的话"),
  proseAfter: (document.body.textContent ?? "").includes("后面的话"),
  notice: !!document.querySelector(".fox-diagram-notice"),
});

report({ pending: true });
// Settle on a stability window rather than on a count: Streamdown resolves each diagram block
// independently, and one flowchart alone already emits two marker ids.
let lastCount = -1;
let stableSince = Date.now();
let finished = false;
const finish = () => {
  if (finished) return;
  finished = true;
  clearInterval(timer);
  report(collect());
};
const timer = setInterval(() => {
  const count = document.querySelectorAll("#root marker").length;
  const prose = (document.body.textContent ?? "").includes("后面的话") || kind === "pie";
  if (count !== lastCount || !prose) {
    lastCount = count;
    stableSince = Date.now();
    return;
  }
  if (Date.now() - stableSince > 1_500) finish();
}, 100);
setTimeout(finish, 15_000);
`

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

async function waitFor(fn, { attempts = 60, delay = 500 } = {}) {
  for (let index = 0; index < attempts; index += 1) {
    try {
      const value = await fn()
      if (value) return value
    } catch {
      // not ready yet
    }
    await sleep(delay)
  }
  return null
}

const cdp = async (target, path) => fetch(`http://127.0.0.1:${CDP_PORT}${path}`).then((response) => response.json())

async function connect() {
  const list = await waitFor(async () => {
    const targets = await cdp(null, '/json')
    return targets.find((entry) => entry.type === 'page' && entry.url.startsWith('http'))
  })
  if (!list) throw new Error('chrome devtools endpoint never exposed a page')
  const socket = new WebSocket(list.webSocketDebuggerUrl)
  await new Promise((resolve) => socket.addEventListener('open', resolve, { once: true }))
  let id = 0
  const pending = new Map()
  socket.addEventListener('message', (event) => {
    const message = JSON.parse(event.data)
    if (message.id && pending.has(message.id)) {
      pending.get(message.id)(message.result)
      pending.delete(message.id)
    }
  })
  const send = (method, params = {}) => new Promise((resolve) => {
    const messageId = ++id
    pending.set(messageId, resolve)
    socket.send(JSON.stringify({ id: messageId, method, params }))
  })
  const evaluate = async (expression) => (await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })).result?.value
  await send('Runtime.enable')
  await send('Page.enable')
  return { send, evaluate, close: () => socket.close() }
}

const writeHarness = () => {
  writeFileSync(htmlPath, html)
  writeFileSync(tsxPath, tsx)
}

const run = async () => {
  mkdirSync(OUT, { recursive: true })
  writeHarness()
  const vite = spawn('pnpm', ['exec', 'vite', '--host', '127.0.0.1', '--port', String(PORT), '--strictPort'], {
    cwd: desktopRoot, stdio: 'ignore', shell: true,
  })
  const chrome = spawn(CHROME, [
    '--headless=new', `--remote-debugging-port=${CDP_PORT}`, '--no-first-run', '--no-default-browser-check',
    '--window-size=1200,900', `--user-data-dir=${join(OUT, 'chrome-profile')}`,
    `http://127.0.0.1:${PORT}/mermaid-browser-check.html?case=flowchart`,
  ], { stdio: 'ignore' })

  const results = {}
  let client = null
  try {
    if (!existsSync(CHROME)) throw new Error(`chrome not found at ${CHROME}`)
    client = await connect()
    const ready = await waitFor(async () => (await client.evaluate('typeof window.__mermaidCheck !== "undefined"')) === true)
    if (!ready) throw new Error('harness page did not boot')

    const read = async (label) => {
      const value = await waitFor(async () => {
        const current = await client.evaluate('JSON.stringify(window.__mermaidCheck || null)')
        const parsed = current ? JSON.parse(current) : null
        return parsed && !parsed.pending ? parsed : null
      }, { attempts: 40, delay: 500 })
      if (!value) throw new Error(`${label}: harness never settled`)
      results[label] = value
    }

    await read('flowchart')
    await client.send('Page.reload', {})
    const rebooted = await waitFor(async () => (await client.evaluate('typeof window.__mermaidCheck !== "undefined"')) === true)
    if (!rebooted) throw new Error('harness did not reboot after reload')
    await read('reload')

    await client.send('Page.navigate', { url: `http://127.0.0.1:${PORT}/mermaid-browser-check.html?case=pie` })
    const bootedPie = await waitFor(async () => (await client.evaluate('typeof window.__mermaidCheck !== "undefined"')) === true)
    if (!bootedPie) throw new Error('pie harness did not boot')
    await read('pie')

    const shot = await client.send('Page.captureScreenshot', { format: 'png' })
    if (shot?.data) writeFileSync(join(OUT, 'mermaid-browser-check.png'), Buffer.from(shot.data, 'base64'))
  } finally {
    client?.close()
    chrome.kill()
    vite.kill()
    await sleep(500)
    for (const path of [htmlPath, tsxPath]) {
      try { rmSync(path, { force: true }) } catch { /* best effort */ }
    }
  }

  const flowchart = results.flowchart
  const reload = results.reload
  const pie = results.pie
  const failures = []
  // A diagram's namespace is its block id: everything before the marker-local suffix. Two
  // diagrams must therefore produce two *different* namespaces — the collision the review
  // reproduced as shared `arrowhead` / `arrowhead-start`.
  const prefixFor = (markers, suffixes) => {
    for (const id of markers) {
      for (const suffix of suffixes) {
        if (id.endsWith(`-${suffix}`)) return id.slice(0, -(suffix.length + 1))
      }
    }
    return null
  }
  const checkCase = (label, sample) => {
    const markers = sample?.markers ?? []
    const flowPrefix = prefixFor(markers, ['arrowhead', 'arrowhead-start'])
    const seqPrefix = prefixFor(markers, ['seq-arrow-open', 'seq-arrow'])
    if (!sample?.hasSvg) failures.push(`${label}: no rendered diagram`)
    if (markers.length !== 4) failures.push(`${label}: expected 4 marker ids (two diagrams), saw ${markers.length}`)
    if (!flowPrefix) failures.push(`${label}: flowchart marker ids missing (${JSON.stringify(markers)})`)
    if (!seqPrefix) failures.push(`${label}: sequence marker ids missing (${JSON.stringify(markers)})`)
    if (flowPrefix && seqPrefix && flowPrefix === seqPrefix) failures.push(`${label}: the two diagrams share one namespace (${flowPrefix})`)
    if (new Set(sample?.ids ?? []).size !== (sample?.ids ?? []).length) failures.push(`${label}: duplicate ids in one document`)
    if (!sample?.hasStart || !sample?.hasClient) failures.push(`${label}: a diagram lost its labels`)
    if (!sample?.proseBefore || !sample?.proseAfter) failures.push(`${label}: surrounding prose lost`)
  }
  checkCase('flowchart', flowchart)
  checkCase('reload', reload)
  if (!pie?.proseBefore || !pie?.proseAfter) failures.push('pie: surrounding prose lost')

  console.log(JSON.stringify({ results, failures }, null, 2))
  writeFileSync(join(OUT, 'mermaid-browser-check.json'), JSON.stringify({ results, failures }, null, 2))
  if (failures.length > 0) {
    console.error(`mermaid browser check FAILED:\n - ${failures.join('\n - ')}`)
    process.exitCode = 1
  } else {
    console.log('mermaid browser check OK (real browser: diagram rendered, prose intact, remount stable, unsupported family non-fatal)')
  }
}

run().catch((error) => {
  console.error(error.message)
  try { rmSync(htmlPath, { force: true }); rmSync(tsxPath, { force: true }) } catch { /* best effort */ }
  process.exitCode = 1
})
