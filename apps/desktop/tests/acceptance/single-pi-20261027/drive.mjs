// Drive the REAL desktop app through its own WebView over CDP.
//
// Why CDP: the app's WebView2 drops posted window messages (PostMessage), so a
// background click/type looks successful but never reaches the page. With
// --remote-debugging-port the same window exposes a devtools target; we set the
// textarea value through the prototype setter (so React sees a real input
// event), read it back, then click the composer's own send button. Nothing is
// force-enabled and no submission path is bypassed.
//
// The send control is located by its own affordance, not by position: the
// composer's `button.fox-send-button` carries `aria-label="Submit"` exactly
// while it is the send control (`aria-label="Stop"` while a Run is in flight).
// A resend banner's 发送/取消 buttons, the steering lane and the Stop button are
// therefore excluded by construction, and the click asserts the control is
// enabled *before* it clicks.
//
// Success is a new Run, not a click: the driver reports `sent`, `runStarted`
// and `completed` from the page's own state (the composer leaves Submit, the
// Host's 运行中 badge appears, and a new assistant turn is rendered), plus the
// page errors it saw. The driver cannot read the database, so the operator
// asserts the row (`runs`, `runtime_sessions`) separately.
//
// Usage (app must be running with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS set):
//   node drive.mjs --conv "<conversation title fragment>" --ask "<question>"
//   node drive.mjs --conv "<fragment>" --ask "<question>" --wait 30000 --json
//
// Exit codes: 0 sent and a Run started, 1 the send never became a Run,
// 2 no CDP page, 3 sent and started but the Run did not settle within --wait.
const args = process.argv.slice(2)
const value = (name, fallback) => { const i = args.indexOf(name); return i >= 0 ? args[i + 1] : fallback }
const conv = value('--conv', null)
const ask = value('--ask', null)
const waitMs = Number(value('--wait', '26000'))
const port = Number(value('--port', '9222'))
const json = args.includes('--json')

const report = (code, payload) => {
  const result = { ok: code === 0, exitCode: code, ...payload }
  if (json) console.log(JSON.stringify(result, null, 1))
  else console.log(result.summary ?? JSON.stringify(result))
  process.exit(code)
}

const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

const targets = await (await fetch(`http://127.0.0.1:${port}/json`)).json().catch(() => null)
if (!targets) {
  console.error(`no devtools endpoint on ${port}; start the app with WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS=--remote-debugging-port=${port}`)
  process.exit(2)
}
const page = targets.find((target) => target.type === 'page' && target.url.includes('1421'))
if (!page) { console.error('no app page among devtools targets'); process.exit(2) }

const socket = new WebSocket(page.webSocketDebuggerUrl)
const pending = new Map()
let nextId = 0
const pageErrors = []
socket.addEventListener('message', (event) => {
  const message = JSON.parse(event.data)
  if (message.id && pending.has(message.id)) { pending.get(message.id)(message); pending.delete(message.id) }
  else if (message.method === 'Runtime.exceptionThrown') pageErrors.push(message.params.exceptionDetails?.exception?.description ?? message.params.exceptionDetails?.text)
})
await new Promise((resolve, reject) => { socket.addEventListener('open', resolve); socket.addEventListener('error', reject) })
const send = (method, params = {}) => new Promise((resolve) => { const id = ++nextId; pending.set(id, resolve); socket.send(JSON.stringify({ id, method, params })) })
const evaluate = async (expression) => {
  const response = await send('Runtime.evaluate', { expression, awaitPromise: true, returnByValue: true })
  if (response.result?.exceptionDetails) return { __error: response.result.exceptionDetails.exception?.description ?? 'evaluate failed' }
  return response.result?.result?.value
}
// Poll a page predicate in Node so a slow React commit cannot be mistaken for
// a failure, and the last observed state is what gets reported.
const waitFor = async (expression, timeoutMs, intervalMs = 250) => {
  const deadline = Date.now() + timeoutMs
  let last = await evaluate(expression)
  while (Date.now() < deadline) {
    if (last?.ok) return last
    await sleep(intervalMs)
    last = await evaluate(expression)
  }
  return last ?? { ok: false, reason: 'no result' }
}

// Every reading the driver takes comes from these page-owned facts.
const PROBE = `(() => {
  const visible = (el) => !!el && el.offsetParent !== null
  const textarea = [...document.querySelectorAll('textarea')].filter(visible).pop() ?? null
  const send = [...document.querySelectorAll('button.fox-send-button')].filter(visible).pop() ?? null
  const assistants = [...document.querySelectorAll('.fox-thread-msg.is-assistant')]
  const users = [...document.querySelectorAll('.fox-thread-msg.is-user')]
  return {
    ok: true,
    hasTextarea: !!textarea,
    inputValue: textarea ? textarea.value : null,
    send: send ? {
      ariaLabel: send.getAttribute('aria-label'),
      disabled: send.disabled,
      type: send.getAttribute('type'),
      visible: true,
    } : null,
    assistantTurns: assistants.length,
    userTurns: users.length,
    lastAssistantText: assistants.length ? (assistants[assistants.length - 1].innerText || '').slice(-1200) : null,
    runningBadge: !!document.querySelector('.fox-running-badge'),
    chatTitle: document.querySelector('.fox-chat-title')?.textContent ?? null,
  }
})()`

await send('Runtime.enable')

