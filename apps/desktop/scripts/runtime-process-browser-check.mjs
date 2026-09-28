// Real Chrome smoke test for the actual RuntimeTimeline and Fox styles.
// It uses an isolated headless profile and a temporary Vite page; no Tauri Host
// or model is involved. Run from the repository root with:
// node apps/desktop/scripts/runtime-process-browser-check.mjs
import { spawn } from 'node:child_process'
import { randomBytes } from 'node:crypto'
import { existsSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from 'node:fs'
import { createServer } from 'node:net'
import { join, resolve, sep } from 'node:path'
import { fileURLToPath } from 'node:url'

const desktopRoot = fileURLToPath(new URL('..', import.meta.url))
const repoRoot = fileURLToPath(new URL('../../..', import.meta.url))
const chromePath = process.env.CHROME_PATH || 'C:/Program Files/Google/Chrome/Application/chrome.exe'
const outputRoot = process.env.RUNTIME_PROCESS_CHECK_OUT || join(repoRoot, 'output', `runtime-process-browser-${Date.now()}`)
const id = randomBytes(6).toString('hex')
const htmlPath = join(desktopRoot, `runtime-process-browser-${id}.html`)
const tsxPath = join(desktopRoot, `runtime-process-browser-${id}.tsx`)
const route = `runtime-process-browser-${id}.html`
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))

const html = `<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"></head><body><div id="root"></div><script type="module" src="/runtime-process-browser-${id}.tsx"></script></body></html>`
const tsx = String.raw`
import { createRoot } from 'react-dom/client';
import './src/styles/globals.css';
import './src/styles/workbench.css';
import './src/styles/workspace-pages.css';
import { RuntimeTimeline } from './src/features/chat/workbench';
import { answerDeltaFingerprint } from './src/features/conversations/model/runtime-delta-fingerprint';

const kind = new URLSearchParams(location.search).get('case') || 'live';
const generation = Date.now().toString(36) + '-' + Math.random().toString(36).slice(2);
const page = window as unknown as { __processCheck?: { kind: string; generation: string } };
page.__processCheck = { kind, generation };
const events: any[] = [];
let seq = 0;
const add = (eventType: string, event: Record<string, unknown>) => {
  events.push({ runId: 'run', seq: ++seq, eventType, event: { type: eventType, ...event }, createdAt: seq });
};
const tools = (prefix: string, activeLast: boolean, failedLast: boolean) => {
  for (let index = 0; index < 20; index++) {
    const toolCallId = prefix + '-' + index;
    add('tool.started', { toolCallId, tool: 'read', input: { path: prefix + '-' + index + '.txt' } });
    if (!activeLast || index < 19) add('tool.completed', {
      toolCallId, tool: 'read', result: failedLast && index === 19 ? { error: 'permission denied' } : { content: 'ok' },
      isError: failedLast && index === 19,
    });
  }
};
const answer = (text: string) => add('message.delta', { deltaLength: text.length, deltaFingerprint: answerDeltaFingerprint(text) });
tools('first', false, false);
answer('第一段');
tools('second', kind === 'live', kind === 'failed');
answer('第二段');
const running = kind === 'live';
const messages: any[] = [
  { id: 'user', conversationId: 'conversation', runId: 'run', role: 'user', kind: 'text', content: '验证过程组织', status: 'completed', ordinal: 1, createdAt: 1, updatedAt: 1 },
  { id: 'assistant-run', conversationId: 'conversation', runId: 'run', role: 'assistant', kind: 'text', content: '第一段第二段', status: running ? 'streaming' : 'completed', ordinal: 2, createdAt: 2, updatedAt: 2 },
];
createRoot(document.getElementById('root') as HTMLElement).render(
  <main style={{ width: 'min(800px, calc(100vw - 40px))', margin: '20px auto' }}>
    <RuntimeTimeline messages={messages} attachments={[]} artifacts={[]} events={events}
      activeRunId="run" runtimeRunning={running} state={running ? 'streaming' : 'idle'} streamingText=""
      onRetry={() => {}} onRerun={async () => false} />
  </main>
);
`

