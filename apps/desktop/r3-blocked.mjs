// Package-2-D closeout, R-3: read the real blocked reason after a real on-disk
// drift of the candidate file, through the real state command and the real card.

import { evaluate, evaluateAsync, consoleLines, finish } from './cdp-drive.mjs'

const CONVERSATION_ID = process.argv[2]
if (!CONVERSATION_ID) {
  console.error('usage: node r3-blocked.mjs <conversationId>')
  process.exit(2)
}
const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

async function openConversation(label, index) {
  const needsSkip = await evaluate(
    `[...document.querySelectorAll('button')].some((b) => (b.textContent || '').includes('跳过设置'))`,
  )
  if (needsSkip) {
    const { clickByText } = await import('./cdp-drive.mjs')
    await clickByText('button', '跳过设置')
    await wait(3000)
    console.log(`${label}: onboarding skipped`)
  }
  await evaluate(`(() => {
    const toggle = document.querySelector('button[aria-label^="展开项目列表"]')
    if (!toggle) return 'no-toggle'
    const entry = toggle.closest('.fox-sidebar-collapsed-entry') || toggle.parentElement
    for (const type of ['mouseenter', 'mouseover', 'focusin']) {
      entry.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window }))
    }
    return 'hovered'
  })()`)
  await wait(900)
  await evaluate(`(() => {
    const projectEntry = [...document.querySelectorAll('.fox-sidebar-hover-project')][0]
    if (!projectEntry) return 'no-project'
    for (const type of ['mouseenter', 'mouseover', 'focusin']) {
      projectEntry.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window }))
    }
    return 'hovered'
  })()`)
  await wait(900)
  const opened = await evaluate(`(() => {
    const popover = document.querySelector('.fox-sidebar-conversations-popover')
    if (!popover) return { opened: null, reason: 'no-popover' }
    const entries = [...popover.querySelectorAll('button[role="menuitem"]')]
    const target = entries[${index}]
    if (!target) return { opened: null, reason: 'no-entry', count: entries.length }
    target.click()
    return { opened: (target.textContent || '').trim().slice(0, 60), count: entries.length }
  })()`)
  console.log(`${label}: open ${JSON.stringify(opened)}`)
  await wait(4500)
}

await openConversation('START', 1)

const card = await evaluate(`(() => {
  const node = document.querySelector('.fox-delivery-acceptance')
  if (!node) return { present: false }
  return {
    present: true,
    blocked: (node.querySelector('.fox-delivery-acceptance-blocked') || {}).textContent?.trim() || null,
    verification: (node.querySelector('dd[data-class]') || {}).textContent?.trim() || null,
    facts: [...node.querySelectorAll('.fox-delivery-acceptance-facts dd')].map((dd) => dd.textContent.trim()),
    buttons: [...node.querySelectorAll('button')].map((b) => (b.textContent || '').trim()),
  }
})()`)
console.log('CARD after-drift ' + JSON.stringify(card))

const state = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const data = (await invoke('conversation_load', { request: { conversationId: ${JSON.stringify(CONVERSATION_ID)} } }))?.data ?? {}
  const runId = data.lastRun?.id
  const itemKey = data.deliveryChecklist?.[0]?.itemKey
  const acceptance = (await invoke('delivery_acceptance_state', { request: { runId, itemKey } }))?.data ?? null
  return { blocked: acceptance?.blocked ?? null, kernelState: data.kernelSnapshot?.state ?? null }
})()`)
console.log('STATE after-drift ' + JSON.stringify(state))
console.log('CONSOLE ' + JSON.stringify(consoleLines.slice(0, 20)))
finish(0)
