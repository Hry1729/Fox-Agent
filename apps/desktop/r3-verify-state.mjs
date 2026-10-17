// Package-2-D closeout, R-3: read the real Run state and the real acceptance
// row through the real IPC, so the UI update can be tied to durable facts.

import { evaluateAsync, finish } from './cdp-drive.mjs'

const conversationId = process.argv[2]
if (!conversationId) {
  console.error('usage: node r3-verify-state.mjs <conversationId>')
  process.exit(2)
}

const result = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  try {
    const data = (await invoke('conversation_load', { request: { conversationId: ${JSON.stringify(conversationId)} } }))?.data ?? {}
    const out = {
      lastRunStatus: data.lastRun?.status ?? null,
      kernelState: data.kernelSnapshot?.state ?? null,
      terminalWritten: data.kernelSnapshot?.terminalWritten ?? null,
      checklist: (data.deliveryChecklist ?? []).map((item) => ({
        itemKey: item.itemKey, status: item.status, runId: item.runId,
      })),
    }
    // The item-detail surface's own state query, which is what the card reads.
    const runId = data.lastRun?.id
    const itemKey = data.deliveryChecklist?.[0]?.itemKey
    if (runId && itemKey) {
      const state = await invoke('delivery_acceptance_state', { request: { runId, itemKey } })
      out.acceptanceState = state?.data ?? state
    }
    return out
  } catch (error) {
    return { error: String(error) }
  }
})()`)
console.log('STATE ' + JSON.stringify(result, null, 1))
finish(0)
