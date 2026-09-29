// Real Chromium interaction check for Fox's Streamdown table and Mermaid controls.
// Uses Fox CSS and the real MessageResponse; only the Tauri host/model and clipboard
// permission are substituted. Pointer clicks go through CDP hit testing.
import { spawn } from 'node:child_process'
import { existsSync, mkdirSync, readFileSync, rmSync, writeFileSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const desktop = fileURLToPath(new URL('..', import.meta.url))
const output = fileURLToPath(new URL('../../../output/message-controls-browser/', import.meta.url))
const htmlPath = join(desktop, 'message-controls-check.html')
const entryPath = join(desktop, 'message-controls-check.tsx')
const port = 1461
const cdpPort = 9363
const chromePath = process.env.CHROME_PATH || 'C:/Program Files/Google/Chrome/Application/chrome.exe'
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))

const html = '<!doctype html><html><head><meta charset="utf-8"></head><body><div id="root"></div><script type="module" src="/message-controls-check.tsx"></script></body></html>'
const entry = `import React from 'react';
import { createRoot } from 'react-dom/client';
import './src/styles/globals.css';
import './src/styles/workbench.css';
import './src/styles/workspace-pages.css';
import { MessageResponse } from './src/components/ai-elements/message-response';

const marker = String.fromCharCode(96).repeat(3);
const markdown = [
  '## 交付成果',
  '| 序号 | 文件 | 位置 | 说明 |',
  '| --- | --- | --- | --- |',
  '| 1 | AGV长时间任务原始记录表.xlsx | 项目根目录 | 原始记录表，2214 行 × 14 列 |',
  '| 2 | AGV长时间任务汇总统计表.xlsx | 项目根目录 | 新建，含 11 工作表 + 7 张图表 |',
  '',
  '## 流程图',
  marker + 'mermaid\\nflowchart TD\\n  A[请求入口] --> B[任务执行]\\n' + marker,
].join('\\n');
Object.defineProperty(navigator, 'clipboard', { configurable: true, value: {
  write: async items => { const blob = await items[0].getType('text/plain'); window.__copied = await blob.text(); },
  writeText: async text => { window.__copied = text; },
} });
const createObjectURL = URL.createObjectURL.bind(URL);
URL.createObjectURL = blob => { window.__lastDownloadBlob = blob; return createObjectURL(blob); };
const anchorClick = HTMLAnchorElement.prototype.click;
HTMLAnchorElement.prototype.click = function() {
  if (this.download && window.__lastDownloadBlob) {
    const name = this.download;
    window.__lastDownloadBlob.text().then(text => (window.__downloads ||= []).push({ name, text }));
  }
  return anchorClick.call(this);
};
document.addEventListener('click', event => (window.__clickTrace ||= []).push({ title:event.target.closest?.('button')?.title, text:event.target.textContent?.slice(0,30) }), true);
document.body.style.margin = '0';
createRoot(document.getElementById('root')!).render(
  <div id="shell" style={{display:'flex',height:'100vh',minWidth:0}}>
    <aside id="sidebar" style={{flex:'none',width:230,background:'var(--fox-sidebar)',borderRight:'1px solid var(--fox-border)',padding:12}}>
      <button id="sidebar-toggle" onClick={event => {
        const side = event.currentTarget.parentElement!;
        side.style.width = side.style.width === '330px' ? '230px' : '330px';
        window.__sidebarClicks = (window.__sidebarClicks || 0) + 1;
      }}>调整侧边栏</button>
    </aside>
    <div className="fox-content-card"><div className="fox-content-surface" style={{flex:'1 1 0',width:'auto',minWidth:0}}>
      <section className="fox-chat-pane" style={{minWidth:0}}>
        <div className="fox-chat-topbar">对话</div>
        <div style={{flex:1,minHeight:0,overflow:'auto',padding:'14px 20px'}}>
          <div className="fox-answer-body"><MessageResponse className="fox-answer-response">{markdown}</MessageResponse></div>
        </div>
      </section>
    </div></div>
  </div>
);
window.__ready = true;
`

