import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync, readFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/library-preview-fixes-20261016/browser')
const port = 1471, cdpPort = 9377
const pause = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
mkdirSync(output, { recursive: true })
let server, browser, client
let diagnostics = ''
const results = []
async function connect() {
  let target
  for (let index = 0; index < 80; index++) {
    try { target = (await fetch(`http://127.0.0.1:${cdpPort}/json`).then((response) => response.json())).find((item) => item.type === 'page'); if (target) break } catch {}
    await pause(250)
  }
  if (!target) throw new Error(`独立测试浏览器未启动：${diagnostics}`)
  const socket = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', reject, { once: true }) })
  let serial = 0
  const pending = new Map(), errors = []
  socket.addEventListener('message', ({ data }) => {
    const response = JSON.parse(data), item = pending.get(response.id)
    if (response.method === 'Runtime.exceptionThrown') errors.push(response.params.exceptionDetails)
    if (item) { clearTimeout(item.timer); pending.delete(response.id); response.error ? item.reject(new Error(JSON.stringify(response.error))) : item.resolve(response.result) }
  })
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++serial
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP 超时：${method}`)) }, 15000)
    pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }))
  })
  const evaluate = async (expression) => { const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }); if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text); return result.result.value }
  return { send, evaluate, errors, close: () => socket.close() }
}
async function waitFor(expression, description) {
  for (let index = 0; index < 200; index++) { if (await client.evaluate(expression)) return; await pause(120) }
  throw new Error(`${description} 未就绪；${JSON.stringify(client.errors)}；${await client.evaluate('document.body.innerText.slice(0,1500)')}`)
}
async function navigate(params = '', width = 1400) {
  await client.send('Emulation.setDeviceMetricsOverride', { width, height: 950, deviceScaleFactor: 1, mobile: false })
  const previous = await client.evaluate('window.__libraryFixture?.generation')
  await client.send('Page.navigate', { url: `http://127.0.0.1:${port}/tests/fixtures/library-unification.html?${params}` })
  await waitFor(`window.__libraryFixture?.generation && window.__libraryFixture.generation!==${JSON.stringify(previous ?? null)} && !!document.querySelector('.fox-local-kb-page,.fox-library-manager,.fox-sidebar-list-pane')`, '组件夹具')
  await client.evaluate('document.fonts.ready.then(()=>true)')
  await pause(500)
}
async function screenshot(name) {
  await pause(650)
  const shot = await client.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false })
  const path = join(output, `${name}.png`)
  writeFileSync(path, Buffer.from(shot.data, 'base64'))
  results.push({ screenshot: name, path, overflow: await client.evaluate('document.documentElement.scrollWidth>innerWidth+1') })
}
const clickText = (text) => client.evaluate(`(()=>{const button=[...document.querySelectorAll('button')].find(item=>item.textContent.trim()===${JSON.stringify(text)});if(!button)throw Error('没有找到按钮 '+${JSON.stringify(text)});button.click()})()`)
const input = (selector, value) => client.evaluate(`(()=>{const element=document.querySelector(${JSON.stringify(selector)});if(!element)throw Error('找不到输入框');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(element,${JSON.stringify(value)});element.dispatchEvent(new Event('input',{bubbles:true}));element.dispatchEvent(new Event('change',{bubbles:true}))})()`)

