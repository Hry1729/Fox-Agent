// Package-2-D closeout, R-2: drive the real `kernel_run_continue` entry against
// the completed Run, and check the parent/child relationship, the re-verified
// authorization and the single-active constraint.
//
// Every call goes through the real IPC to the real registered command, which
// calls `RuntimeHost::continue_kernel_run`. Nothing is mocked.

import { evaluateAsync, finish } from './cdp-drive.mjs'

const SOURCE_RUN_ID = process.argv[2]
const CONVERSATION_ID = process.argv[3]
if (!SOURCE_RUN_ID || !CONVERSATION_ID) {
  console.error('usage: node r2-continue.mjs <sourceRunId> <conversationId>')
  process.exit(2)
}

const result = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const out = {}
  const request = { conversationId: ${JSON.stringify(CONVERSATION_ID)}, sourceRunId: ${JSON.stringify(SOURCE_RUN_ID)} }
  try { out.continuable = await invoke('kernel_run_continuable', { request }) } catch (e) { out.continuable = 'ERROR ' + String(e) }
  try { out.list = await invoke('kernel_run_continuable_list', { conversationId: ${JSON.stringify(CONVERSATION_ID)} }) } catch (e) { out.list = 'ERROR ' + String(e) }
  try { out.continued = await invoke('kernel_run_continue', { request }) } catch (e) { out.continued = 'ERROR ' + String(e) }
  return out
})()`)
console.log('CONTINUE ' + JSON.stringify(result, null, 1))
finish(0)
