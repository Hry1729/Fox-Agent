import { spawn } from 'node:child_process'
import { mkdirSync, mkdtempSync, writeFileSync } from 'node:fs'
import { dirname, resolve, join } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')
const output = resolve(root, '../../output/provider-protocols/browser')
const port = 1489, cdpPort = 9397
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
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP 超时：${method}`)) }, 60000)
    pending.set(id, { resolve, reject, timer }); socket.send(JSON.stringify({ id, method, params }))
  })
  const evaluate = async (expression) => { const result = await send('Runtime.evaluate', { expression, returnByValue: true, awaitPromise: true }); if (result.exceptionDetails) throw new Error(result.exceptionDetails.exception?.description ?? result.exceptionDetails.text); return result.result.value }
  return { send, evaluate, errors, close: () => socket.close() }
}
async function waitFor(expression, description) {
  for (let index = 0; index < 200; index++) { if (await client.evaluate(expression)) return; await pause(120) }
  throw new Error(`${description} 未就绪；${JSON.stringify(client.errors)}；${await client.evaluate('document.body.innerText.slice(0,1500)')}`)
}
async function screenshot(name) {
  await pause(650)
  const shot = await client.send('Page.captureScreenshot', { format: 'png', captureBeyondViewport: false })
  const path = join(output, `${name}.png`)
  writeFileSync(path, Buffer.from(shot.data, 'base64'))
  results.push({ screenshot: name, path, overflow: await client.evaluate('document.documentElement.scrollWidth>innerWidth+1') })
}
try {
  server = spawn(process.execPath, ['--input-type=module', '-e', `import {createServer} from 'vite'; const server = await createServer({cacheDir: ${JSON.stringify(join(output, 'vite-cache'))}, server:{host:'127.0.0.1',port:${port},strictPort:true}}); await server.listen();`], { cwd: root, stdio: ['ignore','ignore','pipe'], windowsHide:true })
  server.stderr.on('data', data => {diagnostics = (diagnostics + data).slice(-5000)})
  const profile = mkdtempSync(join(output, 'chrome-profile-'))
  browser = spawn(process.env.CHROME_PATH || 'C:/Program Files/Google/Chrome/Application/chrome.exe', ['--headless=new','--disable-gpu','--no-first-run','--no-default-browser-check',`--remote-debugging-port=${cdpPort}`,`--user-data-dir=${profile}`,'about:blank'],{stdio:['ignore','ignore','pipe'],windowsHide:true})
  browser.stderr.on('data',data => {diagnostics=(diagnostics+data).slice(-5000)})
  client=await connect(); await client.send('Page.enable'); await client.send('Runtime.enable')
  for(let i=0;i<160;i++){try{if((await fetch(`http://127.0.0.1:${port}/tests/fixtures/provider-protocols.html`)).ok)break}catch{}await pause(200)}
  await client.send('Emulation.setDeviceMetricsOverride',{width:1200,height:1000,deviceScaleFactor:1,mobile:false})
  await client.send('Page.navigate',{url:`http://127.0.0.1:${port}/tests/fixtures/provider-protocols.html`})
  await waitFor(`!!window.protocolFixture && !!document.querySelector('.fox-provider-editor')`,'协议编辑器')
  const clickText = async text => {await client.evaluate(`(()=>{const b=[...document.querySelectorAll('button')].find(b=>b.textContent===${JSON.stringify(text)});if(!b)throw Error('missing button');b.click()})()`);await pause(100)}
  const fill = async (selector,value) => {await client.evaluate(`(()=>{const e=document.querySelector(${JSON.stringify(selector)});if(!e)throw Error('missing input');Object.getOwnPropertyDescriptor(HTMLInputElement.prototype,'value').set.call(e,${JSON.stringify(value)});e.dispatchEvent(new Event('input',{bubbles:true}))})()`);await pause(100)}
  await clickText('添加协议')
  await fill('.fox-protocol-fields input:not([type="checkbox"])','codex-model')
  await fill('.fox-protocol-fields label.is-wide input','https://example.com/v1')
  await client.evaluate(`document.querySelectorAll('.fox-protocol-assignments [role="switch"]')[1].click()`)
  await pause(100)
  await clickText('验证此协议')
  await waitFor(`document.body.innerText.includes('连接验证通过')`,'协议验证结果')
  await fill('.fox-protocol-fields input:not([type="checkbox"])','codex-model-2')
  if(await client.evaluate(`document.body.innerText.includes('连接验证通过')`))throw Error('编辑模型后仍显示旧验证结果')
  await clickText('添加协议')
  await client.evaluate(`document.querySelectorAll('.fox-protocol-fields [role="combobox"]')[1].click()`)
  await waitFor(`!!document.querySelector('[role="option"]')`,'协议选项')
  await client.evaluate(`[...document.querySelectorAll('[role="option"]')].find(e=>e.textContent.includes('Anthropic')).click()`)
  await pause(150)
  await fill('.fox-protocol-fields:nth-of-type(2) input:not([type="checkbox"])','dsh-model')
  await fill('.fox-protocol-fields:nth-of-type(2) label.is-wide input','https://example.com/anthropic')
  await client.evaluate(`document.querySelectorAll('.fox-protocol-fields')[1].querySelectorAll('[role="switch"]')[1].click()`)
  await pause(150)
  await clickText('保存')
  const saved=await client.evaluate(`window.protocolFixture.saved`)
  if(saved.apiType!=='anthropic-messages'||saved.models[0].modelId!=='native-model')throw Error('Fox 原生连接被改写')
  if(saved.protocolProfiles[0].backends.join()!=='codex'||saved.protocolProfiles[1].backends.join()!=='deepseek')throw Error(JSON.stringify(saved.protocolProfiles))
  results.push({check:'two protocol profiles, independent model/address, bindings, unchanged native model, stale test invalidation',passed:true})
  await screenshot('desktop-protocols')
  await client.send('Emulation.setDeviceMetricsOverride',{width:760,height:800,deviceScaleFactor:1,mobile:false})
  await pause(200)
  const overflow=await client.evaluate(`document.documentElement.scrollWidth>innerWidth`)
  if(overflow)throw Error('小窗口横向溢出')
  await screenshot('narrow-protocols')
  results.push({check:'760px window has no horizontal overflow',passed:true})
  await client.evaluate(`document.querySelectorAll('.fox-protocol-fields')[1].querySelectorAll('[role="switch"]')[2].click()`)
  await pause(100)
  if(!await client.evaluate(`document.querySelectorAll('.fox-protocol-fields')[1].querySelector('.fox-protocol-actions button').disabled`))throw Error('独立 Key 为空时仍允许使用共享 Key 验证')
  await fill('.fox-protocol-fields:nth-of-type(2) input[type="password"]','synthetic-fixture-key')
  await client.evaluate(`document.querySelectorAll('.fox-protocol-fields')[1].querySelectorAll('[role="switch"]')[2].click()`)
  await pause(100)
  if(await client.evaluate(`Object.keys(window.protocolFixture.form.protocolKeys).length`))throw Error('切回共享 Key 后仍提交独立 Key')
  results.push({check:'switching back to shared credential removes unsaved separate key',passed:true})
  if(client.errors.length)throw Error(JSON.stringify(client.errors))
  writeFileSync(join(output,'results.json'),JSON.stringify({results,exceptions:client.errors},null,2))
  console.log(JSON.stringify({results,output}))
} finally {client?.close(); browser?.kill(); server?.kill()}