if (conv) {
  const opened = await evaluate(`(() => {
    const scope = document.querySelector('.fox-sidebar, aside, nav') ?? document
    const target = [...scope.querySelectorAll('button,a,li')]
      .find((el) => (el.textContent || '').includes(${JSON.stringify(conv)}))
    if (!target) return { ok: false }
    target.click()
    return { ok: true, text: (target.textContent || '').slice(0, 60) }
  })()`)
  if (!opened?.ok) report(1, { stage: 'open-conversation', conversation: conv, pageErrors })
  const loaded = await waitFor(`(() => ({ ok: (${PROBE}).chatTitle?.includes(${JSON.stringify(conv)}) }))()`, 15000)
  if (!loaded?.ok) report(1, { stage: 'open-conversation', conversation: conv, probe: await evaluate(PROBE), pageErrors })
}

const before = await evaluate(PROBE)
if (!before?.hasTextarea) report(1, { stage: 'composer', reason: 'no visible textarea', probe: before, pageErrors })

const typed = await evaluate(`(() => {
  const visible = (el) => !!el && el.offsetParent !== null
  const el = [...document.querySelectorAll('textarea')].filter(visible).pop()
  if (!el) return { ok: false, reason: 'no visible textarea' }
  const set = Object.getOwnPropertyDescriptor(window.HTMLTextAreaElement.prototype, 'value').set
  el.focus(); set.call(el, ${JSON.stringify(ask ?? '')}); el.dispatchEvent(new Event('input', { bubbles: true }))
  return { ok: true, value: el.value }
})()`)
if (!typed?.ok) report(1, { stage: 'type', probe: typed, pageErrors })
if (typed.value !== (ask ?? '')) report(1, { stage: 'type', reason: 'the written text did not survive read-back', written: ask ?? '', value: typed.value, pageErrors })

// The click only happens on an enabled send control: a disabled or absent one
// is a real state, and the driver reports it instead of guessing a neighbour.
const armed = await waitFor(`(() => {
  const probe = ${PROBE}
  return { ok: probe.send?.ariaLabel === 'Submit' && probe.send.disabled === false, probe }
})()`, 10000)
if (!armed?.ok) report(1, { stage: 'send-control', reason: 'no enabled send control in the composer', probe: armed?.probe ?? null, pageErrors })

const clicked = await evaluate(`(() => {
  const visible = (el) => !!el && el.offsetParent !== null
  const send = [...document.querySelectorAll('button.fox-send-button')].filter(visible).pop()
  if (!send) return { ok: false, reason: 'no send control' }
  if (send.getAttribute('aria-label') !== 'Submit') return { ok: false, reason: 'the send control is not the send affordance', ariaLabel: send.getAttribute('aria-label') }
  if (send.disabled) return { ok: false, reason: 'the send control is disabled' }
  send.click()
  return { ok: true, ariaLabel: send.getAttribute('aria-label'), type: send.getAttribute('type') }
})()`)
if (!clicked?.ok) report(1, { stage: 'click', probe: clicked, pageErrors })

// The Host clears the draft when it accepts the send; a retained draft is the
// "send did not start" path, not a success.
const cleared = await waitFor(`(() => { const probe = ${PROBE}; return { ok: probe.inputValue === '', probe } })()`, 5000)
if (!cleared?.ok) report(1, { stage: 'cleared', reason: 'the textarea was not cleared after the click', probe: cleared?.probe ?? null, pageErrors })

// A Run started when the page shows the Run state: the composer leaves Submit
// (its control becomes Stop) or the Host's 运行中 badge appears. A new
// assistant turn that appears without either is a render-only change.
const started = await waitFor(`(() => {
  const probe = ${PROBE}
  const byComposer = probe.send?.ariaLabel === 'Stop'
  const byBadge = probe.runningBadge
  const byAssistant = probe.assistantTurns > ${before.assistantTurns}
  return { ok: byComposer || byBadge || byAssistant, byComposer, byBadge, byAssistant, probe }
})()`, Math.max(10000, Math.min(30000, waitMs)))
const runStarted = Boolean(started?.ok)
if (!runStarted) report(1, {
  stage: 'run-started',
  reason: 'the page never showed a Run for this send',
  sent: true,
  runStarted: false,
  probe: started?.probe ?? null,
  pageErrors,
})

const settled = await waitFor(`(() => {
  const probe = ${PROBE}
  return {
    ok: probe.send?.ariaLabel === 'Submit' && probe.runningBadge === false && probe.assistantTurns > ${before.assistantTurns},
    probe,
  }
})()`, waitMs)
const finalProbe = settled?.probe ?? (await evaluate(PROBE))
const completed = Boolean(settled?.ok)
const payload = {
  conversation: conv,
  asked: ask,
  sent: true,
  runStarted: true,
  completed,
  startedBy: { composerStop: started?.byComposer ?? false, runningBadge: started?.byBadge ?? false, assistantTurn: started?.byAssistant ?? false },
  clicked,
  cleared: cleared?.probe?.inputValue === '',
  before: { assistantTurns: before.assistantTurns, userTurns: before.userTurns, inputValue: before.inputValue },
  after: { assistantTurns: finalProbe?.assistantTurns ?? null, userTurns: finalProbe?.userTurns ?? null, inputValue: finalProbe?.inputValue ?? null, runningBadge: finalProbe?.runningBadge ?? null, send: finalProbe?.send ?? null },
  assistantText: finalProbe?.lastAssistantText ?? null,
  pageErrors,
  summary: completed
    ? `sent=true runStarted=true completed=true assistantTurns ${before.assistantTurns}->${finalProbe?.assistantTurns}`
    : `sent=true runStarted=true completed=false (Run still in flight or failed; see after/assistantText)`,
}
socket.close()
report(completed ? 0 : 3, payload)
