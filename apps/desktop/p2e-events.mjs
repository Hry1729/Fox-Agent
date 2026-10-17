// Package-2-E acceptance: read the real events/messages of one conversation, so a
// failed start can be explained from the product's own durable records rather than
// guessed at.

import { evaluateAsync, finish } from './cdp-drive.mjs'

const conversationId = process.argv[2]
if (!conversationId) {
  console.error('usage: node p2e-events.mjs <conversationId>')
  process.exit(2)
}

const result = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const loaded = await invoke('conversation_load', { request: { conversationId: ${JSON.stringify(conversationId)} } })
  const data = loaded?.data ?? {}
  return {
    ok: loaded?.ok ?? null, error: loaded?.error ?? null,
    messages: (data.messages ?? []).map((m) => ({
      role: m.role, status: m.status ?? null,
      text: String(m.content ?? '').slice(0, 400),
    })),
    events: (data.runtimeEvents ?? []).slice(-40).map((e) => ({
      type: e.eventType ?? null,
      event: JSON.stringify(e.event ?? {}).slice(0, 600),
    })),
    runs: (data.runs ?? []).map((r) => ({
      id: r.id, status: r.status, errorCode: r.errorCode ?? null, errorMessage: r.errorMessage ?? null,
    })),
    toolCalls: (data.toolCalls ?? []).map((c) => ({ tool: c.tool ?? c.name, status: c.status ?? null, error: c.error ?? null })),
  }
})()`, 120000)
console.log('EVENTS ' + JSON.stringify(result, null, 1))
finish(0)
