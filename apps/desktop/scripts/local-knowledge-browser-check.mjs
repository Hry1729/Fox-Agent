// Local knowledge pages in a real browser: Fox components, Fox themes, mock gateway.
// This is not a live Host/model test; it checks copy, layout and theme contrast.
import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync, rmSync } from 'node:fs'
import { join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = fileURLToPath(new URL('..', import.meta.url))
const output = fileURLToPath(new URL('../../../output/local-knowledge-browser/', import.meta.url))
const htmlPath = join(root, 'local-knowledge-check.html')
const entryPath = join(root, 'local-knowledge-check.tsx')
const port = 1458, cdpPort = 9358
const sleep = ms => new Promise(resolve => setTimeout(resolve, ms))

// Copy contract from docs/03-产品与前端/知识库重设计: professional words become
// everyday words. The scan reports what each mounted page still shows, so the
// list shrinks to empty as the redesign steps land.
const banned = ['已就绪', '索引中', '待重建', '已索引', '解析中', '关键词检索', '向量检索', '混合检索', '检索测试', '分块', '向量化', '文档导入', '文档解析', '索引构建', '索引重建', '文档删除', '隔离测试', '重新解析', '生成向量', '切分文本', '提交代次', '校验索引', '检索模式', '个文档', '构建索引', '向量索引', '解析文档', '解析进度', '导入文档', '知识库文档', '文档预览']

const source = `import React from 'react';
import { createRoot } from 'react-dom/client';
import './src/styles/globals.css';
import './src/styles/workbench.css';
import './src/styles/workspace-pages.css';
import { applyColorTheme } from './src/features/settings/color-theme';
import { createMockLocalKnowledgeGateway } from './src/features/local-knowledge/mock-gateway';
import { LocalKnowledgeWorkspace } from './src/features/local-knowledge/local-knowledge-pages';
const query = new URLSearchParams(location.search);
if(query.get('dark')==='1') document.documentElement.classList.add('dark');
applyColorTheme(query.get('theme') || 'graphite');
document.body.style.margin = '0';
const mock = createMockLocalKnowledgeGateway();
// The shared mock gateway has no chunk store, so retrieval is stubbed with
// fixture snippets. This checks hit layout, marking and ordering only.
const gateway: any = {
  ...mock,
  testRetrieval: async (knowledgeBaseId: string, query: string, mode: string, topK?: number) => {
    const page = await mock.listDocuments(knowledgeBaseId);
    const term = query.trim();
    return {
      knowledgeBaseId, query, mode, fusionVersion: null,
      timings: { keywordMs: 1, vectorMs: 0, totalMs: 1 },
      items: page.items.slice(0, topK ?? 10).map((doc: any, index: number) => ({
        documentId: doc.id,
        documentName: doc.name,
        relativePath: doc.relativePath,
        chunkId: doc.id + '-c0',
        anchor: '第 ' + (index + 1) + ' 段',
        content: '这是「' + doc.name + '」中与「' + term + '」相关的一段示例正文，用来检查命中高亮、排序与引用样式是否符合重设计稿。',
        score: 1 - index * 0.1,
        lexicalScore: 1 - index * 0.1,
        vectorScore: null,
      })),
    };
  },
  // The seeded mock has one running and one partial job, so finished jobs —
  // the history list — would never render. Three fixture rows cover the
  // one-line layout, the warning dot and an older day group.
  listJobs: async (knowledgeBaseId?: string) => {
    const jobs = await mock.listJobs(knowledgeBaseId);
    const stamp = (hoursAgo: number) => new Date(Date.now() - hoursAgo * 3600_000).toISOString();
    const history = [
      { id: 'job-h-today', operationId: 'operation-h-today', knowledgeBaseId: 'local-product-docs', type: 'index', completedItems: 24, failedItems: 0, totalItems: 24, hours: 2 },
      { id: 'job-h-old', operationId: 'operation-h-old', knowledgeBaseId: 'local-research-notes', type: 'delete', completedItems: 3, failedItems: 0, totalItems: 3, hours: 80 },
      { id: 'job-h-week', operationId: 'operation-h-week', knowledgeBaseId: 'local-product-docs', type: 'rebuild', completedItems: 41, failedItems: 0, totalItems: 41, hours: 300 },
    ].map((item) => ({
      id: item.id, operationId: item.operationId, knowledgeBaseId: item.knowledgeBaseId, type: item.type,
      status: 'completed', stage: 'committing', progress: 100, lastSequence: 5, outcome: null,
      completedItems: item.completedItems, failedItems: item.failedItems, totalItems: item.totalItems,
      heartbeatAt: null, error: null, createdAt: stamp(item.hours + 1), updatedAt: stamp(item.hours),
    })) as any[];
    return [...jobs, ...history];
  },
  // Neither seeded model is installed, so the hero card would never render.
  // One ready default plus one mid-download row covers both layouts.
  listEmbeddingModels: async () => {
    const list = await mock.listEmbeddingModels();
    return list.map((model, index) => index === 0
      ? { ...model, status: 'ready', integrityStatus: 'verified', loadReady: true, isDefault: true }
      : { ...model, status: 'installing', download: { id: 'dl-' + model.id, status: 'running', progress: 46, downloadedBytes: Math.round(model.sizeBytes * 0.46), totalBytes: model.sizeBytes, currentFile: 'model.onnx', errorCode: null, errorMessage: null, updatedAt: Date.now() } });
  },
  // The Tauri gateway has native pickers, the mock does not. Without these the
  // dialogs fall back to the <input type=file> branch, so the host-picker
  // buttons the redesign added would never render in this check.
  pickImportFiles: async () => [
    { name: '产品说明.md', relativePath: '产品说明.md', mimeType: 'text/markdown', sizeBytes: 18432 },
    { name: '季度复盘.docx', relativePath: '归档/季度复盘.docx', mimeType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document', sizeBytes: 245760 },
  ],
  pickImportFolder: async () => [
    { name: '用户手册.pdf', relativePath: '手册/用户手册.pdf', mimeType: 'application/pdf', sizeBytes: 1048576 },
  ],
};
const view = (query.get('view') || 'list') as any;
// The task center opens without a knowledge base, otherwise it would filter
// down to the guide base, which has no seeded jobs.
const knowledgeBaseId = query.get('kb') || (view === 'jobs' ? undefined : 'fox-user-guide');
const documentId = query.get('doc') || undefined;
createRoot(document.getElementById('root')!).render(
  <div className="fox-local-kb-check-frame" style={{height:'100vh',display:'flex',flexDirection:'column',overflow:'auto',background:'var(--fox-canvas)',color:'var(--fox-text)'}}><LocalKnowledgeWorkspace view={view} knowledgeBaseId={knowledgeBaseId} documentId={documentId} gateway={gateway} /></div>
);
(window as any).__kbCase = { view, generation: crypto.randomUUID() };
`

async function connect() {
  let target
  for (let i = 0; i < 80; i++) {
    try { target = (await fetch(`http://127.0.0.1:${cdpPort}/json`).then(r => r.json())).find(t => t.type === 'page'); if (target) break } catch {}
    await sleep(250)
  }
  if (!target) throw Error('test browser unavailable')
  const socket = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((r, j) => { socket.addEventListener('open', r, { once: true }); socket.addEventListener('error', j, { once: true }) })
  let id = 0; const pending = new Map(); const errors = []
  socket.addEventListener('message', ({ data }) => {
    const m = JSON.parse(data), p = pending.get(m.id)
    if (m.method === 'Runtime.exceptionThrown') errors.push(m.params.exceptionDetails.exception?.description ?? m.params.exceptionDetails.text)
    if (m.method === 'Runtime.consoleAPICalled' && m.params.type === 'error') errors.push(m.params.args.map(a => a.value ?? a.description ?? '').join(' '))
    if (p) { clearTimeout(p.timer); pending.delete(m.id); m.error ? p.reject(Error(JSON.stringify(m.error))) : p.resolve(m.result) }
  })
  const send = (method, params = {}) => new Promise((resolve, reject) => { const n = ++id; const timer = setTimeout(() => { pending.delete(n); reject(Error('timeout ' + method)) }, 20000); pending.set(n, { resolve, reject, timer }); socket.send(JSON.stringify({ id: n, method, params })) })
  const evaluate = async expression => { const r = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }); if (r.exceptionDetails) throw Error(r.exceptionDetails.exception?.description || r.exceptionDetails.text); return r.result.value }
  return { send, evaluate, errors, close: () => socket.close() }
}

mkdirSync(output, { recursive: true })
writeFileSync(htmlPath, '<!doctype html><html lang="zh-CN"><head><meta charset="utf-8"><title>Fox local knowledge check</title></head><body><div id="root"></div><script type="module" src="/local-knowledge-check.tsx"></script></body></html>')
writeFileSync(entryPath, source)

