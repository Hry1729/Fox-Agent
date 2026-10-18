// CDP driver for the package-2-D closeout acceptance run.
//
// Drives the REAL WebView of the REAL desktop process over WebView2's remote
// debugging endpoint: it evaluates real DOM queries and clicks real buttons, so
// every action goes through the real frontend code, the real `invoke` IPC, the
// real registered Tauri command and the real database. Nothing here calls a Rust
// function directly and nothing mocks `invoke`.
//
// Node 24 ships a global WebSocket, so no dependency is needed.

const ENDPOINT = process.env.FOX_CDP_ENDPOINT || 'http://127.0.0.1:9333'

async function pageTarget() {
  const list = await (await fetch(`${ENDPOINT}/json/list`)).json()
  const page = list.find((entry) => entry.type === 'page' && entry.title === 'Fox')
  if (!page) throw new Error(`no Fox page target among: ${list.map((e) => e.title).join(', ')}`)
  return page
}

const page = await pageTarget()
const socket = new WebSocket(page.webSocketDebuggerUrl)
let nextId = 1
const pending = new Map()
const consoleLines = []

socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data)
  if (message.id && pending.has(message.id)) {
    const { resolve, reject } = pending.get(message.id)
    pending.delete(message.id)
    if (message.error) reject(new Error(JSON.stringify(message.error)))
    else resolve(message.result)
    return
  }
  if (message.method === 'Runtime.consoleAPICalled') {
    const text = (message.params.args || [])
      .map((arg) => arg.value ?? arg.description ?? JSON.stringify(arg))
      .join(' ')
    consoleLines.push(`[${message.params.type}] ${text}`)
  }
  if (message.method === 'Runtime.exceptionThrown') {
    consoleLines.push(`[exception] ${message.params.exceptionDetails?.text}`)
  }
})

await new Promise((resolve, reject) => {
  socket.addEventListener('open', resolve, { once: true })
  socket.addEventListener('error', () => reject(new Error('CDP socket failed to open')), { once: true })
})

function send(method, params = {}) {
  const id = nextId++
  return new Promise((resolve, reject) => {
    pending.set(id, { resolve, reject })
    socket.send(JSON.stringify({ id, method, params }))
  })
}

async function evaluate(expression) {
  const result = await send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    // `awaitPromise` is deliberately NOT set: every expression here is a
    // synchronous IIFE, and asking CDP to await a non-promise result hangs the
    // WebView2 endpoint instead of returning.
  })
  if (result.exceptionDetails) {
    throw new Error(`evaluate threw: ${result.exceptionDetails.text} ${JSON.stringify(result.exceptionDetails.exception?.value ?? '')}`)
  }
  return result.result.value
}

async function clickByText(selector, text) {
  // A real click on the real element, dispatched by the browser, so React's
  // synthetic event system and the component's own handler both run.
  const found = await evaluate(`(() => {
    const nodes = [...document.querySelectorAll(${JSON.stringify(selector)})]
    const hit = nodes.find((node) => (node.textContent || '').includes(${JSON.stringify(text)}))
    if (!hit) return null
    hit.scrollIntoView({ block: 'center' })
    hit.click()
    return { tag: hit.tagName, text: (hit.textContent || '').trim().slice(0, 80), disabled: !!hit.disabled }
  })()`)
  if (!found) throw new Error(`no ${selector} containing "${text}"`)
  return found
}

async function snapshot(label) {
  const state = await evaluate(`(() => {
    const panel = document.querySelector('.fox-delivery-review')
    const cards = [...document.querySelectorAll('.fox-delivery-acceptance')].map((card) => ({
      header: (card.querySelector('header small') || {}).textContent?.trim() || null,
      verification: (card.querySelector('dd[data-class]') || {}).textContent?.trim() || null,
      acceptance: [...card.querySelectorAll('.fox-delivery-acceptance-facts dd')].map((dd) => dd.textContent.trim()),
      blocked: (card.querySelector('.fox-delivery-acceptance-blocked') || {}).textContent?.trim() || null,
      closed: (card.querySelector('.fox-delivery-acceptance-closed') || {}).textContent?.trim() || null,
      buttons: [...card.querySelectorAll('button')].map((b) => ({ text: (b.textContent || '').trim(), disabled: !!b.disabled })),
    }))
    return {
      url: location.href,
      panelPresent: !!panel,
      panelHeader: panel ? (panel.querySelector('header small') || {}).textContent?.trim() || null : null,
      cards,
      bodyTextLength: document.body.innerText.length,
    }
  })()`)
  console.log(`SNAPSHOT ${label} ${JSON.stringify(state)}`)
  return state
}

/// Evaluate an expression that returns a promise.
///
/// Separate from `evaluate` on purpose: the synchronous evaluator must not set
/// `awaitPromise`, because the WebView2 endpoint hangs on a non-promise result
/// when it is asked to await one. Expressions that genuinely return a promise go
/// through here.
async function evaluateAsync(expression, timeoutMs = 30000) {
  const pending = send('Runtime.evaluate', {
    expression,
    returnByValue: true,
    awaitPromise: true,
  })
  const result = await Promise.race([
    pending,
    new Promise((_, reject) =>
      setTimeout(() => reject(new Error(`evaluateAsync timed out after ${timeoutMs}ms`)), timeoutMs),
    ),
  ])
  if (result.exceptionDetails) {
    throw new Error(`evaluateAsync threw: ${result.exceptionDetails.text}`)
  }
  return result.result.value
}

export { send, evaluate, evaluateAsync, clickByText, snapshot, consoleLines, socket }

/// Flush and exit. The CDP socket keeps node's event loop alive, so without this
/// the process hangs after doing its work and its buffered stdout never lands.
function finish(code = 0) {
  try {
    socket.close()
  } catch {
    // Already closed.
  }
  setTimeout(() => process.exit(code), 50)
}
export { finish }
