// Real-browser check for the diagram path, covering what happy-dom cannot:
//   * a Mermaid block really renders inside a real answer, with two diagrams on one screen
//     whose id namespaces do not collide;
//   * the surrounding prose survives, and an unsupported family is non-fatal (either it
//     renders through the stock engine or the notice UI shows a reason);
//   * Fox's own theme stylesheets are loaded, so light/dark, a narrow window and long Chinese
//     labels are checked against the real tokens rather than an unstyled page.
//
// The component, plugin, engines and styles are the real ones; only the Tauri host and a live
// model are absent. The harness page is created here and removed in a finally block.
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

const html = `<!doctype html><html><head><meta charset="utf-8"><title>mermaid browser check</title>
</head><body><div id="root"></div><script type="module" src="/mermaid-browser-check.tsx"></script></body></html>`

// Fox's real stylesheets, exactly the three main.tsx loads, so diagrams inherit the real tokens.
const tsx = `import { createRoot } from "react-dom/client";
import "./src/styles/globals.css";
import "./src/styles/workbench.css";
import "./src/styles/workspace-pages.css";
import { MessageResponse } from "./src/components/ai-elements/message-response";

const params = new URLSearchParams(location.search);
const kind = params.get("case") ?? "flowchart";
const theme = params.get("theme") ?? "light";
if (theme === "dark") document.documentElement.classList.add("dark");

// Unique per page load: after a reload or a navigation the script must not read the previous
// page's result, so every sample carries its own generation token and the case it describes.
const generation = Date.now().toString(36) + "-" + Math.random().toString(36).slice(2, 8);
const window2 = window as unknown as { __mermaidCheck?: unknown };
const report = (value: unknown) => { window2.__mermaidCheck = value };

const tick = String.fromCharCode(96).repeat(3);
const fence = (body: string) => tick + "mermaid\\n" + body + "\\n" + tick;
const flow = "flowchart TD\\n  A[开始] --> B{通过?}\\n  B -->|是| C[完成]\\n  B -- 否 --> D[退回]";
const sequence = "sequenceDiagram\\n  participant C as 客户端\\n  C->>G: 请求";
const pie = 'pie title 占比\\n  "甲" : 60\\n  "乙" : 40';
const longLabel = "这是一个用于验证超长中文标签换行与宽度约束的流程节点名称，长度远超普通节点标签";

const markdown = kind === "pie"
  ? "前面的话。\\n\\n" + fence(pie) + "\\n\\n后面的话。\\n"
  : kind === "long"
    ? "前面的话。\\n\\n" + fence("flowchart TD\\n  A[" + longLabel + "] --> B[下一步]") + "\\n\\n后面的话。\\n"
    : "前面的话。\\n\\n" + fence(flow) + "\\n\\n中间的说明。\\n\\n" + fence(sequence) + "\\n\\n后面的话。\\n";

createRoot(document.getElementById("root") as HTMLElement).render(<MessageResponse>{markdown}</MessageResponse>);

const svgTexts = () => Array.from(document.querySelectorAll("#root svg")).map((element) => (element.textContent ?? "").trim());
const root = () => document.querySelector("#root") as HTMLElement | null;

const collect = () => ({
  case: kind,
  theme,
  generation,
  viewport: [innerWidth, innerHeight],
  rootScrollWidth: document.documentElement.scrollWidth,
  hasSvg: !!document.querySelector("#root svg"),
  markers: Array.from(document.querySelectorAll("#root marker")).map((element) => element.getAttribute("id") ?? ""),
  ids: Array.from(document.querySelectorAll("#root [id]")).map((element) => element.id),
  // A diagram is only a diagram when its own labels are inside the SVG: a lucide icon is an
  // empty <svg> and must never be counted as a rendered chart.
  diagramTexts: svgTexts().filter((text) => text.length > 0),
  notice: document.querySelector(".fox-diagram-notice")?.textContent?.slice(0, 200) ?? null,
  proseBefore: (document.body.textContent ?? "").includes("前面的话"),
  proseAfter: (document.body.textContent ?? "").includes("后面的话"),
  hasStart: svgTexts().some((text) => text.includes("开始")),
  hasClient: svgTexts().some((text) => text.includes("客户端")),
  hasLongLabel: svgTexts().some((text) => text.includes(longLabel)),
  diagramFill: (() => {
    const text = document.querySelector("#root svg text");
    return text ? getComputedStyle(text).fill : null;
  })(),
  foxTextToken: getComputedStyle(document.documentElement).getPropertyValue("--fox-text").trim(),
  // The library folds the colours it was given into its own --fg/--bg aliases, so the test is
  // semantic rather than a string search: the diagram's own node text must paint with the
  // current Fox text token. Edge labels legitimately use the muted token, so "any" is right.
  paintUsesFoxToken: Array.from(document.querySelectorAll("#root svg text")).some((element) =>
    getComputedStyle(element).fill === getComputedStyle(document.documentElement).getPropertyValue("--fox-text").trim(),
  ),
});

report({ pending: true, case: kind, generation });
// Settle on a stability window: each block resolves independently.
let lastSignature = "";
let stableSince = Date.now();
let finished = false;
const finish = () => { if (finished) return; finished = true; clearInterval(timer); report(collect()); };
const timer = setInterval(() => {
  const signature = document.querySelectorAll("#root svg text").length + ":" + (root()?.textContent?.length ?? 0);
  if (signature !== lastSignature) { lastSignature = signature; stableSince = Date.now(); return; }
  if (Date.now() - stableSince > 1_500) finish();
}, 100);
setTimeout(finish, 15_000);
`

