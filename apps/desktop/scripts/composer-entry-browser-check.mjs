import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/composer-entry-20261018/browser')
const pause = ms => new Promise(resolve => setTimeout(resolve, ms))
mkdirSync(output, { recursive: true })
let server, browser, socket, serial = 0
const pending = new Map(), errors = [], results = []
const send = (method, params = {}) => new Promise((resolve, reject) => {
  const id = ++serial
  const timer = setTimeout(() => { pending.delete(id); reject(Error(`CDP timeout: ${method}`)) }, 20000)
  pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }))
})
const evaluate = async expression => {
  const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true })
  if (result.exceptionDetails) throw Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text)
  return result.result.value
}
async function check(expression, label) {
  for (let i = 0; i < 100; i++) { if (await evaluate(expression)) { results.push({ check: label, passed: true }); return }; await pause(100) }
  throw Error(`${label}: ${await evaluate('document.body.innerText.slice(0,1800)')}`)
}
async function click(selector) {
  const point = await evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)throw Error('missing '+${JSON.stringify(selector)});const r=e.getBoundingClientRect();return {x:r.x+r.width/2,y:r.y+r.height/2}})()`)
  await send('Input.dispatchMouseEvent', { type: 'mouseMoved', ...point })
  await send('Input.dispatchMouseEvent', { type: 'mousePressed', ...point, button: 'left', clickCount: 1 })
  await send('Input.dispatchMouseEvent', { type: 'mouseReleased', ...point, button: 'left', clickCount: 1 }); await pause(200)
}
async function escape() { await send('Input.dispatchKeyEvent', { type: 'keyDown', key: 'Escape' }); await send('Input.dispatchKeyEvent', { type: 'keyUp', key: 'Escape' }); await pause(200) }
async function screenshot(name) {
  await pause(300)
  const shot = await send('Page.captureScreenshot', { format: 'png' })
  writeFileSync(join(output, `${name}.png`), Buffer.from(shot.data, 'base64'))
}
try {
  server = spawn(process.execPath, [join(root, 'node_modules/vite/bin/vite.js'), '--host', '127.0.0.1', '--port', '1480', '--strictPort'], { cwd: root, stdio: 'ignore', windowsHide: true })
  browser = spawn('C:/Program Files/Google/Chrome/Application/chrome.exe', ['--headless=new', '--disable-gpu', '--no-first-run', '--no-default-browser-check', '--remote-debugging-port=9386', `--user-data-dir=${mkdtempSync(join(output, 'chrome-profile-'))}`, 'about:blank'], { stdio: 'ignore', windowsHide: true })
  let target
  for (let i = 0; i < 100; i++) { try { target = (await fetch('http://127.0.0.1:9386/json').then(r => r.json())).find(t => t.type === 'page'); if (target && (await fetch('http://127.0.0.1:1480/tests/fixtures/composer-entry.html')).ok) break } catch {} await pause(200) }
  if (!target) throw Error('browser unavailable')
  socket = new WebSocket(target.webSocketDebuggerUrl)
  await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', reject, { once: true }) })
  socket.addEventListener('message', ({ data }) => {
    const response = JSON.parse(data), item = pending.get(response.id)
    if (response.method === 'Runtime.exceptionThrown') errors.push(response.params.exceptionDetails)
    if (item) { clearTimeout(item.timer); pending.delete(response.id); response.error ? item.reject(Error(JSON.stringify(response.error))) : item.resolve(response.result) }
  })
  await send('Runtime.enable'); await send('Page.enable')
  await send('Emulation.setDeviceMetricsOverride', { width: 1120, height: 800, deviceScaleFactor: 1, mobile: false })
  await send('Page.navigate', { url: 'http://127.0.0.1:1480/tests/fixtures/composer-entry.html' })
  await check(`!!window.composerFixture && document.querySelector('[aria-label="选择模型"]')?.textContent.includes('sensenova6.8')`, 'configured model shown')
  await check(`!document.querySelector('.fox-starter-suggestions') && document.querySelector('.fox-expert-chip img')?.getAttribute('src')==='/avatars/fox-assistant.png' && document.querySelector('.fox-execution-control').textContent==='询问'`, 'compact toolbar and default fox avatar')
  await check(`(()=>{const h=document.querySelector('.fox-empty-heading').getBoundingClientRect(), c=document.querySelector('.fox-composer-wrap').getBoundingClientRect(), s=document.querySelector('.fox-chat-stage').getBoundingClientRect();return Math.abs((h.top+c.bottom)/2-(s.top+s.height/2))<3})()`, 'empty page content vertically centered')
  await click('.fox-execution-control')
  await check(`document.querySelector('.fox-execution-menu')?.textContent.includes('执行前询问自动执行只读模式')`, 'execution menu retains full labels')
  await screenshot('execution-menu'); await escape()
  await check(`(()=>{const b=document.querySelector('.is-new-conversation .fox-send-button'),s=getComputedStyle(b);return s.color==='rgb(255, 255, 255)' && s.backgroundColor!=='rgba(0, 0, 0, 0)' && !document.querySelector('.fox-send-beam, .fox-send-metal')})()`, 'send is solid with white arrow and no decorative wrapper')
  await click('textarea'); await send('Input.insertText', { text: '保留这个草稿' })
  await evaluate(`(()=>{const input=document.querySelector('input[type=file]');const dt=new DataTransfer();dt.items.add(new File(['test'],'说明.txt',{type:'text/plain'}));input.files=dt.files;input.dispatchEvent(new Event('change',{bubbles:true}))})()`)
  await check(`document.querySelector('.fox-composer-attachments')?.textContent.includes('说明.txt')`, 'attachment displayed')
  await click('.fox-composer-project-trigger')
  await check(`!!document.querySelector('.fox-composer-project-menu')`, 'project picker opens')
  await check(`getComputedStyle(document.querySelector('.fox-composer-project-menu [cmdk-item]')).cursor==='pointer'`, 'enabled project uses pointer cursor')
  await check(`(()=>{const bar=document.querySelector('.fox-composer-project-bar').getBoundingClientRect(), input=document.querySelector('.fox-prompt-input').getBoundingClientRect();return bar.left>=input.left+15 && bar.right<=input.right-15 && bar.bottom-input.bottom<=38})()`, 'project strip inset and compact')
  await screenshot('project-menu')
  await click('[cmdk-item][data-value*="项目乙"]')
  await check(`window.composerFixture.project==='D:/测试/项目乙' && window.composerFixture.draft==='保留这个草稿' && document.querySelector('.fox-composer-attachments')?.textContent.includes('说明.txt') && !document.querySelector('.fox-composer-attachments')?.textContent.includes('项目乙')`, 'project selection preserves draft and attachment outside input')
  await click('.fox-composer-project-trigger'); await click('.fox-composer-project-add')
  await check(`window.composerFixture.project==='D:/测试/新增项目'`, 'add project callback')
  await click('[aria-label="添加内容"]')
  await check(`JSON.stringify([...document.querySelectorAll('.fox-add-menu [role=menuitem]')].map(e=>e.textContent.trim()))===JSON.stringify(['添加附件','添加资料库'])`, 'plus menu contains exactly two actions')
  await screenshot('add-menu')
  await click('.fox-add-menu [role=menuitem]:last-child')
  await check('window.composerFixture.knowledge', 'knowledge action')
  await click('.fox-expert-chip')
  await check(`JSON.stringify([...document.querySelectorAll('.fox-expert-menu [role=menuitem]')].map(e=>e.textContent))===JSON.stringify(['查看详情','更换专家'])`, 'default expert has existing detail and change actions')
  await click('.fox-expert-menu [role=menuitem]:first-child'); await check('window.composerFixture.viewed', 'default expert detail callback')
  await click('.fox-expert-chip'); await click('.fox-expert-menu [role=menuitem]:last-child')
  await check(`document.querySelector('.fox-expert-chip')?.textContent.includes('代码审查专家')`, 'changed expert displayed')
  await click('[aria-label="选择模型"]')
  await check(`getComputedStyle(document.querySelector('.aui-model-menu [cmdk-item]')).cursor==='pointer' && !document.querySelector('.aui-model-menu [cmdk-item] button')`, 'model row has pointer and a single selection target')
  await screenshot('model-menu')
  await click('[cmdk-item][data-value="provider-test:model-b"]')
  await check(`document.querySelector('[aria-label="选择模型"]')?.textContent.includes('model-test-b') && window.composerMock.calls.filter(c=>c.name==='model_service_save').length===1`, 'model selection saved once and refreshed')
  await evaluate('window.composerMock.failSave=true')
  await click('[aria-label="选择模型"]'); await click('[cmdk-item][data-value="provider-test:model-a"]')
  await check(`window.composerMock.calls.filter(c=>c.name==='model_service_save').length===2 && document.querySelector('[aria-label="选择模型"]')?.textContent.includes('model-test-b')`, 'failed model save retains actual model')
  await screenshot('new-conversation')
  await evaluate('window.composerFixture.setHistory(true)')
  await pause(300)
  console.log(JSON.stringify(await evaluate(`(()=>{const e=document.querySelector('textarea'),g=document.querySelector('.fox-prompt-input > [data-slot="input-group"]');return {textareaHeight:e.getBoundingClientRect().height,textareaMin:getComputedStyle(e).minHeight,textareaFlex:getComputedStyle(e).flex,inputHeight:g.getBoundingClientRect().height,inputPadding:getComputedStyle(g).padding}})()`)))
  await check(`!document.querySelector('.fox-composer-project-bar') && !document.querySelector('.fox-composer-attachments')?.textContent.includes('项目') && document.querySelector('textarea').getBoundingClientRect().height<=42`, 'history hides project and uses compact textarea')
  await check(`(()=>{const plus=document.querySelector('[aria-label="添加内容"] svg').getBoundingClientRect(), arrow=document.querySelector('.lucide-arrow-up').getBoundingClientRect(); return plus.width===arrow.width && plus.height===arrow.height})()`, 'plus and send arrows have equal dimensions')
  await check(`getComputedStyle(document.querySelector('.fox-send-button')).color==='rgb(255, 255, 255)' && !document.querySelector('.fox-send-beam, .fox-send-metal')`, 'history send matches new conversation')
  for (const theme of ['graphite','mist','paper']) {
    await evaluate(`document.documentElement.dataset.colorTheme='${theme}'`)
    await screenshot('history-theme-'+theme)
  }
  await evaluate(`document.documentElement.dataset.colorTheme='graphite'`)
  await screenshot('history')
  await evaluate(`window.composerFixture.setDraft('第一行\\n第二行'.replaceAll('\\n',String.fromCharCode(10)))`)
  await check(`document.querySelector('.fox-composer-wrap').classList.contains('is-expanded') && document.querySelector('textarea').getBoundingClientRect().height>32`, 'two lines expand capsule')
  await evaluate(`window.composerFixture.setDraft(Array.from({length:10},(_,i)=>'第'+i+'行').join(String.fromCharCode(10)))`)
  await check(`document.querySelector('textarea').getBoundingClientRect().height===140 && document.querySelector('textarea').scrollHeight>140 && getComputedStyle(document.querySelector('textarea')).overflowY==='auto'`, 'ten lines capped at six visible lines with scroll')
  await screenshot('history-expanded')
  await evaluate(`window.composerFixture.setDraft('长文本'.repeat(80))`)
  await check(`document.querySelector('.fox-composer-wrap').classList.contains('is-expanded') && document.querySelector('textarea').getBoundingClientRect().height>32`, 'soft wrapped text expands')
  await evaluate(`window.composerFixture.setDraft('短消息')`)
  await check(`!document.querySelector('.fox-composer-wrap').classList.contains('is-expanded') && document.querySelector('textarea').getBoundingClientRect().height===32`, 'short text restores capsule')
  await evaluate(`(()=>{const input=document.querySelector('input[type=file]');const dt=new DataTransfer();dt.items.add(new File(['test'],'附件.txt',{type:'text/plain'}));input.files=dt.files;input.dispatchEvent(new Event('change',{bubbles:true}))})()`)
  await check(`document.querySelector('.fox-composer-wrap').classList.contains('is-expanded')`, 'attachment expands capsule')
  await screenshot('history-attachment')
  await click('[aria-label="添加内容"]')
  await check(`!!document.querySelector('.fox-history-add-menu .fox-expert-chip') && !!document.querySelector('.fox-history-add-menu [aria-label="选择模型"]')`, 'history expert and model moved into plus menu')
  await click('.fox-history-add-menu [aria-label="选择模型"]')
  await check(`!!document.querySelector('.aui-model-menu')`, 'history nested model picker opens')
  await screenshot('history-model-menu'); await escape(); await escape()
  await evaluate('window.composerFixture.setRunning(true)')
  await click('[aria-label="添加内容"]')
  await check(`document.querySelector('[aria-label="选择模型"]').disabled`, 'model switching disabled during run')
  await escape()
  await evaluate('window.composerFixture.setRunning(false)')
  await send('Emulation.setDeviceMetricsOverride', { width: 700, height: 560, deviceScaleFactor: 1, mobile: false })
  await check(`document.documentElement.scrollWidth<=innerWidth && document.querySelector('.fox-prompt-toolbar').scrollWidth<=document.querySelector('.fox-prompt-toolbar').clientWidth+1`, 'narrow history toolbar fits')
  await screenshot('narrow-history')
  if (errors.length) throw Error(JSON.stringify(errors))
  writeFileSync(join(output, 'report.json'), JSON.stringify({ passed: true, results, errors }, null, 2))
  console.log(JSON.stringify({ passed: true, results, errors }))
} catch (error) { console.error(error); if (socket?.readyState === 1) await screenshot('failure'); process.exitCode = 1 }
finally { socket?.close(); browser?.kill(); server?.kill() }