async function freePort() {
  const server = createServer()
  await new Promise((resolve, reject) => server.once('error', reject).listen(0, '127.0.0.1', resolve))
  const port = server.address().port
  await new Promise(resolve => server.close(resolve))
  return port
}

async function waitFor(fn, attempts = 100) {
  for (let attempt = 0; attempt < attempts; attempt++) {
    try {
      const result = await fn()
      if (result) return result
    } catch { /* server or page is not ready */ }
    await sleep(150)
  }
  throw new Error('timed out waiting for fresh browser state')
}

async function connect(cdpPort, vitePort) {
  const target = await waitFor(async () => {
    const pages = await fetch(`http://127.0.0.1:${cdpPort}/json`, { signal: AbortSignal.timeout(1500) }).then(response => response.json())
    return pages.find(page => page.type === 'page' && page.url.startsWith(`http://127.0.0.1:${vitePort}/`))
  })
  const socket = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((resolve, reject) => {
    socket.addEventListener('open', resolve, { once: true })
    socket.addEventListener('error', reject, { once: true })
  })
  let nextId = 0
  const pending = new Map()
  const rejectPending = reason => { for (const entry of pending.values()) entry.reject(new Error(reason)); pending.clear() }
  socket.addEventListener('close', () => rejectPending('Chrome DevTools connection closed'))
  socket.addEventListener('error', () => rejectPending('Chrome DevTools connection failed'))
  socket.addEventListener('message', event => {
    const message = JSON.parse(event.data)
    const entry = pending.get(message.id)
    if (entry) {
      pending.delete(message.id)
      message.error ? entry.reject(new Error(message.error.message)) : entry.resolve(message.result)
    }
  })
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const messageId = ++nextId
    const timeout = setTimeout(() => { pending.delete(messageId); reject(new Error(`CDP ${method} timed out`)) }, 10_000)
    pending.set(messageId, { resolve: value => { clearTimeout(timeout); resolve(value) }, reject: error => { clearTimeout(timeout); reject(error) } })
    socket.send(JSON.stringify({ id: messageId, method, params }))
  })
  const evaluate = async expression => {
    const reply = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })
    if (reply.exceptionDetails) throw new Error(reply.exceptionDetails.text)
    return reply.result?.value
  }
  await send('Runtime.enable')
  await send('Page.enable')
  return { socket, send, evaluate }
}

const click = (client, selector, index = 0) => client.evaluate(`(() => { const node = document.querySelectorAll(${JSON.stringify(selector)})[${index}]; if (!node) return false; node.click(); return true })()`)
const metrics = client => client.evaluate(`(() => {
  const stages = [...document.querySelectorAll('.fox-process-stage')];
  const headers = stages.map(stage => stage.querySelector('.fox-chain-of-thought-header'));
  const scrolls = stages.map(stage => stage.querySelector('.fox-runtime-process-scroll'));
  return {
    generation: window.__processCheck?.generation, kind: window.__processCheck?.kind,
    stageCount: stages.length, answerCount: document.querySelectorAll('.fox-answer-segment').length,
    answers: [...document.querySelectorAll('.fox-answer-segment')].map(node => node.textContent.trim()),
    titles: stages.map(stage => stage.querySelector('.fox-runtime-process-summary')?.textContent?.trim()),
    open: headers.map(header => header?.getAttribute('aria-expanded')),
    hidden: stages.map(stage => stage.hidden),
    scrolls: scrolls.map(node => node ? { top: node.scrollTop, height: node.scrollHeight, client: node.clientHeight, overflow: getComputedStyle(node).overflowY } : null),
    toggle: document.querySelector('.fox-process-turn-toggle')?.textContent?.trim(),
  };
})()`)

