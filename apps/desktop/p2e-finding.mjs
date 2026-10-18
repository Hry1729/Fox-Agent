// Package-2-E closeout U-1: read one conversation's checklist row finding through
// the real command, so the drift reason can be read precisely.
import { evaluateAsync, finish } from './cdp-drive.mjs'

const CONVERSATION_ID = process.argv[2]
if (!CONVERSATION_ID) {
  console.error('usage: node p2e-finding.mjs <conversationId>')
  process.exit(2)
}

const result = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const data = (await invoke('conversation_load', { request: { conversationId: ${JSON.stringify(CONVERSATION_ID)} } }))?.data ?? {}
  const item = (data.deliveryChecklist ?? [])[0] ?? {}
  let finding = null
  const raw = item.findingJson ?? item.finding_json ?? null
  if (typeof raw === 'string') {
    try { finding = JSON.parse(raw) } catch { finding = raw }
  }
  const runId = data.lastRun?.id
  const itemKey = item.itemKey
  let acceptance = null
  if (runId && itemKey) {
    acceptance = (await invoke('delivery_acceptance_state', { request: { runId, itemKey } }))?.data ?? null
  }
  return {
    targetPath: item.targetPath ?? null,
    finding,
    acceptance: acceptance ? { blocked: acceptance.blocked ?? null, verificationClass: acceptance.verificationClass } : null,
  }
})()`)
console.log('FINDING ' + JSON.stringify(result, null, 1))
finish(0)
