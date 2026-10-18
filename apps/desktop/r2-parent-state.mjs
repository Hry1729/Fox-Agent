// Package-2-D closeout §5: verify the PARENT Run is still parked on
// awaiting_delivery_review after the continuation, and that its acceptance row is
// unchanged — the父子隔离 evidence, read through the real commands.

import { evaluateAsync, finish } from './cdp-drive.mjs'

const PARENT_RUN_ID = process.argv[2]
const CONVERSATION_ID = process.argv[3]
if (!PARENT_RUN_ID || !CONVERSATION_ID) {
  console.error('usage: node r2-parent-state.mjs <parentRunId> <conversationId>')
  process.exit(2)
}

const result = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const out = {}
  // The parent's kernel snapshot, read through the conversation's own detail.
  const data = (await invoke('conversation_load', { request: { conversationId: ${JSON.stringify(CONVERSATION_ID)} } }))?.data ?? {}
  out.lastRunId = data.lastRun?.id ?? null
  out.childKernelState = data.kernelSnapshot?.state ?? null
  // The parent's acceptance state, and whether it is still continuable.
  out.parentAcceptance = (await invoke('delivery_acceptance_state', { request: { runId: ${JSON.stringify(PARENT_RUN_ID)}, itemKey: 'slot:xlsx:1' } }))?.data ?? null
  try {
    out.parentContinuable = (await invoke('kernel_run_continuable', { request: { conversationId: ${JSON.stringify(CONVERSATION_ID)}, sourceRunId: ${JSON.stringify(PARENT_RUN_ID)} } }))?.data ?? null
  } catch (error) { out.parentContinuable = 'ERROR ' + String(error) }
  // The parent's own checklist row is still pending and still unverified.
  out.parentChecklist = (data.deliveryChecklist ?? []).filter((i) => (i.runId ?? i.run_id) === ${JSON.stringify(PARENT_RUN_ID)})
  return out
})()`)
console.log('PARENT_STATE ' + JSON.stringify(result, null, 1))
finish(0)