let vite, chrome, client, browserProfile, chromeDiagnostics = ''
const results = []
try {
  vite = spawn(process.execPath, [join(root, 'node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { cwd: root, stdio: 'ignore', windowsHide: true })
  browserProfile = mkdtempSync(join(output, 'browser-profile-'))
  chrome = spawn(process.env.CHROME_PATH || 'C:/Program Files/Google/Chrome/Application/chrome.exe', ['--headless=new', '--disable-gpu', `--remote-debugging-port=${cdpPort}`, '--no-first-run', '--no-default-browser-check', `--user-data-dir=${browserProfile}`, 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'], windowsHide: true })
  chrome.stderr.on('data', chunk => { chromeDiagnostics = (chromeDiagnostics + chunk.toString()).slice(-3000) })
  client = await connect(); await client.send('Page.enable'); await client.send('Runtime.enable')
  let serving = false
  for (let i = 0; i < 120; i++) { try { serving = (await fetch(`http://127.0.0.1:${port}/local-knowledge-check.html`)).ok; if (serving) break } catch {} await sleep(250) }
  if (!serving) throw Error('fixture dev server unavailable')

  const scenarios = []
  for (const view of ['home', 'files', 'list', 'detail', 'documents', 'jobs', 'models', 'retrieval']) {
    for (const [mode, dark, theme] of [['light', false, 'graphite'], ['dark', true, 'graphite'], ['paper', false, 'paper']]) {
      scenarios.push({ view, mode, dark, theme })
    }
  }
  const bannedPattern = new RegExp(banned.map(term => term.replace(/[.*+?^${}()|[\]\\]/g, '\\$&')).join('|'), 'g')
  for (const scenario of scenarios) {
    const name = `${scenario.view}-${scenario.mode}`
    await client.send('Emulation.setDeviceMetricsOverride', { width: 1320, height: 900, deviceScaleFactor: 1, mobile: false })
    const previous = await client.evaluate('window.__kbCase?.generation')
    await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/local-knowledge-check.html?view=${scenario.view}&dark=${scenario.dark ? 1 : 0}&theme=${scenario.theme}` })
    let ready = false
    for (let i = 0; i < 400; i++) { ready = await client.evaluate(`window.__kbCase?.generation!==${JSON.stringify(previous ?? null)} && !!document.querySelector('[class*="fox-local-kb"],[class*="fox-kb-"],.fox-page-content')`); if (ready) break; await sleep(200) }
    if (!ready) throw Error(name + ': view did not mount ' + JSON.stringify(client.errors) + ' ' + chromeDiagnostics)
    await client.evaluate('document.fonts.ready.then(() => true)')
    await sleep(300)
    const read = await client.evaluate(`(()=>{
      const root=document.getElementById('root');
      const text=root.innerText;
      const leftover={};
      for(const term of ${JSON.stringify(banned)}){const n=text.split(term).length-1;if(n)leftover[term]=n}
      const marks=[...root.querySelectorAll('mark')].map(m=>({bg:getComputedStyle(m).backgroundColor,color:getComputedStyle(m).color}));
      return {overflow:document.documentElement.scrollWidth>innerWidth+1,textLength:text.length,leftover,marks,title:document.querySelector('h1')?.innerText??null,preview:text.replace(/\s+/g,' ').slice(0,200)};
    })()`)
    if (read.overflow) throw Error(name + ': horizontal overflow ' + JSON.stringify(read))
    if (!read.title || read.textLength < 8) throw Error(name + ': view rendered no heading or almost no text ' + JSON.stringify(read))
    // Step 1 copy contract: the shared label helpers must reach these pages.
    const expected = { list: '可以使用', detail: '可以在对话中使用' }[scenario.view]
    if (expected && !read.preview.includes(expected) && !await client.evaluate(`document.getElementById('root').innerText.includes(${JSON.stringify(expected)})`)) throw Error(name + ': missing expected copy ' + expected + ' ' + JSON.stringify(read))
    if (client.errors.length) throw Error(name + ': console/page errors ' + JSON.stringify(client.errors.slice(0, 3)))
    const shot = await client.send('Page.captureScreenshot', { format: 'png' })
    writeFileSync(join(output, `${name}.png`), Buffer.from(shot.data, 'base64'))
    results.push({ name, ...read })
  }

  // Hover-only affordances have to answer to the keyboard too, and the create
  // dialog is the entry point of the whole page, so both get checked here.
  async function pointer(type, x, y) {
    await client.send('Input.dispatchMouseEvent', { type, x, y, button: type === 'mouseMoved' ? 'none' : 'left', clickCount: type === 'mouseMoved' ? 0 : 1 })
  }
  async function centerOf(selector) {
    const box = await client.evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)return null;const r=e.getBoundingClientRect();return {x:Math.round(r.left+r.width/2),y:Math.round(r.top+r.height/2)}})()`)
    if (!box) throw Error('missing element ' + selector)
    return box
  }
  async function shoot(name) {
    const shot = await client.send('Page.captureScreenshot', { format: 'png' })
    writeFileSync(join(output, `${name}.png`), Buffer.from(shot.data, 'base64'))
  }
  // 共享 SelectContent 曾默认 item-aligned，Radix 会把面板按视口居中，这就是用户说的「弹窗错位」。
  // 这里量浮层是否贴着触发器：垂直间距在 sideOffset 附近，水平必须和触发器有重叠。
  async function anchored(label, triggerSelector, layerSelector) {
    const geo = await client.evaluate(`(()=>{const t=document.querySelector(${JSON.stringify(triggerSelector)});const c=document.querySelector(${JSON.stringify(layerSelector)});if(!t||!c)return null;const tr=t.getBoundingClientRect(),cr=c.getBoundingClientRect();return {gap:Math.round(cr.top-tr.bottom),overlap:Math.round(Math.min(tr.right,cr.right)-Math.max(tr.left,cr.left)),trigger:[Math.round(tr.left),Math.round(tr.right)],layer:[Math.round(cr.left),Math.round(cr.right)],viewport:innerWidth}})()`)
    if (!geo) throw Error(`${label}: trigger or layer missing ` + JSON.stringify({ triggerSelector, layerSelector }))
    if (geo.gap < -4 || geo.gap > 16) throw Error(`${label}: the popup is not attached to its trigger ` + JSON.stringify(geo))
    if (geo.overlap <= 0) throw Error(`${label}: the popup floats away from its trigger ` + JSON.stringify(geo))
    return geo
  }
  const interactions = {}
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/local-knowledge-check.html?view=list&dark=0&theme=graphite` })
  for (let i = 0; i < 400 && !await client.evaluate('document.querySelectorAll(".fox-kb-cover-card").length===3'); i++) await sleep(200)
  // The built-in guide card has no menu, so hover a card that does.
  const card = await centerOf('.fox-kb-cover-card:has(.fox-kb-cover-menu)')
  await pointer('mouseMoved', card.x, card.y)
  await sleep(300)
  interactions.hoverMenuOpacity = await client.evaluate('getComputedStyle(document.querySelector(".fox-kb-cover-card .fox-kb-cover-menu")).opacity')
  if (interactions.hoverMenuOpacity !== '1') throw Error('card menu stays hidden on hover ' + JSON.stringify(interactions))
  await shoot('list-hover')
  await pointer('mouseMoved', 4, 4)
  await sleep(300)
  await client.evaluate('document.querySelector(".fox-kb-cover-more").focus()')
  await sleep(300)
  interactions.focusMenuOpacity = await client.evaluate('getComputedStyle(document.querySelector(".fox-kb-cover-card .fox-kb-cover-menu")).opacity')
  if (interactions.focusMenuOpacity !== '1') throw Error('card menu stays hidden on keyboard focus ' + JSON.stringify(interactions))
  await shoot('list-focus-menu')
  // Radix menus ignore synthetic .click(); they need real pointer events.
  const more = await centerOf('.fox-kb-cover-more')
  await pointer('mouseMoved', more.x, more.y)
  await pointer('mousePressed', more.x, more.y)
  await pointer('mouseReleased', more.x, more.y)
  await sleep(400)
  interactions.menuItems = await client.evaluate(`[...document.querySelectorAll('[role=menuitem]')].map(e=>e.innerText)`)
  if (!interactions.menuItems.includes('编辑信息') || !interactions.menuItems.some(item => item.includes('删除'))) throw Error('card menu items missing ' + JSON.stringify(interactions))
  interactions.cardMenuAnchor = await anchored('资料库卡片菜单', '.fox-kb-cover-more', '[role=menu]')
  await shoot('list-card-menu')
  await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await client.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await sleep(300)
  await client.evaluate(`(()=>{const b=[...document.querySelectorAll('.fox-local-kb-header-actions button')].find(e=>e.innerText.includes('新建资料库'));b.click();return true})()`)
  for (let i = 0; i < 200 && !await client.evaluate('!!document.querySelector(".fox-kb-create-dialog")'); i++) await sleep(100)
  interactions.dropZone = await centerOf('.fox-kb-create-drop-zone').then(() => true, () => false)
  if (!interactions.dropZone) throw Error('create dialog has no drop zone ' + JSON.stringify(interactions))
  // shadcn opens dialogs with zoom-in-95; measuring mid-animation reads a 0.95
  // scaled rect, so let the entrance finish first.
  await sleep(500)
  interactions.dialog = await client.evaluate(`(()=>{const d=document.querySelector('.fox-kb-create-dialog'),title=d.querySelector('[data-slot="dialog-title"]'),desc=d.querySelector('[data-slot="dialog-description"]'),input=d.querySelector('[data-slot="input"]'),label=d.querySelector('.fox-local-kb-create-form label'),drop=document.querySelector('.fox-kb-create-drop-zone'),foot=document.querySelector('.fox-local-kb-create-actions'),prev=foot.previousElementSibling;return {width:Math.round(d.getBoundingClientRect().width),padding:Math.round(parseFloat(getComputedStyle(d).paddingLeft)),titleSize:getComputedStyle(title).fontSize,descSize:getComputedStyle(desc).fontSize,labelSize:getComputedStyle(label).fontSize,inputHeight:Math.round(input.getBoundingClientRect().height),dropPad:Math.round(parseFloat(getComputedStyle(drop).paddingTop)),dropBorder:getComputedStyle(drop).borderTopStyle,footGap:Math.round(parseFloat(getComputedStyle(foot).gap)),footTop:Math.round(foot.getBoundingClientRect().top-prev.getBoundingClientRect().bottom)}})()`)
  if (interactions.dialog.width !== 460 || interactions.dialog.padding !== 24) throw Error('create dialog is not the prototype 460/24 ' + JSON.stringify(interactions.dialog))
  if (interactions.dialog.titleSize !== '16px' || interactions.dialog.descSize !== '13px' || interactions.dialog.labelSize !== '12px') throw Error('create dialog type scale drifted ' + JSON.stringify(interactions.dialog))
  if (interactions.dialog.inputHeight !== 36 || interactions.dialog.dropPad !== 22 || interactions.dialog.dropBorder !== 'dashed') throw Error('create dialog fields are not the prototype 36px input / 22px dashed drop ' + JSON.stringify(interactions.dialog))
  if (interactions.dialog.footGap !== 8 || interactions.dialog.footTop !== 24) throw Error('create dialog footer rhythm drifted ' + JSON.stringify(interactions.dialog))
  // The folder picker sat under the real Button swap: it has to look like every
  // other outline control in the dialog, not like a bare browser button.
  interactions.folderPick = await client.evaluate(`(()=>{const b=document.querySelector('.fox-kb-create-pick-folder');if(!b)return null;const r=b.getBoundingClientRect(),cs=getComputedStyle(b),drop=document.querySelector('.fox-kb-create-drop-zone'),dr=drop.getBoundingClientRect();return {tag:b.tagName,slot:b.getAttribute('data-slot'),dropTag:drop.tagName,variant:b.getAttribute('data-variant'),height:Math.round(r.height),borderTop:cs.borderTopWidth+' '+cs.borderTopStyle,radius:cs.borderTopLeftRadius,gapToDrop:Math.round(r.top-dr.bottom),centerOffset:Math.round(Math.abs((r.left+r.width/2)-(dr.left+dr.width/2)))}})()`)
  if (!interactions.folderPick) throw Error('the create dialog has no 选择一个文件夹 control; the fixture gateway lost pickImportFolder')
  if (interactions.folderPick.dropTag !== 'BUTTON') throw Error('the primary drop zone is not a real button ' + JSON.stringify(interactions.folderPick))
  if (interactions.folderPick.slot !== 'button') throw Error('the folder picker is a bare element, not the shared Button ' + JSON.stringify(interactions.folderPick))
  if (interactions.folderPick.height !== 28) throw Error('the folder picker is not the prototype 28px small control ' + JSON.stringify(interactions.folderPick))
  if (!interactions.folderPick.borderTop.startsWith('1px solid')) throw Error('the folder picker lost its outline border ' + JSON.stringify(interactions.folderPick))
  if (interactions.folderPick.gapToDrop !== 8 || interactions.folderPick.centerOffset > 1) throw Error('the folder picker is off the drop zone grid ' + JSON.stringify(interactions.folderPick))
  const closedCopy = await client.evaluate('document.body.innerText.replace(/\\s+/g," ")')
  await shoot('create-dialog')
  await client.evaluate(`(()=>{const b=[...document.querySelectorAll('.fox-kb-create-more-toggle')][0];b.click();return true})()`)
  await sleep(300)
  const moreCopy = await client.evaluate('document.body.innerText.replace(/\\s+/g," ")')
  await shoot('create-dialog-more')
  for (const label of ['简介', '搜索方式', '段落长度', '衔接长度']) {
    if (!moreCopy.includes(label)) throw Error(`the 更多选项 disclosure does not reveal ${label}`)
  }
  if (closedCopy.includes('段落长度')) throw Error('advanced fields are visible before the disclosure opens')
  interactions.disclosureFields = ['简介', '搜索方式', '段落长度', '衔接长度']
  results.push({ name: 'interactions', leftover: {}, title: '新建资料库', ...interactions })

  // Hub: the search box replaces the old 查找测试 tab, so a query has to produce
  // marked hits, and the clock / gear have to open their surfaces.
  const hub = {}
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/local-knowledge-check.html?view=detail&kb=local-product-docs&dark=0&theme=graphite` })
  for (let i = 0; i < 400 && !await client.evaluate('!!document.querySelector(".fox-kb-hub-head")'); i++) await sleep(200)
  if (!await client.evaluate('!!document.querySelector(".fox-kb-hub-head")')) throw Error('hub header did not render ' + JSON.stringify(client.errors.slice(0, 3)))
  hub.status = await client.evaluate('document.querySelector(".fox-kb-hub-status").innerText.replace(/\\s+/g," ")')
  if (!hub.status.includes('可以在对话中使用')) throw Error('hub status line wrong ' + JSON.stringify(hub))
  hub.geometry = await client.evaluate(`(()=>{const head=document.querySelector('.fox-kb-hub-head'),box=document.querySelector('.fox-kb-search'),crumb=document.querySelector('.fox-kb-doc-crumb'),row=document.querySelector('.fox-local-kb-folder-row, .fox-local-kb-document-row'),drop=document.querySelector('.fox-kb-dropline');return {searchHeight:Math.round(box.getBoundingClientRect().height),searchTopGap:Math.round(box.getBoundingClientRect().top-head.getBoundingClientRect().bottom),crumbTopGap:Math.round(crumb.getBoundingClientRect().top-(document.querySelector('.fox-kb-hub-hint')??box).getBoundingClientRect().bottom),rowHeight:Math.round(row.getBoundingClientRect().height),dropline:drop?drop.innerText.trim():null}})()`)
  if (hub.geometry.searchHeight !== 36) throw Error('the hub search box is not the prototype 36px ' + JSON.stringify(hub.geometry))
  if (hub.geometry.searchTopGap !== 24) throw Error('the hub search box is not 24px under the header ' + JSON.stringify(hub.geometry))
  if (hub.geometry.rowHeight !== 44) throw Error('document rows are not the prototype 44px ' + JSON.stringify(hub.geometry))
  if (!hub.geometry.dropline) throw Error('the list has no add-files line under it ' + JSON.stringify(hub.geometry))
  await client.evaluate(`(()=>{const i=document.querySelector('.fox-kb-search input');const setter=Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set;setter.call(i,'资料库');i.dispatchEvent(new Event('input',{bubbles:true}))})()`)
  await client.evaluate(`document.querySelector('.fox-kb-search').requestSubmit()`)
  for (let i = 0; i < 200 && !await client.evaluate('!!document.querySelector(".fox-kb-hit")'); i++) await sleep(100)
  hub.hits = await client.evaluate('document.querySelectorAll(".fox-kb-hit").length')
  if (!hub.hits) throw Error('hub search returned no hits ' + JSON.stringify({ hub, errors: client.errors.slice(0, 3) }))
  hub.marks = await client.evaluate(`(()=>{const m=[...document.querySelectorAll('.fox-kb-hit mark')];return {count:m.length,bg:m[0]?getComputedStyle(m[0]).backgroundColor:null,text:m[0]?.innerText??null}})()`)
  if (!hub.marks.count || hub.marks.bg === 'rgba(0, 0, 0, 0)') throw Error('search hits are not marked ' + JSON.stringify(hub))
  hub.searching = await client.evaluate(`(()=>{const b=document.querySelector('.fox-local-kb-browser'),acts=document.querySelector('.fox-kb-hit-acts');return {listHidden:b?getComputedStyle(b).display==='none':'missing',actsOpacity:getComputedStyle(acts).opacity}})()`)
  if (!hub.searching.listHidden) throw Error('the document list is still shown under the results ' + JSON.stringify(hub.searching))
  const hitCenter = await centerOf('.fox-kb-hit')
  await pointer('mouseMoved', hitCenter.x, hitCenter.y)
  await sleep(250)
  hub.hitHover = await client.evaluate('getComputedStyle(document.querySelector(".fox-kb-hit-acts")).opacity')
  if (hub.hitHover === '0') throw Error('the hit actions never appear on hover ' + JSON.stringify(hub))
  await pointer('mouseMoved', 4, 4)
  await sleep(250)
  await shoot('hub-search')
  const clock = await centerOf('.fox-kb-activity-trigger')
  await pointer('mousePressed', clock.x, clock.y); await pointer('mouseReleased', clock.x, clock.y)
  await sleep(400)
  hub.activityRows = await client.evaluate('document.querySelectorAll(".fox-kb-activity-row").length')
  if (!hub.activityRows) throw Error('activity popover is empty ' + JSON.stringify(hub))
  hub.activityAnchor = await anchored('最近动态浮层', '.fox-kb-activity-trigger', '.fox-kb-activity-pop')
  await shoot('hub-activity')
  await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await client.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await sleep(300)
  const gear = await centerOf('.fox-kb-hub-actions button[aria-label="资料库设置"]')
  await pointer('mousePressed', gear.x, gear.y); await pointer('mouseReleased', gear.x, gear.y)
  for (let i = 0; i < 200 && !await client.evaluate('!!document.querySelector(".fox-kb-settings-sheet")'); i++) await sleep(100)
  await sleep(400)
  hub.sheet = await client.evaluate(`(()=>{const s=document.querySelector('.fox-kb-settings-sheet');const field=document.querySelector('.fox-kb-settings-form > div:first-child'),fl=field.querySelector('label'),fi=field.querySelector('input'),next=document.querySelectorAll('.fox-kb-settings-form > div')[1],mode=document.querySelectorAll('.fox-kb-settings-mode')[0],foot=document.querySelector('.fox-kb-settings-foot'),head=document.querySelector('.fox-kb-settings-sheet [data-slot="sheet-header"]');return s?{width:Math.round(s.getBoundingClientRect().width),modes:document.querySelectorAll('.fox-kb-settings-mode').length,checked:[...document.querySelectorAll('.fox-kb-settings-mode input')].findIndex(i=>i.checked),headPadTop:Math.round(parseFloat(getComputedStyle(head).paddingTop)),labelSize:getComputedStyle(fl).fontSize,labelGap:Math.round(fi.getBoundingClientRect().top-fl.getBoundingClientRect().bottom),blockGap:Math.round(next.getBoundingClientRect().top-fi.getBoundingClientRect().bottom),rowBorder:getComputedStyle(mode).borderBottomWidth,rowFrame:getComputedStyle(mode).borderTopWidth+'/'+getComputedStyle(mode).borderRadius,inputHeight:Math.round(fi.getBoundingClientRect().height),footPad:Math.round(parseFloat(getComputedStyle(foot).paddingLeft)),footBottom:Math.round(window.innerHeight-foot.getBoundingClientRect().bottom),closeSize:Math.round(document.querySelector('.fox-kb-settings-sheet [data-slot="sheet-close"]').getBoundingClientRect().width),footHeight:Math.round(foot.getBoundingClientRect().height),radio:(r=>Math.round(r.width)+'x'+Math.round(r.height))(mode.querySelector('input').getBoundingClientRect())}:null})()`)
  if (!hub.sheet) throw Error('settings sheet did not open ' + JSON.stringify(hub))
  if (hub.sheet.modes !== 2) throw Error('settings sheet is missing the two search modes ' + JSON.stringify(hub))
  if (hub.sheet.width !== 400) throw Error('settings sheet is not the prototype 400px ' + JSON.stringify(hub.sheet))
  if (hub.sheet.headPadTop !== 20 || hub.sheet.labelSize !== '12px' || hub.sheet.inputHeight !== 36) throw Error('settings sheet head/label/input do not match the prototype ' + JSON.stringify(hub.sheet))
  if (hub.sheet.rowBorder !== '1px' || !hub.sheet.rowFrame.startsWith('0px')) throw Error('search modes are still cards instead of the prototype rows ' + JSON.stringify(hub.sheet))
  if (hub.sheet.footPad !== 24 || hub.sheet.footBottom !== 0 || hub.sheet.closeSize !== 28) throw Error('settings sheet footer/close do not match the prototype ' + JSON.stringify(hub.sheet))
  if (hub.sheet.labelGap !== 8 || hub.sheet.blockGap !== 20) throw Error('settings sheet label rhythm drifted ' + JSON.stringify(hub.sheet))
  if (hub.sheet.radio !== '15x15') throw Error('the search-mode radios are not 15px dots ' + JSON.stringify(hub.sheet))
  if (hub.sheet.footHeight > 66) throw Error('the settings sheet footer stacked its buttons ' + JSON.stringify(hub.sheet))
  await client.evaluate(`(()=>{const b=document.querySelector('.fox-kb-settings-disclose');b.click()})()`)
  await sleep(300)
  hub.advanced = await client.evaluate(`(()=>{const t=document.querySelector('.fox-kb-settings-sheet').innerText;return {facts:[...document.querySelectorAll('.fox-kb-settings-facts dt')].map(e=>e.innerText),hasChunk:t.includes('段落长度')}})()`)
  if (!hub.advanced.hasChunk || !hub.advanced.facts.includes('存放位置')) throw Error('advanced disclosure incomplete ' + JSON.stringify(hub))
  await shoot('hub-settings')
  hub.columns = await client.evaluate(`(()=>{const head=[...document.querySelectorAll('.fox-local-kb-browser-list-head > span')].map(e=>Math.round(e.getBoundingClientRect().left));const row=document.querySelector('.fox-local-kb-folder-row, .fox-local-kb-document-row');return {head,cells:row?[...row.children].map(e=>Math.round(e.getBoundingClientRect().left)):null}})()`)
  if (hub.columns.head[1] !== hub.columns.cells[1] || hub.columns.head[2] !== hub.columns.cells[2]) throw Error('the document list head does not line up with its columns ' + JSON.stringify(hub.columns))
  // 内置使用指南也要有设置入口，只是打开后是只读的。
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/local-knowledge-check.html?view=detail&kb=fox-user-guide&dark=0&theme=graphite` })
  for (let i = 0; i < 400 && !await client.evaluate(`!!document.querySelector('.fox-kb-hub-actions button[aria-label="资料库设置"]')`); i++) await sleep(200)
  const guideGear = await centerOf('.fox-kb-hub-actions button[aria-label="资料库设置"]')
  await pointer('mouseMoved', guideGear.x, guideGear.y)
  await pointer('mousePressed', guideGear.x, guideGear.y); await pointer('mouseReleased', guideGear.x, guideGear.y)
  for (let i = 0; i < 60 && !await client.evaluate('!!document.querySelector(".fox-kb-settings-sheet")'); i++) await sleep(100)
  hub.guideSheet = await client.evaluate(`(()=>{const s=document.querySelector('.fox-kb-settings-sheet');if(!s)return null;return {buttons:[...s.querySelectorAll('button')].map(b=>b.innerText.trim()).filter(Boolean),readonly:!!s.querySelector('.fox-kb-settings-form.is-readonly'),nameDisabled:!!document.querySelector('#local-knowledge-base-edit-name')?.disabled,desc:s.querySelector('[data-slot="sheet-description"]')?.innerText.replace(/\\s+/g,' ')}})()`)
  if (!hub.guideSheet) throw Error('the built-in guide base has no settings sheet ' + JSON.stringify(hub))
  if (!hub.guideSheet.readonly || !hub.guideSheet.nameDisabled) throw Error('the guide settings sheet is editable ' + JSON.stringify(hub.guideSheet))
  if (hub.guideSheet.buttons.includes('保存') || hub.guideSheet.buttons.some(b => b.includes('删除'))) throw Error('the guide settings sheet still offers save/delete ' + JSON.stringify(hub.guideSheet))
  if (!hub.guideSheet.buttons.includes('关闭')) throw Error('the guide settings sheet has no 关闭 ' + JSON.stringify(hub.guideSheet))
  await shoot('hub-settings-guide')
  results.push({ name: 'hub', leftover: {}, title: '资料库主页', ...hub })

  // Reader: one text action, the rest behind icons and the ⋯ menu.
  const reader = {}
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/local-knowledge-check.html?view=documents&kb=fox-user-guide&doc=guide-workspace&dark=0&theme=graphite` })
  for (let i = 0; i < 400 && !await client.evaluate('!!document.querySelector(".fox-kb-reader-bar")'); i++) await sleep(200)
  if (!await client.evaluate('!!document.querySelector(".fox-kb-reader-bar")')) throw Error('reader bar did not render ' + JSON.stringify(client.errors.slice(0, 3)))
  reader.crumb = await client.evaluate('document.querySelector(".fox-kb-reader-crumb").innerText.replace(/\\s+/g," ")')
  if (!reader.crumb.includes('Fox 使用指南') || !reader.crumb.includes('工作区和文档.md')) throw Error('reader breadcrumb incomplete ' + JSON.stringify(reader))
  reader.textButtons = await client.evaluate(`[...document.querySelectorAll('.fox-kb-reader-actions button')].filter(b => b.innerText.trim()).map(b => b.innerText)`)
  if (reader.textButtons.length !== 1) throw Error('reader has more than one text button ' + JSON.stringify(reader))
  reader.iconLabels = await client.evaluate(`[...document.querySelectorAll('.fox-kb-reader-actions button')].map(b => b.getAttribute('aria-label')).filter(Boolean)`)
  reader.geometry = await client.evaluate(`(()=>{const nav=document.querySelector('.fox-kb-reader-crumb');const kids=[...nav.children].map(e=>{const r=e.getBoundingClientRect();return {tag:e.tagName,text:(e.innerText||'').slice(0,12),top:Math.round(r.top),height:Math.round(r.height)}});const cs=getComputedStyle(nav);return {display:cs.display,flexWrap:cs.flexWrap,align:cs.alignItems,navHeight:Math.round(nav.getBoundingClientRect().height),kids}})()`)
  const rows = new Set(reader.geometry.kids.map(k => Math.round(k.top / 8)))
  if (reader.geometry.display !== 'flex' || rows.size > 1) throw Error('breadcrumb is not a single centered row ' + JSON.stringify(reader.geometry))
  reader.layout = await client.evaluate(`(()=>{const bar=document.querySelector('.fox-kb-reader-bar'),body=document.querySelector('.fox-local-kb-document-reader-body'),article=body.firstElementChild,meta=document.querySelector('.fox-local-kb-document-reader-meta'),left=bar.getBoundingClientRect().left,content=bar.closest('.fox-local-kb-content').getBoundingClientRect();return {barWidth:Math.round(bar.getBoundingClientRect().width),contentWidth:Math.round(content.width),barLeft:Math.round(left-content.left),barHeight:Math.round(bar.getBoundingClientRect().height),barBorder:getComputedStyle(bar).borderBottomWidth,hubHead:!!document.querySelector('.fox-kb-hub-head'),bodyBorder:getComputedStyle(body).borderTopWidth,articleWidth:Math.round(article.getBoundingClientRect().width),articleLeft:Math.round(article.getBoundingClientRect().left-left),metaLeft:Math.round(meta.getBoundingClientRect().left-left),metaTop:Math.round(meta.getBoundingClientRect().top-bar.getBoundingClientRect().bottom),metaPadTop:Math.round(parseFloat(getComputedStyle(meta).paddingTop)),fontSize:getComputedStyle(article).fontSize}})()`)
  if (reader.layout.hubHead) throw Error('the reader still shows the knowledge-base page header ' + JSON.stringify(reader.layout))
  if (reader.layout.barWidth !== reader.layout.contentWidth || reader.layout.barLeft !== 0 || reader.layout.barHeight !== 52 || reader.layout.barBorder !== '1px') throw Error('reader bar is not the prototype full-bleed 52px bar ' + JSON.stringify(reader.layout))
  if (reader.layout.bodyBorder !== '0px') throw Error('reader body is still wrapped in a card ' + JSON.stringify(reader.layout))
  if (reader.layout.articleLeft !== 96 || reader.layout.metaLeft !== 96 || reader.layout.metaTop !== 0 || reader.layout.metaPadTop !== 40) throw Error('reader article is not aligned to the prototype 96px gutter under a 40px gap ' + JSON.stringify(reader.layout))
  if (reader.layout.articleWidth > 640 || reader.layout.fontSize !== '14px') throw Error('reader article is not the prototype 640px reading column ' + JSON.stringify(reader.layout))
  await shoot('reader')
  const ellipsis = await centerOf('.fox-kb-reader-actions button:last-child')
  await pointer('mousePressed', ellipsis.x, ellipsis.y); await pointer('mouseReleased', ellipsis.x, ellipsis.y)
  await sleep(400)
  reader.menuItems = await client.evaluate(`[...document.querySelectorAll('[role=menuitem]')].map(e => e.innerText)`)
  for (const want of ['查看分段', '重新读取', '复制路径', '从资料库移除']) if (!reader.menuItems.includes(want)) throw Error('reader menu missing ' + want + ' ' + JSON.stringify(reader))
  reader.menuAnchor = await anchored('阅读页 ⋯ 菜单', '.fox-kb-reader-actions button:last-child', '[role=menu]')
  await shoot('reader-menu')
  results.push({ name: 'reader', leftover: {}, title: '阅读资料', ...reader })

  // Document list: folders live in a scope dropdown, row actions appear on hover.
  const browser = {}
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/local-knowledge-check.html?view=documents&kb=fox-user-guide&dark=0&theme=graphite` })
  for (let i = 0; i < 400 && !await client.evaluate('!!document.querySelector(".fox-kb-doc-crumb")'); i++) await sleep(200)
  if (!await client.evaluate('!!document.querySelector(".fox-kb-doc-crumb")')) throw Error('document list toolbar did not render ' + JSON.stringify(client.errors.slice(0, 3)))
  browser.scope = await client.evaluate('document.querySelector(".fox-kb-doc-scope").innerText.replace(/\\s+/g," ")')
  if (browser.scope !== '全部资料') throw Error('scope button label wrong ' + JSON.stringify(browser))
  const scopeButton = await centerOf('.fox-kb-doc-scope')
  await pointer('mousePressed', scopeButton.x, scopeButton.y); await pointer('mouseReleased', scopeButton.x, scopeButton.y)
  await sleep(400)
  browser.scopeItems = await client.evaluate(`[...document.querySelectorAll('[role=menuitem]')].map(e=>e.innerText.replace(/\\s+/g,' '))`)
  if (!browser.scopeItems.some(item => item.includes('全部资料')) || !browser.scopeItems.some(item => item.includes('新建文件夹'))) throw Error('scope dropdown incomplete ' + JSON.stringify(browser))
  browser.scopeAnchor = await anchored('资料列表范围菜单', '.fox-kb-doc-scope', '.fox-kb-doc-scope-pop')
  await shoot('documents-scope')
  await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await client.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await sleep(300)
  const firstFolder = await centerOf('.fox-local-kb-folder-row')
  await pointer('mousePressed', firstFolder.x, firstFolder.y); await pointer('mouseReleased', firstFolder.x, firstFolder.y)
  await sleep(500)
  browser.enteredFolder = await client.evaluate('document.querySelector(".fox-kb-doc-scope").innerText.replace(/\\s+/g," ")')
  const docRow = await centerOf('.fox-local-kb-document-row')
  await pointer('mouseMoved', docRow.x, docRow.y)
  await sleep(300)
  browser.hoverMenuOpacity = await client.evaluate('getComputedStyle(document.querySelector(".fox-local-kb-document-row .fox-local-kb-row-menu")).opacity')
  if (browser.hoverMenuOpacity !== '1') throw Error('row menu stays hidden on hover ' + JSON.stringify(browser))
  await shoot('documents-rows')
  const rowMore = await centerOf('.fox-local-kb-document-row .fox-local-kb-row-more')
  await pointer('mousePressed', rowMore.x, rowMore.y); await pointer('mouseReleased', rowMore.x, rowMore.y)
  await sleep(400)
  browser.rowItems = await client.evaluate(`[...document.querySelectorAll('[role=menuitem]')].map(e=>e.innerText.replace(/\\s+/g,' '))`)
  for (const want of ['预览', '在文件夹中显示', '从资料库移除']) if (!browser.rowItems.includes(want)) throw Error('row menu missing ' + want + ' ' + JSON.stringify(browser))
  await shoot('documents-row-menu')
  await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await client.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await sleep(200)
  const beforeSort = await client.evaluate(`[...document.querySelectorAll('.fox-local-kb-document-row .fox-local-kb-browser-name-text')].map(e=>e.innerText)`)
  await client.evaluate(`document.querySelector('.fox-kb-doc-sort').click()`)
  await sleep(300)
  const afterSort = await client.evaluate(`[...document.querySelectorAll('.fox-local-kb-document-row .fox-local-kb-browser-name-text')].map(e=>e.innerText)`)
  browser.sort = { before: beforeSort, after: afterSort, label: await client.evaluate('document.querySelector(".fox-kb-doc-sort").innerText.trim()') }
  if (beforeSort.length > 1 && beforeSort.join() === afterSort.join()) throw Error('sort did not reorder the list ' + JSON.stringify(browser.sort))
  results.push({ name: 'browser', leftover: {}, title: '资料列表', ...browser })

  // Task center: running and needs-attention stay expandable on top, finished
  // work collapses to one line per task.
  const jobs = {}
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/local-knowledge-check.html?view=jobs&dark=0&theme=graphite` })
  for (let i = 0; i < 400 && !await client.evaluate('!!document.querySelector(".fox-kb-job-pinned")'); i++) await sleep(200)
  if (!await client.evaluate('!!document.querySelector(".fox-kb-job-pinned")')) throw Error('pinned job section did not render ' + JSON.stringify(client.errors.slice(0, 3)))
  jobs.pinned = await client.evaluate(`(()=>({rows:document.querySelectorAll('.fox-kb-job-pinned .fox-local-kb-job-row').length}))()`)
  if (jobs.pinned.rows !== 2) throw Error('pinned section should hold the two unfinished tasks ' + JSON.stringify(jobs))
  jobs.days = await client.evaluate(`[...document.querySelectorAll('.fox-local-kb-job-day > header strong')].map(e => e.innerText)`)
  jobs.history = await client.evaluate(`(()=>{const lines=[...document.querySelectorAll('.fox-local-kb-job-line')];return {count:lines.length,heights:lines.map(e=>Math.round(e.getBoundingClientRect().height)),expanders:lines.filter(e=>e.querySelector('button,[aria-expanded]')).length,titles:lines.map(e=>e.innerText.replace(/\\s+/g,' ')).filter(t=>t),dotless:lines.filter(e=>!e.querySelector('.fox-status-dot')).length}})()`)
  if (!jobs.history.count) throw Error('no history rows rendered ' + JSON.stringify(jobs))
  if (jobs.history.expanders) throw Error('history rows are still expandable ' + JSON.stringify(jobs.history))
  if (Math.max(...jobs.history.heights) > 40) throw Error('history row is not a single line ' + JSON.stringify(jobs.history))
  if (jobs.history.dotless) throw Error('history row without a status dot ' + JSON.stringify(jobs.history))
  if (jobs.days.length < 2) throw Error('history did not group into day sections ' + JSON.stringify(jobs))
  jobs.geometry = await client.evaluate(`(()=>{const line=document.querySelector('.fox-local-kb-job-line'),day=document.querySelector('.fox-local-kb-job-day > header'),pinned=document.querySelector('.fox-kb-job-pinned');return {pinnedWidth:Math.round(pinned.getBoundingClientRect().width),lineHeight:Math.round(line.getBoundingClientRect().height),dayBorderBottom:getComputedStyle(day).borderBottomWidth,dayToggle:!!document.querySelector('.fox-local-kb-job-day-toggle'),pinnedBorder:getComputedStyle(pinned).borderTopWidth,tabs:[...document.querySelectorAll('.fox-local-kb-job-filter-tabs [role=tab]')].map(t=>t.innerText.replace(/\\s+/g,''))}})()`)
  if (jobs.geometry.pinnedWidth !== 820) throw Error('the jobs column is not the prototype 820px ' + JSON.stringify(jobs.geometry))
  if (jobs.geometry.lineHeight !== 40 || jobs.geometry.dayToggle || jobs.geometry.pinnedBorder !== '0px') throw Error('history rows are not the prototype one-line list ' + JSON.stringify(jobs.geometry))
  if (!/^进行中\s*\d+$/.test(jobs.geometry.tabs[1]) || !/^需处理\s*\d+$/.test(jobs.geometry.tabs[2]) || !/^已完成\s*\d+$/.test(jobs.geometry.tabs[3])) throw Error('the segmented tabs are missing their counts ' + JSON.stringify(jobs.geometry.tabs))
  if (!await client.evaluate(`(()=>{const h=document.querySelector('.fox-local-kb-header');return !!h && h.contains(document.querySelector('.fox-local-kb-job-toolbar'))})()`)) throw Error('the jobs toolbar did not move into the page header')
  await shoot('jobs-split')
  // 原型 08-jobs：进行中的任务默认也只是一行 + 3px 进度条，阶段清单要点开才出现。
  jobs.collapsed = await client.evaluate(`(()=>{const rows=[...document.querySelectorAll('.fox-kb-job-pinned .fox-local-kb-job-row')];const active=rows.find(r=>r.classList.contains('is-active'));const bar=active&&active.querySelector('.fox-local-kb-job-row-progress');return {classes:rows.map(r=>r.className.replace('fox-local-kb-job-row ','')),open:document.querySelectorAll('.fox-kb-job-pinned .fox-local-kb-job-row.is-expanded').length,bar:bar?Math.round(bar.getBoundingClientRect().height):null,pct:active?/\\d+%/.test(active.querySelector('.fox-local-kb-job-row-status').innerText):false}})()`)
  if (jobs.collapsed.open || jobs.collapsed.bar !== 3 || !jobs.collapsed.pct) throw Error('a running task is not the prototype one-line row ' + JSON.stringify(jobs.collapsed))
  jobs.rowIcon = await client.evaluate(`(()=>{const active=[...document.querySelectorAll('.fox-kb-job-pinned .fox-local-kb-job-row')].find(r=>r.classList.contains('is-active'));const icon=active.querySelector('.fox-local-kb-job-row-icon');return {width:getComputedStyle(icon).width,cls:icon.getAttribute('class')}})()`)
  if (jobs.rowIcon.width !== '16px') throw Error('a pinned row has no 16px leading status icon ' + JSON.stringify(jobs))
  await client.evaluate(`document.querySelector('.fox-kb-job-pinned').scrollIntoView({block:'start'})`)
  await client.evaluate(`(()=>{document.querySelector('.fox-kb-job-pinned .fox-local-kb-job-row.is-active .fox-local-kb-job-row-head').click()})()`)
  await sleep(400)
  jobs.expanded = await client.evaluate(`(()=>{const row=document.querySelector('.fox-kb-job-pinned .fox-local-kb-job-row.is-expanded');return {stages:row?row.querySelectorAll('.fox-local-kb-job-stages > li').length:0,progress:row?/已完成 \\d+\\/\\d+/.test(row.innerText):false,bar:!!row?.querySelector('.fox-local-kb-job-row-progress')}})()`)
  if (jobs.expanded.stages < 4 || !jobs.expanded.progress || jobs.expanded.bar) throw Error('the opened pinned task is missing its stages ' + JSON.stringify(jobs))
  await shoot('jobs-expanded')
  await client.evaluate(`(()=>{document.querySelector('.fox-kb-job-pinned .fox-local-kb-job-row.is-active .fox-local-kb-job-row-head').click()})()`)
  await sleep(400)
  jobs.recollapsed = await client.evaluate('document.querySelectorAll(".fox-kb-job-pinned .fox-local-kb-job-row.is-expanded").length')
  if (jobs.recollapsed !== 0) throw Error('the pinned task head row does not collapse ' + JSON.stringify(jobs))
  await client.evaluate(`(()=>{const t=[...document.querySelectorAll('.fox-local-kb-job-filter-tabs [role=tab]')].find(e => e.innerText.trim().startsWith('已完成'));t.scrollIntoView({block:'center'});t.dataset.foxTarget='1'})()`)
  const jobTab = await centerOf('[data-fox-target="1"]')
  await pointer('mouseMoved', jobTab.x, jobTab.y)
  await pointer('mousePressed', jobTab.x, jobTab.y); await pointer('mouseReleased', jobTab.x, jobTab.y)
  await sleep(400)
  jobs.completedFilter = await client.evaluate(`(()=>({pinned:document.querySelectorAll('.fox-kb-job-pinned').length,lines:document.querySelectorAll('.fox-local-kb-job-line').length}))()`)
  if (jobs.completedFilter.pinned !== 0 || jobs.completedFilter.lines !== 3) throw Error('the 已完成 filter still shows unfinished tasks ' + JSON.stringify(jobs.completedFilter))
  await shoot('jobs-completed-filter')
  await client.evaluate(`(()=>{const t=[...document.querySelectorAll('.fox-local-kb-job-filter-tabs [role=tab]')].find(e => e.innerText.trim()==='全部');t.dataset.foxTarget='1'})()`)
  const allTab = await centerOf('[data-fox-target="1"]')
  await pointer('mouseMoved', allTab.x, allTab.y)
  await pointer('mousePressed', allTab.x, allTab.y); await pointer('mouseReleased', allTab.x, allTab.y)
  await sleep(400)
  const filterTrigger = await centerOf('.fox-local-kb-job-toolbar button[aria-label^="筛选任务"]')
  await pointer('mouseMoved', filterTrigger.x, filterTrigger.y)
  await pointer('mousePressed', filterTrigger.x, filterTrigger.y); await pointer('mouseReleased', filterTrigger.x, filterTrigger.y)
  for (let i = 0; i < 60 && !await client.evaluate('!!document.querySelector(".fox-local-kb-job-filter-pop")'); i++) await sleep(100)
  jobs.filterAnchor = await anchored('任务筛选浮层', '.fox-local-kb-job-toolbar button[aria-label^="筛选任务"]', '.fox-local-kb-job-filter-pop')
  await shoot('jobs-filter-pop')
  results.push({ name: 'jobs', leftover: {}, title: '处理进度', ...jobs })

  // Models: the model in use gets its own card, everything else is a plain
  // row with one action, and the technical fields live behind the menu.
  const models = {}
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/local-knowledge-check.html?view=models&dark=0&theme=graphite` })
  for (let i = 0; i < 400 && !await client.evaluate('!!document.querySelector(".fox-kb-model-hero")'); i++) await sleep(200)
  if (!await client.evaluate('!!document.querySelector(".fox-kb-model-hero")')) throw Error('model hero card did not render ' + JSON.stringify(client.errors.slice(0, 3)))
  models.hero = await client.evaluate(`(()=>{const hero=document.querySelector('.fox-kb-model-hero');return {status:hero.querySelector('.fox-kb-model-hero-status').innerText.replace(/\\s+/g,' '),text:hero.innerText.replace(/\\s+/g,' '),buttons:[...hero.querySelectorAll('button')].map(b => b.innerText.trim() || b.getAttribute('aria-label'))}})()`)
  if (!models.hero.status.includes('使用中')) throw Error('hero card is not the model in use ' + JSON.stringify(models.hero))
  if (!models.hero.buttons.includes('试一试')) throw Error('hero card has no 试一试 ' + JSON.stringify(models.hero))
  models.tucked = await client.evaluate(`(()=>{const text=document.body.innerText;return {dimension:text.includes('维度'),license:text.includes('MIT'),lead:text.includes('装好一个就够用')}})()`)
  if (models.tucked.dimension || models.tucked.license) throw Error('technical fields are still on the page ' + JSON.stringify(models))
  models.list = await client.evaluate(`(()=>({section:document.querySelector('.fox-kb-model-section')?.innerText??null,rows:[...document.querySelectorAll('.fox-local-model-row')].map(r=>({text:r.innerText.replace(/\\s+/g,' ').slice(0,60),buttons:[...r.querySelectorAll('button')].map(b=>b.innerText.trim()||b.getAttribute('aria-label')),pause:r.querySelector('.fox-kb-model-pause')?.getAttribute('aria-label')??null,bar:!!r.querySelector('.fox-local-model-download .fox-local-kb-progress span')}))}))()`)
  if (models.list.section !== '其他可选') throw Error('the other-models heading is missing ' + JSON.stringify(models.list))
  if (models.list.rows.length !== 1) throw Error('expected one row under 其他可选 ' + JSON.stringify(models.list))
  if (models.list.rows[0].pause !== '暂停下载' || !models.list.rows[0].bar) throw Error('the downloading row has no progress and pause ' + JSON.stringify(models.list))
  await shoot('models-hero')
  const pageMore = await centerOf('.fox-local-models-page .fox-local-kb-header-actions button[aria-label="模型页更多操作"]')
  await pointer('mouseMoved', pageMore.x, pageMore.y)
  await pointer('mousePressed', pageMore.x, pageMore.y); await pointer('mouseReleased', pageMore.x, pageMore.y)
  await sleep(400)
  models.pageMenu = await client.evaluate(`[...document.querySelectorAll('[role=menuitem]')].map(e=>e.innerText.replace(/\\s+/g,' '))`)
  if (!models.pageMenu.some(item => item.includes('导入模型包')) || !models.pageMenu.some(item => item.includes('运行状态'))) throw Error('model page menu incomplete ' + JSON.stringify(models))
  models.pageMenuAnchor = await anchored('模型页 ⋯ 菜单', '.fox-local-models-page .fox-local-kb-header-actions button[aria-label="模型页更多操作"]', '[role=menu]')
  await shoot('models-page-menu')
  await client.evaluate(`(()=>{const i=[...document.querySelectorAll('[role=menuitem]')].find(e=>e.innerText.includes('运行状态'));i.click()})()`)
  for (let i = 0; i < 200 && !await client.evaluate('!!document.querySelector(".fox-kb-model-health")'); i++) await sleep(100)
  models.health = await client.evaluate(`(()=>{const d=document.querySelector('.fox-kb-model-health');return d?{facts:[...d.querySelectorAll('dt')].map(e=>e.innerText),text:d.innerText.replace(/\\s+/g,' ').slice(0,80)}:null})()`)
  if (!models.health || !models.health.facts.includes('引擎')) throw Error('the 运行状态 dialog did not open ' + JSON.stringify(models))
  await shoot('models-health')
  await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await client.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  results.push({ name: 'models', leftover: {}, title: '向量模型', ...models })

  // Home: one search box, a quiet line of example questions, and 最近查看
  // without a table header.
  const home = {}
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/local-knowledge-check.html?view=home&dark=0&theme=graphite` })
  for (let i = 0; i < 400 && !await client.evaluate('!!document.querySelector(".fox-kb-search")'); i++) await sleep(200)
  home.searchHeight = await client.evaluate('Math.round(document.querySelector(".fox-kb-search").getBoundingClientRect().height)')
  if (home.searchHeight !== 44) throw Error('home search box is not 44px ' + JSON.stringify(home))
  home.geometry = await client.evaluate(`(()=>{const box=document.querySelector('.fox-kb-search'),chip=document.querySelector('.fox-kb-home-jobline'),row=document.querySelector('.fox-local-kb-recent-list > button');return {search:Math.round(box.getBoundingClientRect().width),chipPos:getComputedStyle(chip).position,chipTop:Math.round(chip.getBoundingClientRect().top),rowHeight:Math.round(row.getBoundingClientRect().height)}})()`)
  if (home.geometry.search !== 720) throw Error('the home column is not the prototype 720px ' + JSON.stringify(home.geometry))
  if (home.geometry.chipPos !== 'absolute' || home.geometry.rowHeight !== 44) throw Error('job chip / recent row geometry off spec ' + JSON.stringify(home.geometry))
  home.copy = await client.evaluate(`(()=>{const t=document.body.innerText;return {navButton:[...document.querySelectorAll('button')].some(b=>b.innerText.trim()==='整理文件'),head:!!document.querySelector('.fox-local-kb-recent-list-head'),recent:t.includes('最近查看'),foot:t.includes('看看使用指南'),subtitle:/\\d+ 个资料库，\\d+ 份资料/.test(t),try:document.querySelector('.fox-kb-home-try')?.innerText.replace(/\\s+/g,' ')??null,job:document.querySelector('.fox-kb-home-jobline')?.innerText.replace(/\\s+/g,' ')??null}})()`)
  if (home.copy.navButton) throw Error('the home page still repeats a sidebar entry ' + JSON.stringify(home.copy))
  if (home.copy.head) throw Error('最近查看 still renders a table header ' + JSON.stringify(home.copy))
  if (!home.copy.recent || !home.copy.foot || !home.copy.subtitle) throw Error('home copy incomplete ' + JSON.stringify(home.copy))
  if (!home.copy.try?.startsWith('试试')) throw Error('the 试试 line is missing ' + JSON.stringify(home.copy))
  if (!home.copy.job || !home.copy.job.includes('%')) throw Error('no one-line job progress ' + JSON.stringify(home.copy))
  await shoot('home')
  await client.evaluate(`(()=>{const b=[...document.querySelectorAll('.fox-kb-home-try button')][0];b.click()})()`)
  for (let i = 0; i < 200 && !await client.evaluate('!!document.querySelector(".fox-kb-hit")'); i++) await sleep(100)
  home.exampleHits = await client.evaluate('document.querySelectorAll(".fox-kb-hit").length')
  if (!home.exampleHits) throw Error('an example question did not run a search ' + JSON.stringify(home))
  await shoot('home-example')
  results.push({ name: 'home', leftover: {}, title: '最近', ...home })

  // Local files: the source strip became a scope dropdown, the search became an
  // icon, and row actions only show up on hover.
  const files = {}
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/local-knowledge-check.html?view=files&dark=0&theme=graphite` })
  for (let i = 0; i < 400 && !await client.evaluate('!!document.querySelector(".fox-kb-files-scope")'); i++) await sleep(200)
  if (!await client.evaluate('!!document.querySelector(".fox-kb-files-scope")')) throw Error('file scope dropdown did not render ' + JSON.stringify(client.errors.slice(0, 3)))
  files.scope = await client.evaluate('document.querySelector(".fox-kb-files-scope").innerText.replace(/\\s+/g," ")')
  if (files.scope !== '全部文件夹 1') throw Error('scope button label wrong ' + JSON.stringify(files))
  files.searchCollapsed = await client.evaluate('!!document.querySelector(".fox-local-files-header-actions input")')
  if (files.searchCollapsed) throw Error('the search box is not collapsed to an icon ' + JSON.stringify(files))
  files.head = await client.evaluate(`[...document.querySelectorAll('.fox-local-files-list-head > span')].map(e=>e.innerText)`)
  if (files.head.join() !== ',名称,类型,大小,修改时间,操作') throw Error('file list header wrong ' + JSON.stringify(files))
  files.geometry = await client.evaluate(`(()=>{const row=document.querySelector('.fox-local-files-row'),list=document.querySelector('.fox-local-files-list'),acts=row.querySelector('.fox-local-files-file-acts'),date=row.querySelector('.fox-local-files-file-date');return {rowWidth:Math.round(row.getBoundingClientRect().width),rowHeight:Math.round(row.getBoundingClientRect().height),listBorder:getComputedStyle(list).borderTopWidth,actsAfterDate:Math.round(acts.getBoundingClientRect().left-date.getBoundingClientRect().right)}})()`)
  if (files.geometry.rowWidth !== 940) throw Error('the files column is not the prototype 940px ' + JSON.stringify(files.geometry))
  if (files.geometry.rowHeight !== 52 || files.geometry.listBorder !== '0px') throw Error('file list is not the borderless 52px-row list ' + JSON.stringify(files.geometry))
  if (files.geometry.actsAfterDate < 4) throw Error('the actions column does not sit to the right of 修改时间 ' + JSON.stringify(files.geometry))
  files.columns = await client.evaluate(`(()=>{const head=[...document.querySelectorAll('.fox-local-files-list-head > *')].map(e=>Math.round(e.getBoundingClientRect().left));const row=document.querySelector('.fox-local-files-row');const cells=[...row.children].map(e=>Math.round(e.getBoundingClientRect().left));const title=row.querySelector('.fox-local-files-file-main strong');return {head,cells,icon:Math.round(row.querySelector('.fox-local-files-file-icon').getBoundingClientRect().left),nameLeft:Math.round(title.getBoundingClientRect().left),nameAlign:getComputedStyle(title).textAlign}})()`)
  if (files.columns.head[2] !== files.columns.cells[2] || files.columns.head[3] !== files.columns.cells[3] || files.columns.head[4] !== files.columns.cells[4] || files.columns.head[5] !== files.columns.cells[5] || files.columns.head[6] !== files.columns.cells[6]) throw Error('the file list head does not line up with its columns ' + JSON.stringify(files.columns))
  if (files.columns.icon !== files.columns.head[1]) throw Error('the file row icon does not line up with its head column ' + JSON.stringify(files.columns))
  if (files.columns.nameLeft !== files.columns.cells[2] || files.columns.nameAlign !== 'left') throw Error('the file title is not left aligned in the 名称 column ' + JSON.stringify(files.columns))
  const fileScopeButton = await centerOf('.fox-kb-files-scope')
  await pointer('mouseMoved', fileScopeButton.x, fileScopeButton.y)
  await pointer('mousePressed', fileScopeButton.x, fileScopeButton.y); await pointer('mouseReleased', fileScopeButton.x, fileScopeButton.y)
  await sleep(400)
  files.scopeItems = await client.evaluate(`[...document.querySelectorAll('[role=menuitem]')].map(e=>e.innerText.replace(/\\s+/g,' '))`)
  if (!files.scopeItems.some(item => item.includes('全部文件夹')) || !files.scopeItems.some(item => item.includes('管理文件夹'))) throw Error('scope dropdown incomplete ' + JSON.stringify(files))
  files.scopeAnchor = await anchored('本机文件范围菜单', '.fox-kb-files-scope', '.fox-kb-files-scope-menu')
  await shoot('files-scope')
  await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await client.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await sleep(300)
  const fileRow = await centerOf('.fox-local-files-row')
  files.rowIdleOpacity = await client.evaluate('getComputedStyle(document.querySelector(".fox-local-files-row .fox-local-files-file-acts")).opacity')
  if (files.rowIdleOpacity !== '1') throw Error('the 操作 column is hover-only again ' + JSON.stringify(files))
  await pointer('mouseMoved', fileRow.x, fileRow.y)
  await sleep(300)
  files.rowHoverOpacity = await client.evaluate(`(()=>({acts:getComputedStyle(document.querySelector(".fox-local-files-row .fox-local-files-file-acts")).opacity,check:getComputedStyle(document.querySelector(".fox-local-files-row .fox-kb-files-check")).opacity}))()`)
  if (files.rowHoverOpacity.acts !== '1' || files.rowHoverOpacity.check !== '1') throw Error('row checkbox and 加入资料库 stay hidden on hover ' + JSON.stringify(files))
  await shoot('files-row-hover')
  await client.evaluate(`(()=>{const i=document.querySelector('.fox-local-files-row .fox-kb-files-check');i.click()})()`)
  await sleep(300)
  files.selbar = await client.evaluate(`(()=>{const b=document.querySelector('.fox-local-files-selection');if(!b)return null;const list=document.querySelector('.fox-local-files-list'),btn=b.querySelector('[data-slot="button"]'),cs=getComputedStyle(btn);return {text:b.innerText.replace(/\\s+/g,' '),sticky:getComputedStyle(b).position,belowList:Math.round(b.getBoundingClientRect().top-list.getBoundingClientRect().bottom),height:Math.round(b.getBoundingClientRect().height),button:{bg:cs.backgroundColor,color:cs.color,height:Math.round(btn.getBoundingClientRect().height),contrast:Math.abs(parseFloat(cs.backgroundColor.slice(6))-parseFloat(cs.color.slice(6)))}}})()`)
  if (!files.selbar || !files.selbar.text.includes('已选 1 个')) throw Error('the floating selection bar did not appear ' + JSON.stringify(files.selbar))
  if (files.selbar.belowList < 0) throw Error('the selection bar still sits above the list ' + JSON.stringify(files.selbar))
  if (files.selbar.button.height !== 32 || files.selbar.button.contrast < 0.4) throw Error('the selection bar action button is unreadable ' + JSON.stringify(files.selbar.button))
  if (files.selbar.sticky !== 'sticky') throw Error('selection bar is not a floating bar ' + JSON.stringify(files))
  // Type scale and column geometry, measured rather than eyeballed: the second
  // line of the name cell, the type / size / date cells and the header labels
  // all have to sit on one small size, and each header must line up with its cell.
  files.metrics = await client.evaluate(`(()=>{const px=e=>e?Math.round(parseFloat(getComputedStyle(e).fontSize)*100)/100:null;const row=document.querySelector('.fox-local-files-row'),head=document.querySelector('.fox-local-files-list-head');if(!row||!head)return null;const cells=[...row.children];const col=n=>head.querySelector('.fox-local-files-col-'+n);const left=e=>Math.round(e.getBoundingClientRect().left),right=e=>Math.round(e.getBoundingClientRect().right);const add=document.querySelector('.fox-local-files-header-actions [data-slot=button]'),scope=document.querySelector('.fox-kb-files-scope'),trigger=document.querySelector('.fox-local-files-sort-select');return {name:px(row.querySelector('.fox-local-files-file-main strong')),sub:px(row.querySelector('.fox-local-files-file-main small')),type:px(row.querySelector('.fox-local-files-file-type')),size:px(row.querySelector('.fox-local-files-file-size')),date:px(row.querySelector('.fox-local-files-file-date')),headType:px(col('type')),headSize:px(col('size')),headDate:px(col('date')),checkIndent:left(cells[0])-left(row),iconGap:left(cells[2])-right(cells[1]),cols:cells.length,headCols:head.children.length,actsVisible:getComputedStyle(row.querySelector('.fox-local-files-file-acts')).opacity,addFolder:px(add),scope:px(scope),sortIcons:[...trigger.querySelectorAll('svg')].map(s=>getComputedStyle(s).display)}})()`)
  if (!files.metrics) throw Error('the local file list did not render, so its type scale cannot be verified')
  {
    const m = files.metrics
    for (const [key, value] of Object.entries({ sub: m.sub, type: m.type, size: m.size, date: m.date, headType: m.headType, headSize: m.headSize, headDate: m.headDate })) {
      if (value !== m.date) throw Error(`file list type scale drifted: ${key} is ${value}px, the date column is ${m.date}px ` + JSON.stringify(m))
    }
    if (m.name <= m.date) throw Error('the file title is no larger than its metadata line ' + JSON.stringify(m))
    // Cell edges alone do not prove text centering; check rendered text and the
    // complete action group against each header's grid-column center.
    files.centers = await client.evaluate(`(()=>{const row=document.querySelector('.fox-local-files-row'),head=document.querySelector('.fox-local-files-list-head'),center=r=>r.left+r.width/2,textCenter=e=>{const range=document.createRange();range.selectNodeContents(e);return center(range.getBoundingClientRect())};return ['type','size','date','acts'].map(key=>{const label=head.querySelector('.fox-local-files-col-'+key),cell=row.querySelector('.fox-local-files-file-'+key),buttons=[...cell.querySelectorAll('button')],content=buttons.length?(buttons[0].getBoundingClientRect().left+buttons.at(-1).getBoundingClientRect().right)/2:textCenter(cell);return {key,headerAlign:getComputedStyle(label).textAlign,headerDelta:textCenter(label)-center(label.getBoundingClientRect()),contentDelta:content-center(label.getBoundingClientRect())}})})()`)
    for (const col of files.centers) if (col.headerAlign !== 'center' || Math.abs(col.headerDelta) > 1 || Math.abs(col.contentDelta) > 1) throw Error('file column content/header is not centered ' + JSON.stringify(col))
    if (m.checkIndent < 8) throw Error('the select checkbox is still flush against the left edge ' + JSON.stringify(m))
    if (m.iconGap < 12) throw Error('the file icon is crowded against the title ' + JSON.stringify(m))
    if (m.addFolder !== m.scope) throw Error('the 添加文件夹 button is not on the same type scale as 全部文件夹 ' + JSON.stringify(m))
    if (m.cols !== 7 || m.headCols !== 7) throw Error('the actions column is missing from the file table ' + JSON.stringify(m))
    if (m.actsVisible !== '1') throw Error('the row actions are still hover-only ' + JSON.stringify(m))
    if (m.sortIcons.filter(d => d !== 'none').length !== 1) throw Error('the sort control still shows a second chevron ' + JSON.stringify(m.sortIcons))
  }
  await shoot('files-selected')
  const fileMore = await centerOf('.fox-local-files-row .fox-local-kb-row-more')
  await pointer('mouseMoved', fileMore.x, fileMore.y)
  await pointer('mousePressed', fileMore.x, fileMore.y); await pointer('mouseReleased', fileMore.x, fileMore.y)
  await sleep(400)
  files.rowItems = await client.evaluate(`[...document.querySelectorAll('[role=menuitem]')].map(e=>e.innerText.replace(/\\s+/g,' '))`)
  for (const want of ['加入资料库', '在文件夹中显示']) if (!files.rowItems.includes(want)) throw Error('file row menu missing ' + want + ' ' + JSON.stringify(files))
  if (files.rowItems.includes('预览')) throw Error('预览 belongs to the eye button now, not the row menu ' + JSON.stringify(files.rowItems))
  files.rowMenuAnchor = await anchored('文件行 ⋯ 菜单', '.fox-local-files-row .fox-local-kb-row-more', '[role=menu]')
  await shoot('files-row-menu')
  await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await client.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await sleep(200)
  // 排序下拉：Radix 的 item-aligned 会把面板居中到视口，这里确认它贴着触发器
  const sortTrigger = await centerOf('.fox-local-files-sort-select')
  await pointer('mouseMoved', sortTrigger.x, sortTrigger.y)
  await pointer('mousePressed', sortTrigger.x, sortTrigger.y); await pointer('mouseReleased', sortTrigger.x, sortTrigger.y)
  for (let i = 0; i < 60 && !await client.evaluate('!!document.querySelector("[data-slot=select-content]")'); i++) await sleep(100)
  files.sortAnchor = await anchored('排序下拉', '.fox-local-files-sort-select', '[data-slot=select-content]')
  await shoot('files-sort-open')
  await client.send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await client.send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape', code: 'Escape', windowsVirtualKeyCode: 27 })
  await sleep(200)
  const searchIcon = await centerOf('.fox-local-files-header-actions button[aria-label="搜索本机文件"]')
  await pointer('mouseMoved', searchIcon.x, searchIcon.y)
  await pointer('mousePressed', searchIcon.x, searchIcon.y); await pointer('mouseReleased', searchIcon.x, searchIcon.y)
  await sleep(300)
  files.searchOpen = await client.evaluate('!!document.querySelector(".fox-local-files-header-actions input")')
  if (!files.searchOpen) throw Error('the search icon did not reveal the input ' + JSON.stringify(files))
  await shoot('files-search')
  results.push({ name: 'files', leftover: {}, title: '本机文件', ...files })

  const leaked = results.reduce((all, r) => { for (const term of Object.keys(r.leftover)) all.add(`${r.name}:${term}`); return all }, new Set())
  const summary = { scenarioCount: results.length, leaked: [...leaked].sort(), titles: results.map(r => `${r.name}=${r.title}`) }
  writeFileSync(join(output, 'results.json'), JSON.stringify({ generatedAt: new Date().toISOString(), scenarios: results, summary }, null, 2))
  console.log(JSON.stringify(summary, null, 2))
  console.log(`local-knowledge browser check: ${results.length} scenarios green, ${leaked.size} leftover terms`)
} finally {
  client?.close(); vite?.kill('SIGINT'); chrome?.kill('SIGINT')
  if (browserProfile) setTimeout(() => { try { rmSync(browserProfile, { recursive: true, force: true }) } catch {} }, 500)
}