const CASES = [
  { name: 'flowchart-light', query: 'case=flowchart&theme=light', expectCase: 'flowchart', expectedDiagrams: 2 },
  { name: 'flowchart-dark', query: 'case=flowchart&theme=dark', expectCase: 'flowchart', expectedDiagrams: 2 },
  { name: 'pie-light', query: 'case=pie&theme=light', expectCase: 'pie', expectedDiagrams: 0 },
  { name: 'long-dark-narrow', query: 'case=long&theme=dark', expectCase: 'long', expectedDiagrams: 1, narrow: true },
]

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

async function waitFor(fn, { attempts = 60, delay = 400 } = {}) {
  for (let index = 0; index < attempts; index += 1) {
    try {
      const value = await fn()
      if (value) return value
    } catch { /* not ready */ }
    await sleep(delay)
  }
  return null
}

async function connect() {
  const target = await waitFor(async () => {
    const targets = await fetch(`http://127.0.0.1:${CDP_PORT}/json`).then((response) => response.json())
    return targets.find((entry) => entry.type === 'page' && entry.url.startsWith('http'))
  })
  if (!target) throw new Error('chrome devtools endpoint never exposed a page')
  const socket = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((resolve) => socket.addEventListener('open', resolve, { once: true }))
  let id = 0
  const pending = new Map()
  socket.addEventListener('message', (event) => {
    const message = JSON.parse(event.data)
    if (message.id && pending.has(message.id)) { pending.get(message.id)(message.result); pending.delete(message.id) }
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

/** Read one sample, but only once the page reports the case we asked for in a NEW generation. */
async function readSample(client, { expectCase, generation }) {
  const raw = await waitFor(async () => {
    const value = await client.evaluate('JSON.stringify(window.__mermaidCheck || null)')
    const parsed = value ? JSON.parse(value) : null
    if (!parsed || parsed.pending) return null
    if (parsed.case !== expectCase) return null
    if (parsed.generation === generation) return null
    return parsed
  }, { attempts: 45, delay: 400 })
  if (!raw) throw new Error(`case ${expectCase}: no fresh sample arrived`)
  return raw
}

const writeHarness = () => { writeFileSync(htmlPath, html); writeFileSync(tsxPath, tsx) }

const run = async () => {
  mkdirSync(OUT, { recursive: true })
  writeHarness()
  const vite = spawn('pnpm', ['exec', 'vite', '--host', '127.0.0.1', '--port', String(PORT), '--strictPort'], { cwd: desktopRoot, stdio: 'ignore', shell: true })
  const chrome = spawn(CHROME, [
    '--headless=new', `--remote-debugging-port=${CDP_PORT}`, '--no-first-run', '--no-default-browser-check',
    '--window-size=1200,900', `--user-data-dir=${join(OUT, 'chrome-profile')}`,
    `http://127.0.0.1:${PORT}/mermaid-browser-check.html?${CASES[0].query}`,
  ], { stdio: 'ignore' })

  const results = {}
  const failures = []
  let client = null
  try {
    if (!existsSync(CHROME)) throw new Error(`chrome not found at ${CHROME}`)
    client = await connect()
    let generation = null
    for (const testCase of CASES) {
      if (testCase.narrow) {
        await client.send('Emulation.setDeviceMetricsOverride', { width: 520, height: 820, deviceScaleFactor: 1, mobile: false })
      } else {
        await client.send('Emulation.clearDeviceMetricsOverride')
      }
      const url = `http://127.0.0.1:${PORT}/mermaid-browser-check.html?${testCase.query}`
      await client.send('Page.navigate', { url })
      const sample = await readSample(client, { expectCase: testCase.expectCase, generation })
      generation = sample.generation
      results[testCase.name] = sample
      const shot = await client.send('Page.captureScreenshot', { format: 'png' })
      if (shot?.data) writeFileSync(join(OUT, `mermaid-${testCase.name}.png`), Buffer.from(shot.data, 'base64'))
    }

    const prefixFor = (markers, suffixes) => {
      for (const id of markers) for (const suffix of suffixes) if (id.endsWith(`-${suffix}`)) return id.slice(0, -(suffix.length + 1))
      return null
    }

    for (const testCase of CASES) {
      const sample = results[testCase.name]
      const label = testCase.name
      if (!sample) { failures.push(`${label}: missing sample`); continue }
      if (sample.case !== testCase.expectCase) failures.push(`${label}: wrong case (${sample.case})`)
      if (!sample.proseBefore || !sample.proseAfter) failures.push(`${label}: surrounding prose lost`)
      // The stock engine paints out of its own theme, so the Fox-token assertion applies to the
      // beautiful-mermaid families only; the pie case is asserted by its rendered labels.
      if (testCase.expectCase !== 'pie' && !sample.paintUsesFoxToken) failures.push(`${label}: diagram text is not painted with the Fox text token (${sample.diagramFill} vs ${sample.foxTextToken})`)

      if (testCase.expectCase === 'flowchart') {
        const flowPrefix = prefixFor(sample.markers, ['arrowhead', 'arrowhead-start'])
        const seqPrefix = prefixFor(sample.markers, ['seq-arrow-open', 'seq-arrow'])
        if (sample.diagramTexts.filter((text) => text.length > 0).length < 2) failures.push(`${label}: expected two rendered diagrams`)
        if (!flowPrefix || !seqPrefix) failures.push(`${label}: marker ids missing (${JSON.stringify(sample.markers)})`)
        if (flowPrefix && seqPrefix && flowPrefix === seqPrefix) failures.push(`${label}: the two diagrams share one namespace`)
        if (new Set(sample.ids).size !== sample.ids.length) failures.push(`${label}: duplicate ids in one document`)
        if (!sample.hasStart || !sample.hasClient) failures.push(`${label}: a diagram lost its labels`)
      }

      if (testCase.expectCase === 'pie') {
        // Either the stock engine really drew the pie (its labels are inside the SVG), or the
        // notice UI is on screen. Prose alone proves neither, and an icon SVG is not a chart.
        const pieDrawn = sample.diagramTexts.some((text) => text.includes('甲') && text.includes('乙'))
        const noticeShown = typeof sample.notice === 'string' && sample.notice.length > 0
        if (!pieDrawn && !noticeShown) failures.push(`${label}: pie neither rendered (labels missing) nor reported (no notice)`)
        if (pieDrawn) results[`${label}-outcome`] = 'rendered by the stock engine'
        if (!pieDrawn && noticeShown) results[`${label}-outcome`] = `notice shown: ${sample.notice}`
      }

      if (testCase.expectCase === 'long') {
        if (!sample.hasLongLabel) failures.push(`${label}: the long Chinese label is missing from the diagram`)
        if (sample.rootScrollWidth > sample.viewport[0] + 1) failures.push(`${label}: the page overflows horizontally (${sample.rootScrollWidth} > ${sample.viewport[0]})`)
      }
    }

    const light = results['flowchart-light']
    const dark = results['flowchart-dark']
    if (light && dark && light.diagramFill === dark.diagramFill) failures.push(`theme: diagram text colour did not change between light and dark (${light.diagramFill})`)
    if (light && dark && light.foxTextToken === dark.foxTextToken) failures.push(`theme: --fox-text did not change between light and dark`)

    writeFileSync(join(OUT, 'mermaid-browser-check.json'), JSON.stringify({ results, failures }, null, 2))
    console.log(JSON.stringify({ results, failures }, null, 2))
    if (failures.length > 0) {
      console.error(`mermaid browser check FAILED:\n - ${failures.join('\n - ')}`)
      process.exitCode = 1
    } else {
      console.log('mermaid browser check OK (real browser, real Fox styles: diagrams render, ids are namespaced per block, prose intact, unsupported family non-fatal, light/dark differ, narrow window does not overflow)')
    }
  } finally {
    client?.close()
    chrome.kill()
    vite.kill()
    await sleep(500)
    for (const path of [htmlPath, tsxPath]) { try { rmSync(path, { force: true }) } catch { /* best effort */ } }
  }
}

run().catch((error) => {
  console.error(error.message)
  try { rmSync(htmlPath, { force: true }); rmSync(tsxPath, { force: true }) } catch { /* best effort */ }
  process.exitCode = 1
})