async function connect() {
  let target
  for (let i = 0; i < 100; i++) {
    try {
      target = (await fetch(`http://127.0.0.1:${cdpPort}/json`).then(response => response.json()))
        .find(item => item.type === 'page' && item.url.includes('message-controls-check.html'))
      if (target) break
    } catch { /* Vite and Chrome are still starting. */ }
    await sleep(150)
  }
  if (!target) throw new Error('Chromium page did not start')
  const socket = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise(resolve => socket.addEventListener('open', resolve, { once: true }))
  let id = 0
  const pending = new Map()
  const events = []
  socket.addEventListener('message', event => {
    const response = JSON.parse(event.data)
    if (response.method === 'Runtime.exceptionThrown' || response.method === 'Log.entryAdded') events.push(response)
    if (response.id && pending.has(response.id)) {
      pending.get(response.id)(response)
      pending.delete(response.id)
    }
  })
  const send = (method, params = {}) => new Promise(resolve => {
    const next = ++id
    pending.set(next, resolve)
    socket.send(JSON.stringify({ id: next, method, params }))
  })
  const evaluate = async expression => {
    const response = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })
    if (response.error || response.result?.exceptionDetails) throw new Error(JSON.stringify(response))
    return response.result?.result?.value
  }
  await send('Runtime.enable')
  await send('Log.enable')
  await send('Page.enable')
  return { send, evaluate, events, close: () => socket.close() }
}

async function waitFor(predicate, label, attempts = 80) {
  for (let i = 0; i < attempts; i++) {
    const value = await predicate()
    if (value) return value
    await sleep(150)
  }
  throw new Error(`Timed out: ${label}`)
}