try {
  server = spawn(process.execPath, [join(root, 'node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', String(port), '--strictPort'], { cwd: root, stdio: 'ignore', windowsHide: true })
  const profile = mkdtempSync(join(output, 'chrome-profile-'))
  browser = spawn(process.env.CHROME_PATH || 'C:/Program Files/Google/Chrome/Application/chrome.exe', ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', `--remote-debugging-port=${cdpPort}`, `--user-data-dir=${profile}`, 'about:blank'], { stdio: ['ignore', 'ignore', 'pipe'], windowsHide: true })
  browser.stderr.on('data', (data) => { diagnostics = (diagnostics + data).slice(-4000) })
  client = await connect()
  await client.send('Page.enable'); await client.send('Runtime.enable')
  let serverReady = false
  for (let attempt = 0; attempt < 100; attempt++) { try { if ((await fetch(`http://127.0.0.1:${port}/tests/fixtures/library-unification.html`)).ok) { serverReady = true; break } } catch {} await pause(150) }
  if (!serverReady) throw new Error('组件夹具服务未启动')

  await navigate(process.env.FOX_READER ? 'view=detail&realReader=1' : 'folders=1&realPreview=1')
  if (!process.env.FOX_READER) await client.evaluate(`document.querySelector('button[aria-label="打开本机文件夹项目资料"]').click()`)
  await waitFor(`!!document.querySelector('button[aria-label="预览 sample.pdf"]')`,'预览文件列表')
  for(const name of ['sample.pdf','sample.xlsx']) {
    if (process.env.FOX_READER) { await navigate('view=detail&realReader=1'); await waitFor(`!!document.querySelector('button[aria-label="预览 ${name}"]')`,'阅读列表') }
    await client.evaluate(`document.querySelector('button[aria-label="预览 ${name}"]').click()`)
    await pause(5000)
    const state=await client.evaluate(`({text:document.querySelector('.fox-local-kb-document-reader,[role=dialog]')?.innerText,canvases:[...document.querySelectorAll('.fox-local-kb-document-reader canvas,[role=dialog] canvas')].map(c=>({w:c.width,h:c.height})),scroll:[...document.querySelectorAll('.fox-local-kb-document-reader *,[role=dialog] *')].filter(e=>e.scrollWidth>e.clientWidth+10||e.scrollHeight>e.clientHeight+10).map(e=>({tag:e.tagName,cls:e.className,cw:e.clientWidth,ch:e.clientHeight,sw:e.scrollWidth,sh:e.scrollHeight}))})`)
    results.push({name,state})
    if(!state.canvases.length || state.text.includes('预览加载失败')) throw Error(`${name} 未成功渲染`)
    await screenshot(name)
    if(name.endsWith('pdf')) {
      await client.evaluate(`document.querySelector('button[aria-label="下一页"]').click()`)
      await waitFor(`!!document.querySelector('canvas[aria-label="sample.pdf 第 2 页"]')`,'PDF 翻页')
      results.push({check:'PDF page two rendered',passed:true})
    }
    if(name.endsWith('xlsx')) {
      const box=await client.evaluate(`(()=>{const r=document.querySelector('.x-spreadsheet-overlayer')?.getBoundingClientRect();return r?{x:r.x+100,y:r.y+100}:null})()`)
      if(box) {
        for(const delta of [{deltaX:0,deltaY:500},{deltaX:600,deltaY:0}]) {await client.send('Input.dispatchMouseEvent',{type:'mouseWheel',...box,...delta});await pause(700)}
        const afterWheel=await client.evaluate(`[...document.querySelectorAll('.x-spreadsheet-scrollbar')].map(e=>({cls:e.className,x:e.scrollLeft,y:e.scrollTop}))`)
        if(!afterWheel.some(s=>s.x>=500)||!afterWheel.some(s=>s.y>=400))throw Error('表格双向滚动失败')
        results.push({afterWheel})
        await screenshot('xlsx-after-wheel')
      }
    }
    await client.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27})
    await client.send('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27})
    await pause(300)
  }
  await navigate('folders=1&cancelPicker=1')
  if(await client.evaluate(`!!document.querySelector('.fox-library-source-filter')`)) throw Error('来源切换未移除')
  await clickText('新建资料库')
  await waitFor(`!!document.querySelector('.fox-kb-create-dialog')`,'新建弹窗')
  await clickText('将本机文件夹作为资料库')
  await pause(300)
  if(!await client.evaluate(`!!document.querySelector('.fox-kb-create-dialog') && window.__libraryFixture.operations.every(o=>o.action!=='add-folder')`)) throw Error('取消目录选择异常')
  await navigate('folders=1')
  await clickText('新建资料库')
  await waitFor(`!!document.querySelector('.fox-kb-create-dialog')`,'新建弹窗')
  await clickText('将本机文件夹作为资料库')
  await waitFor(`window.__libraryFixture.route?.id==='added-folder'`,'新增目录导航')
  results.push({check:'source switch removed, add folder from step one, cancellation and navigation',passed:true})
  await navigate('folders=1')
  const maxkb=JSON.parse(readFileSync(resolve(root,'../../output/library-preview-fixes-20261016/maxkb-files.json'),'utf8'))
  await client.evaluate(`(()=>{const files=${JSON.stringify(maxkb)}.map(f=>({...f,sourceId:'fixture-folder',relativePath:f.path,sizeBytes:f.size,extension:f.name.split('.').pop(),modifiedAt:''}));window.__libraryFixture.gateway.listLocalFiles=async(o)=>{window.__libraryFixture.operations.push({action:'page',limit:o.limit,offset:o.offset});return files.slice(o.offset,o.offset+o.limit)}})()`)
  const started=performance.now()
  await client.evaluate(`document.querySelector('button[aria-label="打开本机文件夹项目资料"]').click()`)
  await waitFor(`!!document.querySelector('.fox-library-manager') && !!document.querySelector('.fox-library-tree-scroll')`,'大目录打开')
  const large=await client.evaluate(`({calls:window.__libraryFixture.operations.filter(o=>o.action==='page'),treeItems:document.querySelectorAll('.fox-library-tree-scroll button').length,rows:document.querySelectorAll('.fox-library-file').length})`)
  if(large.calls.length!==1)throw Error('大目录未合并读取')
  results.push({check:'1445 actual catalog metadata in mock gateway',elapsedMs:performance.now()-started,...large})
  await client.evaluate(`document.querySelector('button[aria-label="返回资料库"]').click()`)
  await pause(350)
  if(await client.evaluate(`document.querySelectorAll('.fox-library-tree-scroll').length`))throw Error('非激活目录树未隐藏')
  await screenshot('cards-final')
  const spacing=await client.evaluate(`(()=>{const section=document.querySelector('.fox-library-source-section');const h=section.querySelector('.fox-local-folder-group-head').getBoundingClientRect(),g=section.querySelector('.fox-kb-cover-grid').getBoundingClientRect();return g.top-h.bottom})()`)
  if(spacing>12)throw Error('卡片与标题间距过大 '+spacing)
  results.push({check:'heading to card grid spacing',pixels:spacing})
  results.push({check:'inactive trees unmounted',passed:true})
  if(client.errors.length) throw Error(JSON.stringify(client.errors))
  writeFileSync(join(output,'report.json'),JSON.stringify({results,errors:client.errors},null,2))
  process.stdout.write(JSON.stringify({results,errors:client.errors}))
} catch(error) {process.stderr.write(error.stack);process.exitCode=1} finally {client?.close();browser?.kill();server?.kill()}
