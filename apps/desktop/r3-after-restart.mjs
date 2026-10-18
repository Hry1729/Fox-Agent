// Package-2-D closeout, R-3: after a REAL application restart, confirm both
// conversations, their Run states and their acceptance rows are still findable
// through the real commands — and that the card still renders.

import { evaluate, evaluateAsync, consoleLines, finish } from './cdp-drive.mjs'

const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms))

const needsSkip = await evaluate(
  `[...document.querySelectorAll('button')].some((b) => (b.textContent || '').includes('跳过设置'))`,
)
if (needsSkip) {
  const { clickByText } = await import('./cdp-drive.mjs')
  await clickByText('button', '跳过设置')
  await wait(3000)
  console.log('ONBOARDING skipped')
}

const list = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  return ((await invoke('conversations_list', {}))?.data ?? []).map((item) => ({
    id: item.id, lastRunId: item.lastRunId, lastRunStatus: item.lastRunStatus,
  }))
})()`)
console.log('AFTER_RESTART_LIST ' + JSON.stringify(list))

const details = []
for (const conversation of list) {
  const detail = await evaluateAsync(`(async () => {
    const invoke = window.__TAURI_INTERNALS__.invoke
    const data = (await invoke('conversation_load', { request: { conversationId: ${JSON.stringify(conversation.id)} } }))?.data ?? {}
    const runId = data.lastRun?.id
    const itemKey = data.deliveryChecklist?.[0]?.itemKey
    const acceptance = runId && itemKey
      ? ((await invoke('delivery_acceptance_state', { request: { runId, itemKey } }))?.data ?? null)
      : null
    return {
      kernelState: data.kernelSnapshot?.state ?? null,
      terminalWritten: data.kernelSnapshot?.terminalWritten ?? null,
      checklist: (data.deliveryChecklist ?? []).map((i) => ({ itemKey: i.itemKey, status: i.status })),
      accepted: acceptance?.accepted ? { by: acceptance.accepted.acceptedBy, at: acceptance.accepted.acceptedAt } : null,
      rejected: acceptance?.rejected ?? null,
      runTerminal: acceptance?.runTerminal ?? null,
      blockedKind: acceptance?.blocked?.kind ?? null,
    }
  })()`)
  console.log(`AFTER_RESTART_DETAIL ${conversation.id} ${JSON.stringify(detail)}`)
  details.push(detail)
}

console.log('CONSOLE ' + JSON.stringify(consoleLines.slice(0, 20)))
finish(0)
