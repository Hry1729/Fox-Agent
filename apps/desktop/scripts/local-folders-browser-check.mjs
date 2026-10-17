import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/local-folders-20261016/browser')
const port = 1465, cdpPort = 9371
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
  await navigate('folders=1')
  await waitFor('!!document.querySelector("article[aria-label=打开本地文件夹项目资料]")', '本地文件夹分组')
  if (await client.evaluate('document.body.innerText.includes("全部本机文件")')) throw Error('旧入口未移除')
  await screenshot('01-folders-overview')
  await client.evaluate('document.querySelector("article[aria-label=打开本地文件夹项目资料]").click()')
  await waitFor('!!document.querySelector("button[title=方案]")', '完整目录')
  await clickText('方案'); await clickText('归档')
  await waitFor('document.querySelectorAll(".fox-library-file").length===125', '分页完整读取')
  await screenshot('02-folder-details')
  await client.evaluate('document.querySelector("button[aria-label=搜索本地文件夹]").click()')
  await waitFor('!!document.querySelector("input[placeholder=搜索文件名称]")', '文件夹搜索框')
  await input('input[placeholder=搜索文件名称]', '资料-124')
  await waitFor('document.querySelectorAll(".fox-library-file").length===1', '文件夹搜索')
  await input('input[placeholder=搜索文件名称]', '')
  await waitFor('document.querySelectorAll(".fox-library-file").length===125', '清空文件夹搜索')
  await client.evaluate('document.querySelector("button[aria-label=网格视图]").click()')
  await waitFor('!!document.querySelector(".fox-library-files.is-cards")', '网格视图')
  await screenshot('03-folder-cards')
  await client.evaluate('document.querySelector("button[aria-label=紧凑列表]").click()')
  await waitFor('!!document.querySelector(".fox-library-files.is-compact")', '紧凑列表')
  await client.evaluate('document.querySelector("button[aria-label=详细列表]").click()')
  await client.evaluate('document.querySelector("button[aria-label="+CSS.escape("预览 资料-0.md")+"]").click()')
  await waitFor('!!document.querySelector("[role=dialog]") && document.body.innerText.includes("本地目录中的资料")', '文件预览')
  await clickText('打开所在文件夹')
  if (!await client.evaluate('window.__libraryFixture.operations.some(item=>item.action==="reveal-file"&&item.id==="folder-file-0")')) throw Error('定位文件未传目录记录 ID')
  await client.send('Input.dispatchKeyEvent', { type:'keyDown', key:'Escape', code:'Escape', windowsVirtualKeyCode:27 }); await client.send('Input.dispatchKeyEvent', { type:'keyUp', key:'Escape', code:'Escape', windowsVirtualKeyCode:27 })
  await client.evaluate('document.querySelector("button[aria-label=返回资料库]").click()')
  await clickText('添加文件夹')
  await waitFor('!!document.querySelector("article[aria-label=打开本地文件夹新增资料]")', '新增目录')
  await waitFor('!!document.querySelector(".fox-library-sidebar-tree[aria-label=新增资料的文件夹]")', '侧栏同步')
  await client.evaluate('document.querySelector("article[aria-label=打开本地文件夹新增资料]").click()')
  await waitFor('document.body.innerText.includes("没有可显示的文档")', '空目录说明')
  await client.evaluate('document.querySelector("button[aria-label=返回资料库]").click()')
  const menu = async (name, action) => {
    await client.evaluate(`document.querySelector('button[aria-label="${name} 的更多操作"]').dispatchEvent(new PointerEvent('pointerdown',{bubbles:true,button:0,pointerType:'mouse',ctrlKey:false}))`)
    await waitFor(`!![...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent.includes('${action}'))`, action)
    await client.evaluate(`[...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent.includes('${action}')).click()`)
  }
  await menu('新增资料','重命名')
  await input('input[aria-label=文件夹显示名称]', '已改名')
  await clickText('保存')
  await waitFor('!!document.querySelector("article[aria-label=打开本地文件夹已改名]")', '名称更新')
  await menu('已改名','移除文件夹'); await clickText('移除')
  await waitFor('!document.querySelector("article[aria-label=打开本地文件夹已改名]")&&!document.querySelector(".fox-library-sidebar-tree[aria-label=已改名的文件夹]")', '移除同步')
  await navigate('folders=1&cancelPicker=1')
  await clickText('添加文件夹'); await pause(200)
  if (await client.evaluate('window.__libraryFixture.operations.some(item=>item.action==="add-folder")')) throw Error('取消选择仍添加')
  await navigate('folders=1&refreshError=1')
  await client.evaluate('document.querySelector("article[aria-label=打开本地文件夹项目资料]").click()')
  await waitFor('!!document.querySelector("button[title=方案]")', '刷新错误夹具')
  await client.evaluate('document.querySelector("button[aria-label=刷新文件夹]").click()')
  await waitFor('document.body.innerText.includes("文件夹暂时无法访问")', '刷新失败可见')
  await screenshot('04-folder-refresh-error')
  if (client.errors.length) throw Error(JSON.stringify(client.errors))
  if (results.some(item=>item.overflow)) throw Error('页面横向溢出')
  writeFileSync(join(output, 'report.json'), JSON.stringify({ passed:true, evidence:'synthetic component browser test', checks:['source group','125 files across pages','nested folders','three views','preview','reveal by id','add','rename','remove registration','sidebar synchronization','picker cancellation','refresh error'], results }, null, 2))
  process.stdout.write(JSON.stringify({passed:true, output}))
} catch (error) {
  writeFileSync(join(output, 'report.json'), JSON.stringify({ passed: false, error: String(error), diagnostics, results }, null, 2))
  process.stderr.write(`${error.stack ?? error}\n`)
  process.exitCode = 1
} finally { client?.close(); browser?.kill(); server?.kill() }