async function run() {
  if (!existsSync(chromePath)) throw new Error(`Chrome missing: ${chromePath}`)
  mkdirSync(output, { recursive: true })
  writeFileSync(htmlPath, html)
  writeFileSync(entryPath, entry)
  const vite = spawn(process.execPath, [join(desktop, 'node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { cwd: desktop, stdio: ['ignore', 'pipe', 'pipe'] })
  let viteOutput = ''
  vite.stdout.on('data', data => { viteOutput += data.toString() })
  vite.stderr.on('data', data => { viteOutput += data.toString() })
  await waitFor(async () => {
    try { return (await fetch(`http://127.0.0.1:${port}/message-controls-check.html`)).ok } catch { return false }
  }, 'Vite server')
  const chrome = spawn(chromePath, ['--headless=new', '--no-first-run', '--no-default-browser-check', `--remote-debugging-port=${cdpPort}`, '--window-size=1180,830', `--user-data-dir=${join(output, 'chrome-profile')}`, `http://127.0.0.1:${port}/message-controls-check.html`], { stdio: 'ignore' })
  let client
  const observations = {}
  const failures = []
  const check = (condition, label) => { if (!condition) failures.push(label) }
  try {
    client = await connect()
    const evaluate = client.evaluate
    await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/message-controls-check.html` })
    const pointer = async selector => {
      const target = await evaluate(`(() => {
        const element = document.querySelector(${JSON.stringify(selector)});
        if (!element) return null;
        const rect = element.getBoundingClientRect();
        const x = rect.left + rect.width / 2, y = rect.top + rect.height / 2;
        const hit = document.elementFromPoint(x, y);
        return { x, y, width: rect.width, height: rect.height,
          hit: hit === element || element.contains(hit), hitTag: hit?.tagName,
          hitTitle: hit?.closest('button')?.title || '' };
      })()`)
      if (!target) throw new Error(`Missing target: ${selector}`)
      if (!target.hit) failures.push(`${selector}: pointer center is obstructed by ${target.hitTag}/${target.hitTitle}`)
      await client.send('Input.dispatchMouseEvent', { type: 'mousePressed', x: target.x, y: target.y, button: 'left', clickCount: 1 })
      await client.send('Input.dispatchMouseEvent', { type: 'mouseReleased', x: target.x, y: target.y, button: 'left', clickCount: 1 })
      return target
    }
    const bounds = async selector => evaluate(`(() => {
      const r = document.querySelector(${JSON.stringify(selector)})?.getBoundingClientRect();
      return r ? {left:r.left,top:r.top,right:r.right,bottom:r.bottom,width:r.width,height:r.height} : null;
    })()`)
    const screenshot = async name => {
      const response = await client.send('Page.captureScreenshot', { format: 'png' })
      if (response.result?.data) writeFileSync(join(output, name), Buffer.from(response.result.data, 'base64'))
    }
    const within = (inner, outer) => inner && outer && inner.left >= outer.left - 2 && inner.top >= outer.top - 2 && inner.right <= outer.right + 2 && inner.bottom <= outer.bottom + 2
    const menu = 'div[data-streamdown="table-wrapper"] > div:first-child > div > div'

    try {
      await waitFor(async () => evaluate('!!window.__ready && !!document.querySelector("[data-streamdown=mermaid-block] [aria-label=\\"Mermaid chart\\"]")'), 'Mermaid render', 250)
    } catch (error) {
      const sourceResponse = await fetch(`http://127.0.0.1:${port}/message-controls-check.tsx`)
      console.error(await evaluate('({ ready:window.__ready, state:document.readyState, resources:performance.getEntriesByType("resource").map(e=>e.name).slice(-20), body:document.body.innerHTML.slice(0,3000), text:document.body.textContent?.slice(0,500) })'), viteOutput.slice(-3000), client.events, sourceResponse.status, (await sourceResponse.text()).slice(0,1000))
      throw error
    }
    observations.tableInitial = await bounds('[data-streamdown="table-wrapper"] table')
    observations.tableShell = await bounds('[data-streamdown="table-wrapper"]')
    check(observations.tableInitial.width > 500, 'table collapsed inside wrapper')

    await pointer('[data-streamdown="table-wrapper"] button[title="Copy table"]')
    await waitFor(async () => evaluate(`!!document.querySelector(${JSON.stringify(menu)})`), 'copy menu')
    await screenshot('table-copy-menu.png')
    observations.copyMenu = await evaluate(`(() => {const e=document.querySelector(${JSON.stringify(menu)});const r=e.getBoundingClientRect();const b=e.querySelector('button').getBoundingClientRect();return {width:r.width, firstHeight:b.height, text:e.textContent}})()`)
    check(observations.copyMenu.width >= 118 && observations.copyMenu.firstHeight < 50, 'copy menu wraps vertically')
    await pointer(`${menu} button[title="Copy table as Markdown"]`)
    observations.tableCopied = await waitFor(async () => evaluate('window.__copied'), 'table clipboard')
    check(observations.tableCopied.includes('AGV长时间任务原始记录表.xlsx'), 'table copy content wrong')

    await pointer('[data-streamdown="table-wrapper"] button[title="Download table"]')
    await waitFor(async () => evaluate(`!!document.querySelector(${JSON.stringify(menu)})`), 'download menu')
    await client.send('Page.setDownloadBehavior', { behavior: 'allow', downloadPath: output })
    await pointer(`${menu} button[title="Download table as CSV"]`)
    await waitFor(async () => existsSync(join(output, 'table.csv')), 'CSV download')
    observations.csv = readFileSync(join(output, 'table.csv'), 'utf8').slice(0, 130)
    check(observations.csv.includes('AGV长时间任务原始记录表.xlsx'), 'CSV file content wrong')

    const title = await bounds('[data-streamdown="mermaid-block"] > div:first-child')
    const actions = await bounds('[data-streamdown="mermaid-block-actions"]')
    observations.mermaidHeader = { title, actions }
    check(Math.abs((title.top + title.bottom) / 2 - (actions.top + actions.bottom) / 2) < 8, 'Mermaid title and actions are on separate rows')
    await pointer('[data-streamdown="mermaid-block-actions"] button[title="Copy Code"]')
    observations.mermaidCopied = await waitFor(async () => evaluate('window.__copied?.startsWith("flowchart TD") ? window.__copied : null'), 'Mermaid clipboard')
    await pointer('[data-streamdown="mermaid-block-actions"] button[title="Download diagram"]')
    await waitFor(async () => evaluate('!!document.querySelector("[data-streamdown=mermaid-block-actions] > div > div")'), 'diagram download menu')
    await screenshot('mermaid-download-menu.png')
    observations.mmdMenuLayout = await evaluate(`(() => {
      const e = document.querySelector('[data-streamdown=mermaid-block-actions] > div > div');
      const p = e.parentElement, a = p.parentElement;
      const r = n => {const b=n.getBoundingClientRect(), s=getComputedStyle(n);return {x:b.x,y:b.y,width:b.width,height:b.height,position:s.position,right:s.right,display:s.display}};
      return {menu:r(e),parent:r(p),actions:r(a)};
    })()`)
    observations.mmdPointer = await pointer('[data-streamdown="mermaid-block-actions"] button[title="Download diagram as MMD"]')
    try {
      observations.mmdDownload = await waitFor(async () => evaluate('window.__downloads?.find(item => item.name === "diagram.mmd")'), 'MMD download callback', 10)
    } catch (error) {
      console.error(JSON.stringify({pointer:observations.mmdPointer, state:await evaluate('({ downloads:window.__downloads, lastBlob:window.__lastDownloadBlob?.size, clicks:window.__clickTrace?.slice(-8), menu:document.querySelector("[data-streamdown=mermaid-block-actions]")?.outerHTML.slice(0,2500), copied:window.__copied?.slice(0,80) })'), events:client.events}, null, 2))
      failures.push('MMD download callback not invoked')
    }
    if (observations.mmdDownload) check(observations.mmdDownload.text.includes('请求入口'), 'MMD Blob content wrong')

    await pointer('[data-streamdown="table-wrapper"] button[title="View fullscreen"]')
    await waitFor(async () => evaluate('!!document.querySelector(".fox-chat-local-fullscreen[data-fox-streamdown=table-fullscreen]")'), 'table local fullscreen')
    observations.tableFullscreen = await bounds('.fox-chat-local-fullscreen')
    observations.tableCorners = await evaluate('({surface:getComputedStyle(document.querySelector(".fox-content-surface")).borderRadius,overlay:getComputedStyle(document.querySelector(".fox-chat-local-fullscreen")).borderRadius})')
    check(observations.tableCorners.overlay === observations.tableCorners.surface && observations.tableCorners.overlay !== '0px', 'table fullscreen loses conversation corners')
    await screenshot('table-local-fullscreen.png')
    observations.pane = await bounds('.fox-chat-pane')
    check(within(observations.tableFullscreen, observations.pane), 'table fullscreen exceeds chat pane')
    await pointer('#sidebar-toggle')
    observations.resizedPane = await bounds('.fox-chat-pane')
    await waitFor(async () => {
      const r = await bounds('.fox-chat-local-fullscreen')
      return Math.abs(r.left - observations.resizedPane.left) < 2 && Math.abs(r.width - observations.resizedPane.width) < 2
    }, 'fullscreen follows sidebar resize')
    observations.sidebarClicks = await evaluate('window.__sidebarClicks')
    check(observations.sidebarClicks === 1, 'sidebar blocked by local fullscreen')
    await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
    await waitFor(async () => evaluate('!document.querySelector(".fox-chat-local-fullscreen")'), 'table Escape close')
    observations.tableRestoredFocus = await evaluate('document.activeElement?.title')
    check(observations.tableRestoredFocus === 'View fullscreen', 'table focus did not restore')

    await pointer('[data-streamdown="mermaid-block-actions"] button[title="View fullscreen"]')
    await waitFor(async () => evaluate('!!document.querySelector(".fox-chat-local-fullscreen [aria-label=\\"Mermaid chart\\"]")'), 'Mermaid local fullscreen')
    try {
      await waitFor(async () => evaluate('!!document.querySelector(".fox-chat-local-fullscreen [aria-label=\\"Mermaid chart\\"] svg text")'), 'Mermaid fullscreen SVG text', 60)
    } catch (error) {
      console.error('Mermaid fullscreen diagnostic', await evaluate(`(() => {
        const root=document.querySelector('.fox-chat-local-fullscreen');
        return {html:root?.innerHTML.slice(0,1800),text:root?.textContent?.slice(0,300),
          nodes:[...root?.querySelectorAll('[data-streamdown=mermaid], [role=application], [aria-label="Mermaid chart"], svg')||[]].map(e=>{const r=e.getBoundingClientRect();return {tag:e.tagName,role:e.getAttribute('role'),label:e.getAttribute('aria-label'),width:r.width,height:r.height,display:getComputedStyle(e).display}})};
      })()`))
      throw error
    }
    observations.mermaidDiagram = await evaluate(`(() => {
      const svg=document.querySelector('.fox-chat-local-fullscreen [aria-label="Mermaid chart"] svg');
      const r=svg.getBoundingClientRect();
      return {text:Array.from(svg.querySelectorAll('text')).map(node => node.textContent).join(' '),width:r.width,height:r.height,left:r.left,top:r.top,right:r.right,bottom:r.bottom};
    })()`)
    observations.mermaidFullscreen = await bounds('.fox-chat-local-fullscreen')
    observations.mermaidCorners = await evaluate('({surface:getComputedStyle(document.querySelector(".fox-content-surface")).borderRadius,overlay:getComputedStyle(document.querySelector(".fox-chat-local-fullscreen")).borderRadius})')
    check(observations.mermaidCorners.overlay === observations.mermaidCorners.surface && observations.mermaidCorners.overlay !== '0px', 'Mermaid fullscreen loses conversation corners')
    check(observations.mermaidDiagram.text.includes('请求入口') && observations.mermaidDiagram.text.includes('任务执行'), 'Mermaid fullscreen labels missing')
    check(observations.mermaidDiagram.width > 20 && observations.mermaidDiagram.height > 20 && within(observations.mermaidDiagram, observations.mermaidFullscreen), 'Mermaid fullscreen SVG hidden or outside pane')
    await screenshot('mermaid-local-fullscreen.png')
    observations.paneAfterResize = await bounds('.fox-chat-pane')
    check(within(observations.mermaidFullscreen, observations.paneAfterResize), 'Mermaid fullscreen exceeds chat pane')
    await pointer('#sidebar-toggle')
    observations.mermaidPaneResized = await bounds('.fox-chat-pane')
    await waitFor(async () => {
      const r = await bounds('.fox-chat-local-fullscreen')
      return Math.abs(r.left - observations.mermaidPaneResized.left) < 2 && Math.abs(r.width - observations.mermaidPaneResized.width) < 2
    }, 'Mermaid fullscreen follows sidebar resize')
    await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
    await waitFor(async () => evaluate('!document.querySelector(".fox-chat-local-fullscreen")'), 'Mermaid Escape close')
    observations.mermaidRestoredFocus = await evaluate('document.activeElement?.title')
    check(observations.mermaidRestoredFocus === 'View fullscreen', 'Mermaid focus did not restore')

    await screenshot('message-controls.png')
    writeFileSync(join(output, 'message-controls-browser-check.json'), JSON.stringify({ observations, failures }, null, 2))
    console.log(JSON.stringify({ observations, failures }, null, 2))
    if (failures.length) process.exitCode = 1
  } finally {
    client?.close()
    chrome.kill()
    vite.kill()
    await sleep(500)
    for (const path of [htmlPath, entryPath]) { try { rmSync(path, { force: true }) } catch { /* best effort */ } }
  }
}

run().catch(error => {
  console.error(error)
  try { rmSync(htmlPath, { force: true }); rmSync(entryPath, { force: true }) } catch { /* best effort */ }
  process.exitCode = 1
})
