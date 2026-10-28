// Narrow CDP driver for the isolated desktop acceptance window.
import { writeFile } from 'node:fs/promises'
import path from 'node:path'
import { DatabaseSync } from 'node:sqlite'

const args = process.argv.slice(2)
const command = args[0]
const value = (name, fallback = undefined) => {
  const at = args.indexOf(name)
  return at < 0 ? fallback : args[at + 1]
}
const port = Number(value('--port', '9222'))
const delay = (ms) => new Promise((resolve) => setTimeout(resolve, ms))
const deadlineFetch = async (url) => fetch(url, { signal: AbortSignal.timeout(5000) })
let socket
let nextId = 0
const pending = new Map()

try {
  const targets = await (await deadlineFetch(`http://127.0.0.1:${port}/json`)).json()
  const pages = targets.filter((target) => target.type === 'page' && target.url.startsWith('http://127.0.0.1:1421/'))
  if (pages.length !== 1) throw new Error(`expected exactly one Fox WebView, found ${pages.length}`)
  socket = new WebSocket(pages[0].webSocketDebuggerUrl)
  socket.addEventListener('message', ({ data }) => {
    const item = JSON.parse(data)
    if (!item.id || !pending.has(item.id)) return
    const entry = pending.get(item.id)
    clearTimeout(entry.timer)
    pending.delete(item.id)
    item.error ? entry.reject(new Error(JSON.stringify(item.error))) : entry.resolve(item.result)
  })
  await new Promise((resolve, reject) => { socket.addEventListener('open', resolve, { once: true }); socket.addEventListener('error', reject, { once: true }) })

  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++nextId
    const timer = setTimeout(() => { pending.delete(id); reject(new Error(`CDP timeout: ${method}`)) }, 10000)
    pending.set(id, { resolve, reject, timer })
    socket.send(JSON.stringify({ id, method, params }))
  })
  const evaluate = async (expression) => {
    const response = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true })
    if (response.exceptionDetails) throw new Error(response.exceptionDetails.text)
    return response.result.value
  }
  const probe = () => evaluate(`(() => {
    const visible = el => !!el && el.getClientRects().length > 0 && getComputedStyle(el).visibility !== 'hidden'
    const read = selector => [...document.querySelectorAll(selector)].filter(visible)
    const input = read('textarea').at(-1)
    const send = read('button.fox-send-button').at(-1)
    const users = read('.fox-thread-msg.is-user, .fox-user-message')
    const assistants = read('.fox-thread-msg.is-assistant, .fox-assistant-message')
    return { title: document.querySelector('.fox-chat-title')?.innerText ?? null,
      text: document.body.innerText.slice(-12000),
      inputs: read('textarea').map(el => ({value: el.value, placeholder: el.placeholder, disabled: el.disabled})),
      send: send && {label: send.getAttribute('aria-label'), disabled: send.disabled, title: send.title},
      users: users.length, assistants: assistants.length,
      turnIds: [...document.querySelectorAll('[data-turn-message-id]')].filter(el => visible(el.querySelector('.fox-thread-msg, .fox-message'))).map(el => el.getAttribute('data-turn-message-id')),
      lastAssistant: assistants.at(-1)?.innerText?.slice(-6000) ?? null,
      input: input && {value: input.value, disabled: input.disabled},
      dialogs: read('[role="alertdialog"], [role="dialog"]').map(el => el.innerText),
      menuItems: read('[role="menuitem"]').map(el => ({text: el.innerText, disabled: el.getAttribute('data-disabled') !== null || el.getAttribute('aria-disabled') === 'true', title: el.title, aria: el.getAttribute('aria-label')})) }
  })()`)
  const waitFor = async (predicate, milliseconds = 12000) => {
    const until = Date.now() + milliseconds
    let last
    do { last = await predicate(); if (last?.ok) return last; await delay(250) } while (Date.now() < until)
    throw new Error(`timed out after ${milliseconds} ms; last=${JSON.stringify(last)}`)
  }
  const point = async (selector, exactText) => evaluate(`(() => {
    const candidates = [...document.querySelectorAll(${JSON.stringify(selector)})].filter(el => el.getClientRects().length > 0 && getComputedStyle(el).visibility !== 'hidden')
    const matches = ${exactText === undefined ? 'candidates' : `candidates.filter(el => (el.innerText || el.textContent || '').trim() === ${JSON.stringify(exactText)})`}
    if (matches.length !== 1) return {ok:false, reason:'expected one matching visible element', count:matches.length, candidates:candidates.map(el => ({text:(el.innerText||'').slice(0,120), aria:el.getAttribute('aria-label')})).slice(0,20)}
    const el = matches[0]
    if (el.disabled || el.getAttribute('aria-disabled') === 'true' || el.hasAttribute('data-disabled')) return {ok:false, reason:'element disabled', title:el.title, aria:el.getAttribute('aria-label'), text:el.innerText}
    el.scrollIntoView({block:'center', inline:'nearest'})
    const r = el.getBoundingClientRect(), x = r.left + r.width/2, y = r.top + r.height/2
    const hit = document.elementFromPoint(x,y)
    if (!hit || !(el === hit || el.contains(hit))) return {ok:false, reason:'element occluded', target:el.outerHTML.slice(0,300), hit:hit?.outerHTML?.slice(0,300), x,y}
    return {ok:true, x, y, text:(el.innerText||'').slice(0,120), aria:el.getAttribute('aria-label')}
  })()`)
  const click = async (selector, exactText) => {
    const p = await point(selector, exactText)
    if (!p?.ok) throw new Error(`click ${selector}: ${JSON.stringify(p)}`)
    await send('Input.dispatchMouseEvent', {type:'mouseMoved', x:p.x, y:p.y})
    await send('Input.dispatchMouseEvent', {type:'mousePressed', x:p.x, y:p.y, button:'left', clickCount:1})
    await send('Input.dispatchMouseEvent', {type:'mouseReleased', x:p.x, y:p.y, button:'left', clickCount:1})
    return p
  }
  const type = async (selector, text) => {
    await click(selector)
    await send('Input.dispatchKeyEvent', {type:'keyDown', key:'a', code:'KeyA', modifiers:2, windowsVirtualKeyCode:65, nativeVirtualKeyCode:65})
    await send('Input.dispatchKeyEvent', {type:'keyUp', key:'a', code:'KeyA', modifiers:2, windowsVirtualKeyCode:65, nativeVirtualKeyCode:65})
    await send('Input.insertText', {text})
    const result = await evaluate(`(() => {const el=[...document.querySelectorAll(${JSON.stringify(selector)})].find(item=>item.getClientRects().length > 0 && getComputedStyle(item).visibility !== 'hidden');return {ok:el?.value===${JSON.stringify(text)}, value:el?.value}})()`)
    if (!result.ok) throw new Error(`input readback mismatch: ${JSON.stringify(result)}`)
    return result
  }
  const screenshot = async (path) => {
    const data = await send('Page.captureScreenshot', {format:'png', captureBeyondViewport:false})
    await writeFile(path, Buffer.from(data.data, 'base64'))
    return path
  }
  const assertIdentity = (state) => {
    const expectedTitle = value('--expect-title')
    const expectedConversation = value('--expect-conversation')
    const data = value('--data')
    if (!expectedTitle || !expectedConversation || !data || !path.isAbsolute(data)) throw new Error('requires --expect-title, --expect-conversation, and absolute --data')
    if (state.title !== expectedTitle) throw new Error(`conversation title mismatch: expected=${JSON.stringify(expectedTitle)} actual=${JSON.stringify(state.title)}`)
    const db = new DatabaseSync(path.join(data, 'fox.db'), {readOnly:true})
    const find = db.prepare('SELECT conversation_id FROM messages WHERE id=?')
    const ids = state.turnIds.map(id => ({id, conversation:find.get(id)?.conversation_id})).filter(item => item.conversation)
    db.close()
    if (!ids.some(item => item.conversation === expectedConversation) || ids.some(item => item.conversation !== expectedConversation)) throw new Error(`conversation identity mismatch: expected=${expectedConversation} anchors=${JSON.stringify(ids)}`)
    return ids.length
  }

  let result
  if (command === 'probe') result = await probe()
  else if (command === 'inspect') result = await evaluate(`([...document.querySelectorAll(${JSON.stringify(value('--selector'))})].map(el=>({tag:el.tagName,text:el.innerText,outerHTML:el.outerHTML,role:el.getAttribute('role'),tabIndex:el.getAttribute('tabindex'),cursor:getComputedStyle(el).cursor,rect:el.getBoundingClientRect().toJSON()})))`)
  else if (command === 'screenshot') result = {screenshot: await screenshot(value('--out'))}
  else if (command === 'click') result = {clicked: await click(value('--selector'), value('--text')), after: await probe()}
  else if (command === 'guarded-click') {
    const verifiedAnchors = assertIdentity(await probe())
    result = {expectedConversation:value('--expect-conversation'), verifiedAnchors, clicked:await click(value('--selector'), value('--text')), after:await probe()}
  }
  else if (command === 'hover') {
    const target = await point(value('--selector'), value('--text'))
    if (!target?.ok) throw new Error(`hover target: ${JSON.stringify(target)}`)
    await send('Input.dispatchMouseEvent', {type:'mouseMoved', x:target.x, y:target.y})
    result = {hovered:target, after:await probe()}
  }
  else if (command === 'type') result = await type(value('--selector'), value('--text', ''))
  else if (command === 'reset') {
    const start = await probe()
    const verifiedAnchors = assertIdentity(start)
    const menu = await click('[data-testid="conversation-menu"]')
    const option = await waitFor(async () => {
      const p = await point('[data-testid="reset-context"]')
      if (p?.reason === 'element disabled') throw new Error(`reset unavailable: ${JSON.stringify(p)}`)
      return p
    })
    await click('[data-testid="reset-context"]')
    await waitFor(async () => ({ok: (await probe()).dialogs.some(text => text.includes('重置会话上下文'))}))
    if (value('--out')) await screenshot(value('--out'))
    assertIdentity(await probe())
    const confirm = await click('[role="alertdialog"] button', '重置上下文')
    await waitFor(async () => ({ok: !(await probe()).dialogs.some(text => text.includes('重置会话上下文'))}))
    result = {title:start.title, expectedConversation:value('--expect-conversation'), verifiedAnchors, menu, option, confirm, after:await probe()}
  } else if (command === 'ask') {
    const before = await probe()
    const text = value('--text', '')
    const expectedTitle = value('--expect-title')
    const expectedConversation = value('--expect-conversation')
    if (!text) throw new Error('empty prompt')
    const verifiedAnchors = assertIdentity(before)
    await type('textarea', text)
    const armed = await waitFor(async () => { const p=await probe(); assertIdentity(p); return {ok:p.send?.label==='Submit' && p.send?.disabled===false, probe:p} })
    await click('button.fox-send-button')
    const cleared = await waitFor(async () => {const p=await probe(); return {ok:p.input?.value==='', probe:p}}, 8000)
    result = {before, expectedConversation, verifiedAnchors, armed:armed.probe.send, cleared:cleared.probe.input, after:await probe()}
  } else throw new Error('expected probe, inspect, screenshot, click, guarded-click, hover, type, reset, or ask')
  console.log(JSON.stringify({ok:true, command, target:{title:pages[0].title,url:pages[0].url}, result}, null, 2))
} catch (error) {
  console.error(JSON.stringify({ok:false, command, error:String(error?.message ?? error)}, null, 2))
  process.exitCode = 1
} finally {
  socket?.close()
}
