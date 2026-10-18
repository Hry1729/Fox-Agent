// Package-2-D closeout, R-2: read the child Run's terminal reason, so the
// continuation mechanism can be reported honestly.

import { evaluateAsync, finish } from './cdp-drive.mjs'

const CONVERSATION_ID = process.argv[2]
if (!CONVERSATION_ID) {
  console.error('usage: node r2-child-reason.mjs <conversationId>')
  process.exit(2)
}

const result = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const data = (await invoke('conversation_load', { request: { conversationId: ${JSON.stringify(CONVERSATION_ID)} } }))?.data ?? {}
  return {
    lastRun: data.lastRun ? {
      id: data.lastRun.id, status: data.lastRun.status,
      errorCode: data.lastRun.errorCode, errorMessage: data.lastRun.errorMessage,
    } : null,
    runs: (data.runs ?? []).map((r) => ({ id: r.id, status: r.status, errorCode: r.errorCode, errorMessage: r.errorMessage })),
    messages: (data.messages ?? []).map((m) => ({ role: m.role, status: m.status, text: String(m.content ?? '').slice(0, 200) })),
  }
})()`)
console.log('CHILD_REASON ' + JSON.stringify(result, null, 1))
finish(0)