async function ready(client, kind, priorGeneration) {
  return waitFor(async () => {
    const sample = await metrics(client)
    return sample?.kind === kind && sample.generation !== priorGeneration && sample.stageCount === 2 && sample.answerCount === 2 ? sample : null
  })
}

function check(condition, detail) { if (!condition) throw new Error(detail) }
function scrollAtEnd(scroll) { return scroll && scroll.height > scroll.client && scroll.top > 0 && Math.abs(scroll.top - (scroll.height - scroll.client)) < 3 }
function safeProfileRemove(profile, output) {
  const parent = resolve(output)
  const target = resolve(profile)
  check(target.startsWith(parent + sep), `refusing to delete profile outside evidence directory: ${target}`)
  rmSync(target, { recursive: true, force: true })
}

async function run() {
  check(existsSync(chromePath), `Chrome missing: ${chromePath}`)
  mkdirSync(outputRoot, { recursive: true })
  const profile = mkdtempSync(join(outputRoot, 'chrome-profile-'))
  writeFileSync(htmlPath, html, { flag: 'wx' })
  writeFileSync(tsxPath, tsx, { flag: 'wx' })
  const vitePort = await freePort()
  const cdpPort = await freePort()
  console.log(`checking RuntimeTimeline via isolated Vite ${vitePort} / Chrome CDP ${cdpPort}`)
  const viteLog = []
  const vite = spawn(process.execPath, [join(desktopRoot, 'node_modules', 'vite', 'bin', 'vite.js'), '--host', '127.0.0.1', '--port', String(vitePort), '--strictPort'], {
    cwd: desktopRoot, windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'],
  })
  for (const stream of [vite.stdout, vite.stderr]) stream.on('data', chunk => { viteLog.push(String(chunk)); if (viteLog.length > 20) viteLog.shift() })
  let chrome = null
  const chromeLog = []
  let client = null
  const results = {}
  try {
    await waitFor(async () => {
      const response = await fetch(`http://127.0.0.1:${vitePort}/${route}?case=live`, { signal: AbortSignal.timeout(1500) })
      return response.ok
    })
    console.log('Vite fixture ready')
    chrome = spawn(chromePath, [
      '--headless=new', '--disable-gpu', '--disable-gpu-compositing', `--remote-debugging-port=${cdpPort}`, '--no-first-run', '--no-default-browser-check',
      '--window-size=1200,900', `--user-data-dir=${profile}`,
      `http://127.0.0.1:${vitePort}/${route}?case=live`,
    ], { windowsHide: true, stdio: ['ignore', 'pipe', 'pipe'] })
    for (const stream of [chrome.stdout, chrome.stderr]) stream.on('data', chunk => { chromeLog.push(String(chunk)); if (chromeLog.length > 20) chromeLog.shift() })
    client = await connect(cdpPort, vitePort)
    console.log('Chrome CDP connected')
    let live = await ready(client, 'live', null)
    check(live.answers[0].includes('第一段') && live.answers[1].includes('第二段'), 'answer segments lost or reordered')
    check(live.titles[1].includes('正在读取文件'), 'tool remains in flight after answer but stage is not running')
    check(await click(client, '.fox-process-stage .fox-chain-of-thought-header', 1), 'missing live stage header')
    live = await waitFor(async () => { const sample = await metrics(client); return sample.open[1] === 'true' && sample.scrolls[1] ? sample : null })
    check(scrollAtEnd(live.scrolls[1]), `live stage did not open at end: ${JSON.stringify(live.scrolls[1])}`)
    check(await click(client, '.fox-process-stage .fox-chain-of-thought-header', 0), 'missing completed stage header')
    let first = await waitFor(async () => { const sample = await metrics(client); return sample.open[0] === 'true' && sample.scrolls[0] ? sample : null })
    check(first.scrolls[0].height > first.scrolls[0].client && first.scrolls[0].top === 0, `completed stage did not open at top: ${JSON.stringify(first.scrolls[0])}`)
    await client.evaluate(`document.querySelectorAll('.fox-process-stage .fox-runtime-process-scroll')[0].scrollTop = 160`)
    await click(client, '.fox-process-stage .fox-chain-of-thought-header', 0)
    await click(client, '.fox-process-stage .fox-chain-of-thought-header', 0)
    first = await metrics(client)
    check(first.scrolls[0].top === 0, 'completed stage did not reset to top on reopen')
    await click(client, '.fox-process-turn-toggle')
    const collapsed = await metrics(client)
    check(collapsed.hidden.every(Boolean) && collapsed.answerCount === 2, 'collapsing process hid answer or left process rows visible')
    await click(client, '.fox-process-turn-toggle')
    const restored = await metrics(client)
    check(restored.open[1] === 'true' && restored.answerCount === 2, 'reopening process lost expanded stage or answer')
    results.live = { initial: live, completedStage: first, collapsed, restored }
    const liveShot = await client.send('Page.captureScreenshot', { format: 'png' })
    if (liveShot?.data) writeFileSync(join(outputRoot, 'live.png'), Buffer.from(liveShot.data, 'base64'))

    let generation = live.generation
    await client.send('Page.navigate', { url: `http://127.0.0.1:${vitePort}/${route}?case=completed` })
    const completed = await ready(client, 'completed', generation)
    generation = completed.generation
    await click(client, '.fox-process-stage .fox-chain-of-thought-header', 1)
    const completedOpen = await waitFor(async () => { const sample = await metrics(client); return sample.open[1] === 'true' && sample.scrolls[1] ? sample : null })
    check(completedOpen.scrolls[1].height > completedOpen.scrolls[1].client && completedOpen.scrolls[1].top === 0, 'finished stage did not open from beginning')
    results.completed = completedOpen
    const completedShot = await client.send('Page.captureScreenshot', { format: 'png' })
    if (completedShot?.data) writeFileSync(join(outputRoot, 'completed.png'), Buffer.from(completedShot.data, 'base64'))

    await client.send('Page.navigate', { url: `http://127.0.0.1:${vitePort}/${route}?case=failed` })
    const failed = await ready(client, 'failed', generation)
    check(failed.titles[1].includes('失败'), `failed tool hidden from stage title: ${failed.titles[1]}`)
    await click(client, '.fox-process-turn-toggle')
    const failedCollapsed = await metrics(client)
    check(failedCollapsed.toggle.includes('失败') && failedCollapsed.answerCount === 2 && failedCollapsed.hidden.every(Boolean), 'failed tool hidden from whole-turn summary or answer lost')
    results.failed = failedCollapsed
    const failedShot = await client.send('Page.captureScreenshot', { format: 'png' })
    if (failedShot?.data) writeFileSync(join(outputRoot, 'failed.png'), Buffer.from(failedShot.data, 'base64'))
    writeFileSync(join(outputRoot, 'runtime-process-browser-check.json'), JSON.stringify({ results, vitePort, cdpPort }, null, 2))
    console.log(`runtime-process browser check OK: ${outputRoot}`)
    console.log('real RuntimeTimeline + Fox CSS: live end, completed top, collapse state, response-after-active-tool, partial failure')
  } catch (error) {
    writeFileSync(join(outputRoot, 'runtime-process-browser-error.txt'), `${error.stack || error}\n\nVite output:\n${viteLog.join('')}\nChrome output:\n${chromeLog.join('')}`)
    throw error
  } finally {
    try { await Promise.race([client?.send('Browser.close'), sleep(500)]) } catch { /* close our own Chrome below */ }
    client?.socket.close()
    chrome?.kill()
    vite.kill()
    await sleep(400)
    for (const path of [htmlPath, tsxPath]) rmSync(path, { force: true })
    safeProfileRemove(profile, outputRoot)
  }
}

run().catch(error => { console.error(error.stack || error); process.exitCode = 1 })
