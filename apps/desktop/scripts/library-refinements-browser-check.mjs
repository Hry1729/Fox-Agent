import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/library-refinements-20261016/browser')
const port = 1469, cdpPort = 9375
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
  if (!process.env.FOX_HEADER_ONLY) for (const theme of ['graphite','paper','mist']) for (const dark of [false,true]) {
    await navigate(`folders=1&theme=${theme}${dark?'&dark=1':''}`)
    await waitFor('document.querySelectorAll(".fox-kb-cover [data-slot=folder]").length>=4','Rare UI folder cards')
    await screenshot(`cards-${theme}-${dark?'dark':'light'}`)
  }
  await navigate('folders=1')
  await client.evaluate(`document.querySelector('button[aria-label="搜索所有资料库和本机文件"]').click()`)
  await waitFor(`document.activeElement?.getAttribute('aria-label')==='搜索所有资料库和本机文件'`, '页头搜索焦点')
  await input('input[aria-label="搜索所有资料库和本机文件"]', '产品')
  await waitFor(`document.querySelectorAll('.fox-kb-cover-card').length===1`, '搜索筛选')
  const overlap=await client.evaluate(`(()=>{const a=document.querySelector('.fox-library-source-filter').getBoundingClientRect(),b=document.querySelector('input[aria-label="搜索所有资料库和本机文件"]').getBoundingClientRect();return a.left<b.right&&a.right>b.left&&a.top<b.bottom&&a.bottom>b.top})()`); if(overlap) throw Error('搜索框遮挡来源筛选')
  await screenshot('header-search-expanded')
  await client.evaluate(`document.querySelector('button[aria-label="清空搜索"]').click()`)
  await client.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27})
  await client.send('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27})
  await client.evaluate(`document.querySelector('button[aria-label="存储位置"]').click()`)
  await waitFor(`!!document.querySelector('[role="dialog"]')`, '存储位置直接打开')
  results.push({check:'header search filters and storage icon opens dialog',passed:true})
  await navigate('folders=1')
  if (!process.env.FOX_HEADER_ONLY) {
  const widths=[]
  for (const width of [1400,1100,900]) {
    await client.send('Emulation.setDeviceMetricsOverride',{width,height:950,deviceScaleFactor:1,mobile:false})
    await pause(200)
    widths.push(await client.evaluate(`({window:innerWidth,sidebar:document.querySelector('.fox-sidebar').getBoundingClientRect().width,scroll:getComputedStyle(document.querySelector('.fox-sidebar-scroll')).display})`))
  }
  if (widths.some(item=>item.sidebar!==widths[0].sidebar||item.scroll==='none')) throw Error('侧栏随窗口变化 '+JSON.stringify(widths))
  results.push({check:'fixed sidebar on window resize',widths})
  await screenshot('cards-narrow')
  await client.evaluate(`document.querySelector('button[aria-label="产品资料库 的更多操作"]').focus()`)
  if(!await client.evaluate(`getComputedStyle(document.querySelector('button[aria-label="产品资料库 的更多操作"]').closest('.fox-kb-cover-menu')).opacity==='1'`)) { await pause(200) }
  await screenshot('card-keyboard-menu')
  await client.evaluate(`document.querySelector('button[aria-label="产品资料库 的更多操作"]').dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,button:0,pointerType:'mouse',ctrlKey:false}))`)
  await waitFor('!!document.querySelector("[role=menuitem]")','卡片菜单')
  await client.evaluate(`[...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent.includes('编辑信息')).click()`)
  await waitFor('!!document.querySelector(".fox-kb-settings-dialog")','卡片编辑')
  await navigate('view=detail&tree=1')
  await waitFor('document.querySelectorAll(".fox-library-tree-scroll button").length>=60','长目录树')
  const tree=await client.evaluate(`(()=>{const e=document.querySelector('.fox-library-tree-scroll');e.scrollTop=700;return {height:e.getBoundingClientRect().height,scrollHeight:e.scrollHeight,top:e.scrollTop}})()`)
  if(tree.height>321||tree.top<=0||tree.scrollHeight<=tree.height) throw Error('目录树没有内部滚动 '+JSON.stringify(tree))
  results.push({check:'bounded tree',tree})
  await screenshot('bounded-folder-tree')
  }
  if(results.some(item=>item.overflow)) throw Error('出现横向溢出')
  if(client.errors.length) throw Error(JSON.stringify(client.errors))
  writeFileSync(join(output,process.env.FOX_HEADER_ONLY?'header-report.json':'visual-report.json'),JSON.stringify({passed:true,evidence:'component browser fixture',results},null,2))
  process.stdout.write(JSON.stringify({passed:true,output}))
} catch (error) {
  writeFileSync(join(output, 'report.json'), JSON.stringify({ passed: false, error: String(error), diagnostics, results }, null, 2))
  process.stderr.write(`${error.stack ?? error}\n`)
  process.exitCode = 1
} finally { client?.close(); browser?.kill(); server?.kill() }
