// Package-2-E closeout U-1: the real U1 accept on the real WebView.
//
// Opens the conversation through the real sidebar, reads the real card, clicks
// the real "接受未核验风险" button, and reads the durable Run state back through
// the real `conversation_load` command. Nothing is mocked and no Rust function is
// called directly.

import { evaluate, evaluateAsync, clickByText, consoleLines, finish } from './cdp-drive.mjs'

const CONVERSATION_ID = process.argv[2]
if (!CONVERSATION_ID) {
  console.error('usage: node p2e-u1-accept.mjs <conversationId>')
  process.exit(2)
}
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

function card() {
  return evaluate(`(() => {
    const node = document.querySelector('.fox-delivery-acceptance')
    if (!node) return { present: false }
    return {
      present: true,
      cardHeader: (node.querySelector('header small') || {}).textContent?.trim() || null,
      verification: (node.querySelector('dd[data-class]') || {}).textContent?.trim() || null,
      facts: [...node.querySelectorAll('.fox-delivery-acceptance-facts dd')].map((dd) => dd.textContent.trim()),
      blocked: (node.querySelector('.fox-delivery-acceptance-blocked') || {}).textContent?.trim() || null,
      closed: (node.querySelector('.fox-delivery-acceptance-closed') || {}).textContent?.trim() || null,
      buttons: [...node.querySelectorAll('button')].map((b) => (b.textContent || '').trim()),
      panelTitle: (document.querySelector('.fox-delivery-review header strong') || {}).textContent?.trim() || null,
    }
  })()`)
}

async function durable(label) {
  const state = await evaluateAsync(`(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke
    const data = (await invoke('conversation_load', { request: { conversationId: ${JSON.stringify(CONVERSATION_ID)} } }))?.data ?? {}
    const runId = data.lastRun?.id
    const itemKey = data.deliveryChecklist?.[0]?.itemKey
    let acceptance = null
    if (runId && itemKey) {
      acceptance = (await invoke('delivery_acceptance_state', { request: { runId, itemKey } }))?.data ?? null
    }
    return {
      lastRunStatus: data.lastRun?.status ?? null,
      kernelState: data.kernelSnapshot?.state ?? null,
      terminalWritten: data.kernelSnapshot?.terminalWritten ?? null,
      checklist: (data.deliveryChecklist ?? []).map((i) => ({ itemKey: i.itemKey, status: i.status })),
      acceptance: acceptance ? {
        verificationClass: acceptance.verificationClass,
        blockedKind: acceptance.blocked?.kind ?? null,
        acceptedBy: acceptance.accepted?.acceptedBy ?? null,
        rejected: acceptance.rejected ?? null,
        runTerminal: acceptance.runTerminal ?? null,
      } : null,
    }
  })()`)
  console.log(`DURABLE ${label} ${JSON.stringify(state)}`)
  return state
}

// --- open the conversation through the real sidebar ---
const needsSkip = await evaluate(
  `[...document.querySelectorAll('button')].some((b) => (b.textContent || '').includes('跳过设置'))`,
)
if (needsSkip) {
  await clickByText('button', '跳过设置')
  await wait(3000)
  console.log('ONBOARDING skipped')
}

const PROJECT_MARK = process.argv[3] || ''

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
  const mark = ${JSON.stringify(PROJECT_MARK)}
  const projects = [...document.querySelectorAll('.fox-sidebar-hover-project')]
  const projectEntry = mark
    ? projects.find((node) => (node.getAttribute('title') || '').includes(mark)) ?? projects[0]
    : projects[0]
  if (!projectEntry) return null
  for (const type of ['mouseenter', 'mouseover', 'focusin']) {
    projectEntry.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window }))
  }
  return projectEntry.getAttribute('title') || 'hovered-project'
})()`)
await wait(900)
const openedConversation = await evaluate(`(() => {
  const popover = document.querySelector('.fox-sidebar-conversations-popover')
  if (!popover) return null
  const entries = [...popover.querySelectorAll('button[role="menuitem"]')]
  // The popover is newest-first, so the conversation this run just created is the
  // first entry.
  const target = entries[0]
  if (!target) return null
  target.click()
  return (target.textContent || '').trim().slice(0, 60)
})()`)
console.log('OPEN ' + JSON.stringify({ hovered: opened, project: clickedProject, conversation: openedConversation }))
await wait(5000)

console.log('CARD before ' + JSON.stringify(await card()))
await durable('before')

console.log('CLICK accept ' + JSON.stringify(await clickByText('button', '接受未核验风险')))
await wait(5000)

console.log('CARD after ' + JSON.stringify(await card()))
const after = await durable('after')
console.log('CONSOLE ' + JSON.stringify(consoleLines.slice(0, 25)))
finish(0)
