import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync, readFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/ui-refinements-20261015/browser')
const port = 1464, cdpPort = 9370
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

  async function click(selector) {
    const point = await client.evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)throw Error('missing '+${JSON.stringify(selector)});const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`)
    await client.send('Input.dispatchMouseEvent', {type:'mousePressed', ...point, button:'left',clickCount:1})
    await client.send('Input.dispatchMouseEvent', {type:'mouseReleased', ...point, button:'left',clickCount:1})
  }
  async function escape() {
    await client.send('Input.dispatchKeyEvent',{type:'keyDown',key:'Escape',code:'Escape',windowsVirtualKeyCode:27})
    await client.send('Input.dispatchKeyEvent',{type:'keyUp',key:'Escape',code:'Escape',windowsVirtualKeyCode:27})
    await pause(200)
  }
  async function theme(name, dark=false) {
    await client.evaluate(`document.documentElement.dataset.colorTheme=${JSON.stringify(name)};document.documentElement.classList.toggle('dark',${dark})`)
    await pause(250)
  }
  async function sample(selectors) {
    return client.evaluate(`(()=>{const read=e=>{if(!e)return null;const c=getComputedStyle(e);const r=e.getBoundingClientRect();return {color:c.color,background:c.backgroundColor,border:c.borderColor,shadow:c.boxShadow,accent:c.accentColor,radius:c.borderRadius,width:r.width,height:r.height,x:r.x,y:r.y}};const result={};for(const [key,selector] of Object.entries(${JSON.stringify(selectors)}))result[key]=read(document.querySelector(selector));result.tokens={primary:getComputedStyle(document.documentElement).getPropertyValue('--primary'),brand:getComputedStyle(document.documentElement).getPropertyValue('--brand')};return result})()`)
  }
  const palette = ['graphite','paper','mist']
  if (process.env.THEME_TAIL) results.push(...JSON.parse(readFileSync(join(output, process.env.INTERACTIONS_ONLY ? 'interaction-report.json' : 'report.json'), 'utf8')).results)
  if (!process.env.THEME_TAIL) {
  await navigate('view=detail')
  await waitFor('!!document.querySelector("button[aria-label=添加文件或文件夹]")', '添加按钮')
  await click('button[aria-label="添加文件或文件夹"]')
  await waitFor('!!document.querySelector(".fox-library-add-menu")','添加菜单')
  const menu = await client.evaluate('document.querySelector(".fox-library-add-menu").innerText')
  if(!menu.includes('添加文件')||!menu.includes('添加文件夹'))throw Error('添加菜单项缺失')
  await screenshot('01-add-menu')
  await client.evaluate(`[...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent==='添加文件夹').click()`)
  await waitFor('!!document.querySelector(".fox-local-kb-folder-dialog input")','新建文件夹弹窗')
  await input('.fox-local-kb-folder-dialog input','交互检查')
  await clickText('创建文件夹')
  await waitFor('!document.querySelector(".fox-local-kb-folder-dialog") && !![...document.querySelectorAll(".fox-library-file-open")].find(e=>e.title==="交互检查")','新建完成')
  await client.evaluate(`[...document.querySelectorAll('.fox-library-file-open')].find(e=>e.title==='交互检查').click()`)
  await click('button[aria-label="添加文件或文件夹"]')
  await waitFor('!!document.querySelector(".fox-library-add-menu")','文件添加菜单')
  await client.evaluate(`[...document.querySelectorAll('[role=menuitem]')].find(e=>e.textContent==='添加文件').click()`)
  await waitFor('!!document.querySelector(".fox-local-kb-import-dialog")','导入弹窗')
  await click('.fox-kb-file-dropzone button')
  await waitFor('document.querySelectorAll("button[aria-label^=移除]").length===2','导入文件')
  await clickText('开始导入')
  await waitFor('window.__libraryFixture.operations.some(o=>o.action==="import")','导入请求')
  if(!await client.evaluate('window.__libraryFixture.operations.find(o=>o.action==="import").request.files.every(f=>f.relativePath.startsWith("交互检查/"))'))throw Error('导入丢失当前目录')
  results.push({check:'combined-add-menu-folder-and-file-actions',passed:true})
  await navigate('view=home')
  await waitFor('!![...document.querySelectorAll("button")].find(e=>e.textContent==="看看使用指南")','使用指南入口')
  await clickText('看看使用指南')
  await waitFor('!!document.querySelector(".fox-library-detail-topbar")','指南统一详情')
  if(!await client.evaluate('window.__libraryFixture.route.view==="local-knowledge-detail" && window.__libraryFixture.route.id==="fox-user-guide"'))throw Error('指南仍走旧路由')
  await screenshot('02-guide-unified-detail')
  await navigate('view=documents&documentId=fixture-document-3')
  await waitFor('!!document.querySelector(".fox-library-preview-dialog")','旧文档链接兼容')
  if(!await client.evaluate('!!document.querySelector(".fox-library-detail-topbar")'))throw Error('旧文档链接仍打开独立页面')
  await escape()
  await waitFor('!document.querySelector(".fox-library-preview-dialog")','关闭文档预览')
  results.push({check:'guide-and-legacy-document-route-use-unified-detail',passed:true})
  await navigate('view=detail')
  const geometry=await sample({view:'.fox-library-view-switch',add:'button[aria-label="添加文件或文件夹"]',search:'.fox-kb-expanding-search > div:first-child',tree:'.fox-library-sidebar-tree.is-active'})
  if(geometry.view.x+geometry.view.width>geometry.add.x||geometry.add.x-geometry.view.x-geometry.view.width>20)throw Error('视图切换不在加号左侧')
  if(geometry.search.width>30||geometry.search.border!=='rgba(0, 0, 0, 0)')throw Error('搜索按钮没有缩小和去边框 '+JSON.stringify(geometry.search))
  if(!await client.evaluate(`(()=>{const t=document.querySelector('.fox-library-sidebar-tree.is-active');return t && getComputedStyle(t).backgroundColor!=='rgba(0, 0, 0, 0)' && t.getBoundingClientRect().height>60})()`))throw Error('选中资料库未覆盖文件树')
  results.push({check:'toolbar-order-small-search-and-whole-tree-background',geometry,passed:true})
  await navigate('view=detail&theme=paper', 600)
  await screenshot('10-library-narrow-paper')
  await navigate('view=detail')
  if (!process.env.INTERACTIONS_ONLY) {
  const libraryThemes=[]
  for(const dark of [false,true])for(const name of palette){
    await theme(name,dark)
    await click('input[aria-label="选择当前列表中的全部文件"]')
    const state=await sample({add:'button[aria-label="添加文件或文件夹"]',view:'.fox-library-view-switch [aria-pressed=true] .fox-library-view-indicator',status:'.fox-library-file-status.is-ready > svg',checkbox:'.fox-library-manager input[type=checkbox]'})
    await click('button[aria-label="资料库设置"]')
    await waitFor('!!document.querySelector("#local-knowledge-base-edit-name")','主题设置弹窗')
    const modal=await sample({dialog:'.fox-kb-settings-dialog',save:'.fox-kb-settings-dialog button[type=submit]'})
    libraryThemes.push({name,dark,state,modal})
    await screenshot(`03-library-settings-${name}-${dark?'dark':'light'}`)
    await escape()
    await waitFor('!document.querySelector(".fox-kb-settings-dialog")','关闭设置')
  }
  for(const dark of [false,true]){
    const group=libraryThemes.filter(item=>item.dark===dark)
    for(const key of ['add','view'])if(new Set(group.map(t=>t.state[key].background)).size!==3)throw Error(`${key} 主色未随三个主题切换`)
    if(new Set(group.map(t=>t.state.status.color)).size!==3||new Set(group.map(t=>t.state.checkbox.accent)).size!==3)throw Error('文件状态或勾选颜色未切换')
    if(new Set(group.map(t=>t.modal.dialog.shadow)).size!==3)throw Error('弹窗阴影未切换')
  }
  results.push({check:'library-six-live-theme-states',samples:libraryThemes,passed:true})
  await client.send('Page.navigate',{url:`http://127.0.0.1:${port}/tests/fixtures/theme-refinements.html`})
  await waitFor('!!document.querySelector(".fox-shadcn-agent-card")','专家组件')
  const expertThemes=[]
  for(const dark of [false,true])for(const name of palette){
    await theme(name,dark)
    const point=await client.evaluate('(()=>{const r=document.querySelector(".fox-shadcn-agent-card").getBoundingClientRect();return {x:r.x+50,y:r.y+50}})()')
    await client.send('Input.dispatchMouseEvent',{type:'mouseMoved',...point})
    await pause(200)
    const card=await sample({card:'.fox-shadcn-agent-card',start:'.fox-agent-summon',create:'.fox-agent-toolbar button[data-variant=default]'})
    await click('button[aria-label="查看资料整理专家详情"]')
    await waitFor('!!document.querySelector(".fox-agent-view-dialog")','专家详情')
    const dialog=await sample({dialog:'.fox-agent-view-dialog',start:'.fox-agent-view-dialog [data-variant=default]',avatar:'.fox-agent-view-avatar'})
    expertThemes.push({name,dark,card,dialog})
    await screenshot(`04-expert-detail-${name}-${dark?'dark':'light'}`)
    await escape()
    await waitFor('!document.querySelector(".fox-agent-view-dialog")','关闭专家详情')
  }
  results.push({check:'expert-six-live-theme-states',samples:expertThemes,passed:true})
  await click('button[aria-label="查看资料整理专家详情"]')
  await waitFor('!!document.querySelector(".fox-agent-view-dialog")','管理入口')
  await clickText('管理专家')
  await waitFor('!!document.querySelector(".fox-agent-manage-page")','专家管理页')
  await screenshot('05-expert-management')
  await client.send('Page.navigate',{url:`http://127.0.0.1:${port}/tests/fixtures/theme-refinements.html?theme=paper`})
  await waitFor('!!document.querySelector(".fox-shadcn-agent-card")','专家列表')
  await clickText('数字同事')
  await waitFor('!!document.querySelector(".fox-agent-editor-dialog")','数字同事弹窗')
  const colleagueThemes=[]
  for(const name of palette){await theme(name);colleagueThemes.push({name,...await sample({icon:'.fox-agent-editor-resources > section > header svg',dialog:'.fox-agent-editor-dialog'})})}
  if(new Set(colleagueThemes.map(x=>x.icon.color)).size!==3)throw Error('数字同事图标未切换')
  await theme('paper');await screenshot('06-colleagues-paper')
  results.push({check:'colleague-icon-theme',samples:colleagueThemes,passed:true})
  }
  }
  if (!process.env.INTERACTIONS_ONLY) {
  await client.send('Page.navigate',{url:`http://127.0.0.1:${port}/tests/fixtures/theme-refinements.html?surface=plugins`})
  await waitFor('!!document.querySelector(".fox-plugin-icon, .fox-plugin-installed-card-icon")','插件组件')
  const pluginThemes=[]
  for(const name of palette){await theme(name);pluginThemes.push({name,...await sample({icon:'.fox-plugin-icon, .fox-plugin-installed-card-icon'})});await screenshot(`07-plugins-${name}`)}
  if(new Set(pluginThemes.map(x=>x.icon.background)).size!==3)throw Error('插件图标背景未切换')
  results.push({check:'plugin-icon-theme',samples:pluginThemes,passed:true})
  await client.send('Page.navigate',{url:`http://127.0.0.1:${port}/tests/fixtures/theme-refinements.html?surface=composer`})
  await waitFor('!!document.querySelector(".fox-chat-textarea")','输入框样式夹具')
  await click('.fox-chat-textarea')
  const composerThemes=[]
  for(const dark of [false,true])for(const name of palette){await theme(name,dark);composerThemes.push({name,dark,...await sample({input:'.fox-prompt-input > [data-slot=input-group]',send:'.fox-send-button'})})}
  for(const dark of [false,true]){const group=composerThemes.filter(t=>t.dark===dark);if(new Set(group.map(x=>x.input.border)).size!==3||new Set(group.map(x=>x.send.background)).size!==3)throw Error('输入框边框或发送颜色未切换')}
  await theme('paper');await screenshot('08-composer-paper')
  results.push({check:'composer-six-theme-style-probes',samples:composerThemes,passed:true})

  const contrasts=[]
  for(const dark of [false,true])for(const name of palette){
    await theme(name,dark)
    const contrast=await client.evaluate(`(()=>{const c=document.createElement('canvas').getContext('2d');const root=getComputedStyle(document.documentElement);const lum=token=>{c.fillStyle=root.getPropertyValue(token).trim();c.fillRect(0,0,1,1);const rgb=[...c.getImageData(0,0,1,1).data].slice(0,3).map(v=>{v/=255;return v<=.04045?v/12.92:((v+.055)/1.055)**2.4});return rgb[0]*.2126+rgb[1]*.7152+rgb[2]*.0722};const a=lum('--primary'),b=lum('--primary-foreground');return (Math.max(a,b)+.05)/(Math.min(a,b)+.05)})()`)
    contrasts.push({name,dark,contrast})
    if(contrast<4.5)throw Error('主按钮文字对比度不足 '+JSON.stringify({name,dark,contrast}))
  }
  results.push({check:'primary-button-text-contrast-six-palettes',samples:contrasts,passed:true})
  await client.send('Page.navigate',{url:`http://127.0.0.1:${port}/tests/fixtures/theme-refinements.html?theme=paper`})
  await waitFor('!!document.querySelector(".fox-shadcn-agent-card")','新建专家入口')
  await clickText('新建专家')
  await waitFor('!!document.querySelector(".fox-agent-editor-dialog")','新建专家弹窗')
  await screenshot('09-create-expert-paper')
  const createThemes=[]
  for(const name of palette){await theme(name);createThemes.push({name,...await sample({dialog:'.fox-agent-editor-dialog',save:'.fox-agent-editor-dialog [data-variant=default]'})})}
  if(new Set(createThemes.map(x=>x.dialog.shadow)).size!==3)throw Error('新建专家弹窗阴影未切换')
  results.push({check:'create-expert-dialog-theme',samples:createThemes,passed:true})
  const experts=results.find(x=>x.check==='expert-six-live-theme-states').samples
  for(const dark of [false,true]){const group=experts.filter(x=>x.dark===dark);for(const key of ['start','create'])if(new Set(group.map(x=>x.card[key]?.background)).size!==3)throw Error('专家按钮未切换 '+key);if(new Set(group.map(x=>x.card.card.shadow)).size!==3)throw Error('专家卡片阴影未切换')}
  }
  if(results.some(item=>item.overflow))throw Error('页面横向溢出')
  if(client.errors.length)throw Error(JSON.stringify(client.errors))
  writeFileSync(join(output,process.env.INTERACTIONS_ONLY ? 'interaction-report.json' : 'report.json'),JSON.stringify({passed:true,evidence:'real page components with synthetic APIs; composer style probe; no production backend acceptance',results},null,2))
  process.stdout.write(JSON.stringify({passed:true,screenshots:results.filter(r=>r.screenshot).length,output},null,2))
} catch (error) {
  writeFileSync(join(output, process.env.INTERACTIONS_ONLY ? 'interaction-report.json' : 'report.json'), JSON.stringify({ passed: false, error: String(error), diagnostics, results }, null, 2))
  process.stderr.write(`${error.stack ?? error}\n`)
  process.exitCode = 1
} finally { client?.close(); browser?.kill(); server?.kill() }
