// Package-2-D closeout, R-2: verify the continuation's parent/child linkage, the
// single-active constraint, and that the child Run really executes against the
// isolated local model service.

import { evaluateAsync, finish } from './cdp-drive.mjs'

const PARENT_RUN_ID = process.argv[2]
const CHILD_RUN_ID = process.argv[3]
const CONVERSATION_ID = process.argv[4]

const result = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const out = {}
  const request = { conversationId: ${JSON.stringify(CONVERSATION_ID)}, sourceRunId: ${JSON.stringify(PARENT_RUN_ID)} }

  // The single-active constraint: continuing an already-continued parent must fail.
  try { out.secondContinue = await invoke('kernel_run_continue', { request }) } catch (e) { out.secondContinue = 'ERROR ' + String(e) }

  // The child Run's own state and checklist.
  try {
    const data = (await invoke('conversation_load', { request: { conversationId: ${JSON.stringify(CONVERSATION_ID)} } }))?.data ?? {}
    out.lastRun = { id: data.lastRun?.id, status: data.lastRun?.status }
    out.kernelState = data.kernelSnapshot?.state ?? null
    out.checklist = (data.deliveryChecklist ?? []).map((i) => ({ itemKey: i.itemKey, runId: i.runId, status: i.status }))
    out.runs = (data.runs ?? []).map((r) => ({ id: r.id, status: r.status }))
  } catch (e) { out.conversation = 'ERROR ' + String(e) }

  // The child's acceptance state: an acceptance must NOT be inherited.
  try {
    const state = (await invoke('delivery_acceptance_state', { request: { runId: ${JSON.stringify(CHILD_RUN_ID)}, itemKey: 'slot:xlsx:1' } }))?.data ?? null
    out.childAcceptance = state ? {
      accepted: state.accepted ?? null, rejected: state.rejected ?? null,
      blockedKind: state.blocked?.kind ?? null, runTerminal: state.runTerminal ?? null,
    } : null
  } catch (e) { out.childAcceptance = 'ERROR ' + String(e) }

  // The parent is still terminal and still carries its own acceptance.
  try {
    const state = (await invoke('delivery_acceptance_state', { request: { runId: ${JSON.stringify(PARENT_RUN_ID)}, itemKey: 'slot:xlsx:1' } }))?.data ?? null
    out.parentAcceptance = state ? {
      accepted: state.accepted ? { by: state.accepted.acceptedBy } : null,
      rejected: state.rejected ?? null, blockedKind: state.blocked?.kind ?? null,
      runTerminal: state.runTerminal ?? null,
    } : null
  } catch (e) { out.parentAcceptance = 'ERROR ' + String(e) }

  return out
})()`)
console.log('VERIFY ' + JSON.stringify(result, null, 1))
finish(0)
