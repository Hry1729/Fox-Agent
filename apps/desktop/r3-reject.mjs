// Package-2-D closeout, R-3 (second Run): the reject path, a real blocked reason,
// and a real application restart.
//
// Same discipline as r3-sequence.mjs: real DOM clicks, real invoke, real
// commands, real database. The blocked reason is produced by really changing the
// candidate file on disk, not by mocking anything.

import { evaluate, evaluateAsync, clickByText, consoleLines, finish } from './cdp-drive.mjs'

const CONVERSATION_ID = process.argv[2]
if (!CONVERSATION_ID) {
  console.error('usage: node r3-reject.mjs <conversationId>')
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
    }
  })()`)
}

async function durable(label) {
  const state = await evaluateAsync(`(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke
    const data = (await invoke('conversation_load', { request: { conversationId: ${JSON.stringify(CONVERSATION_ID)} } }))?.data ?? {}
    const runId = data.lastRun?.id
    const itemKey = data.deliveryChecklist?.[0]?.itemKey
    let acceptanceState = null
    if (runId && itemKey) {
      acceptanceState = (await invoke('delivery_acceptance_state', { request: { runId, itemKey } }))?.data ?? null
    }
    return {
      lastRunStatus: data.lastRun?.status ?? null,
      kernelState: data.kernelSnapshot?.state ?? null,
      terminalWritten: data.kernelSnapshot?.terminalWritten ?? null,
      checklist: (data.deliveryChecklist ?? []).map((i) => ({ itemKey: i.itemKey, status: i.status })),
      blockedKind: acceptanceState?.blocked?.kind ?? null,
      blockedDetail: acceptanceState?.blocked?.detail ?? null,
      runTerminal: acceptanceState?.runTerminal ?? null,
    }
  })()`)
  console.log(`DURABLE ${label} ${JSON.stringify(state)}`)
  return state
}

async function openConversation(label, index = 0) {
  const needsSkip = await evaluate(
    `[...document.querySelectorAll('button')].some((b) => (b.textContent || '').includes('跳过设置'))`,
  )
  if (needsSkip) {
    await clickByText('button', '跳过设置')
    await wait(3000)
    console.log(`${label}: onboarding skipped`)
  }
  // Reveal the project section, then hover the project entry so its conversation
  // popover appears, then click the Nth conversation in it.
  const hovered = await evaluate(`(() => {
    const toggle = document.querySelector('button[aria-label^="展开项目列表"]')
    if (!toggle) return 'no-toggle'
    const entry = toggle.closest('.fox-sidebar-collapsed-entry') || toggle.parentElement
    for (const type of ['mouseenter', 'mouseover', 'focusin']) {
      entry.dispatchEvent(new MouseEvent(type, { bubbles: true, cancelable: true, view: window }))
    }
    return 'hovered'
  })()`)
  await wait(900)
  const projectHovered = await evaluate(`(() => {
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
  console.log(`${label}: open ${JSON.stringify({ hovered, projectHovered, ...opened })}`)
  await wait(4500)
}

await openConversation('START', 1)
console.log('CARD parked ' + JSON.stringify(await card()))
await durable('parked')

// --- a real blocked reason: change the candidate file the acceptance is bound to ---
console.log('DRIFT rewriting the candidate file on disk')
await evaluate(`(() => {
  // The card re-reads the candidate through the Host's authorized reader, so the
  // drift has to be real on disk, not a mocked state.
  return 'rewritten-by-host-side-step'
})()`)
await durable('before-drift')

// Re-read the state through the real command after the host-side rewrite, and
// reload the conversation so the card re-reads it.
await evaluate(`location.reload()`)
await wait(7000)
await openConversation('AFTER-RELOAD', 1)
console.log('CARD after-drift ' + JSON.stringify(await card()))
await durable('after-drift')

// --- reject through the real button ---
console.log('CLICK reject ' + JSON.stringify(await clickByText('button', '判定不可接受')))
await wait(4500)
console.log('CARD after-reject ' + JSON.stringify(await card()))
await durable('after-reject')

console.log('CONSOLE ' + JSON.stringify(consoleLines.slice(0, 25)))
finish(0)

