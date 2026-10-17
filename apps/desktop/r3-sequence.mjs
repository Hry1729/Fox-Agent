// Package-2-D closeout, R-3: the full user-visible loop on the real desktop.
//
// Every action is a real DOM click in the real WebView, so each one goes through
// the real frontend code, the real `invoke` IPC, the real registered Tauri
// command and the real database. Nothing here calls a Rust function directly and
// nothing mocks `invoke`. Durable facts are read back through the real
// `conversation_load` command, not from the DOM alone.

import { evaluate, evaluateAsync, clickByText, consoleLines, finish } from './cdp-drive.mjs'

const CONVERSATION_ID = process.argv[2]
if (!CONVERSATION_ID) {
  console.error('usage: node r3-sequence.mjs <conversationId>')
  process.exit(2)
}
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

function card() {
  return evaluate(`(() => {
    const card = document.querySelector('.fox-delivery-acceptance')
    if (!card) return { present: false }
    return {
      present: true,
      cardHeader: (card.querySelector('header small') || {}).textContent?.trim() || null,
      verification: (card.querySelector('dd[data-class]') || {}).textContent?.trim() || null,
      facts: [...card.querySelectorAll('.fox-delivery-acceptance-facts dd')].map((dd) => dd.textContent.trim()),
      blocked: (card.querySelector('.fox-delivery-acceptance-blocked') || {}).textContent?.trim() || null,
      closed: (card.querySelector('.fox-delivery-acceptance-closed') || {}).textContent?.trim() || null,
      buttons: [...card.querySelectorAll('button')].map((b) => (b.textContent || '').trim()),
      error: (card.querySelector('.fox-delivery-acceptance-error') || {}).textContent?.trim() || null,
      panelHeader: (document.querySelector('.fox-delivery-review header small') || {}).textContent?.trim() || null,
      panelTitle: (document.querySelector('.fox-delivery-review header strong') || {}).textContent?.trim() || null,
    }
  })()`)
}

async function durable(label) {
  const state = await evaluateAsync(`(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke
    const data = (await invoke('conversation_load', { request: { conversationId: ${JSON.stringify(CONVERSATION_ID)} } }))?.data ?? {}
    return {
      lastRunStatus: data.lastRun?.status ?? null,
      kernelState: data.kernelSnapshot?.state ?? null,
      terminalWritten: data.kernelSnapshot?.terminalWritten ?? null,
      checklist: (data.deliveryChecklist ?? []).map((i) => ({ itemKey: i.itemKey, status: i.status })),
    }
  })()`)
  console.log(`DURABLE ${label} ${JSON.stringify(state)}`)
  return state
}

// --- a fresh isolated database re-triggers onboarding; skip it for real ---
const needsSkip = await evaluate(
  `[...document.querySelectorAll('button')].some((b) => (b.textContent || '').includes('跳过设置'))`,
)
if (needsSkip) {
  await clickByText('button', '跳过设置')
  await wait(3000)
  console.log('ONBOARDING skipped')
}

// --- open the conversation through the real sidebar ---
const opened = await evaluate(`(() => {
  const toggle = document.querySelector('button[aria-label^="展开项目列表"]')
  if (!toggle) return 'no-toggle'
  const entry = toggle.closest('.fox-sidebar-collapsed-entry') || toggle.parentElement
  for (const type of ['mouseenter', 'mouseover', 'focusin']) {
    entry.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window }))
  }
  return 'hovered'
})()`)
await wait(900)
const clickedProject = await evaluate(`(() => {
  const entry = [...document.querySelectorAll('button[role="menuitem"]')]
    .find((node) => (node.getAttribute('title') || '').includes('g4-p2d-closeout-project'))
  if (!entry) return null
  entry.click()
  return entry.getAttribute('title')
})()`)
console.log('OPEN ' + JSON.stringify({ hovered: opened, project: clickedProject }))
await wait(4500)

// --- 1. the parked state ---
console.log('CARD parked ' + JSON.stringify(await card()))
await durable('parked')

// --- 2. accept through the real button ---
console.log('CLICK accept ' + JSON.stringify(await clickByText('button', '接受未核验风险')))
await wait(4500)
console.log('CARD after-accept ' + JSON.stringify(await card()))
const afterAccept = await durable('after-accept')

// --- 3. reload the page: the Run and its acceptance must still be findable ---
await evaluate(`location.reload()`)
await wait(7000)
const needsSkipAgain = await evaluate(
  `[...document.querySelectorAll('button')].some((b) => (b.textContent || '').includes('跳过设置'))`,
)
if (needsSkipAgain) {
  await clickByText('button', '跳过设置')
  await wait(3000)
  console.log('ONBOARDING skipped after reload')
}
const reopened = await evaluate(`(() => {
  const toggle = document.querySelector('button[aria-label^="展开项目列表"]')
  if (!toggle) return 'no-toggle'
  const entry = toggle.closest('.fox-sidebar-collapsed-entry') || toggle.parentElement
  for (const type of ['mouseenter', 'mouseover', 'focusin']) {
    entry.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window }))
  }
  return 'hovered'
})()`)
await wait(900)
const reopenedProject = await evaluate(`(() => {
  const entry = [...document.querySelectorAll('button[role="menuitem"]')]
    .find((node) => (node.getAttribute('title') || '').includes('g4-p2d-closeout-project'))
  if (!entry) return null
  entry.click()
  return entry.getAttribute('title')
})()`)
console.log('REOPEN ' + JSON.stringify({ hovered: reopened, project: reopenedProject }))
await wait(4500)
console.log('CARD after-reload ' + JSON.stringify(await card()))
await durable('after-reload')

// --- 4. revoke through the real button, on a Run that has already ended ---
console.log('CLICK revoke ' + JSON.stringify(await clickByText('button', '撤销接受')))
await wait(4500)
console.log('CARD after-revoke ' + JSON.stringify(await card()))
await durable('after-revoke')

console.log('CONSOLE ' + JSON.stringify(consoleLines.slice(0, 25)))
console.log('SUMMARY ' + JSON.stringify({
  kernelStateAfterAccept: afterAccept.kernelState,
  terminalWrittenAfterAccept: afterAccept.terminalWritten,
}))
finish(0)
