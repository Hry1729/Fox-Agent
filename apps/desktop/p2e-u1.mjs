// Package-2-E acceptance, U1 leg: drive the REAL acceptance command.
//
// `delivery_acceptance_resolve` is the same command the U1 review card calls.
// Nothing here writes a database row: an accept that lands must have passed the
// product's own preconditions, and one that is refused must come back with the
// product's own `blocked.kind`.
//
// Usage: node p2e-u1.mjs <accept|reject|revoke> <runId> <itemKey>

import { evaluateAsync, finish } from './cdp-drive.mjs'

const DECISION = process.argv[2]
const RUN_ID = process.argv[3]
const ITEM_KEY = process.argv[4]

const script = `(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const out = { decision: ${JSON.stringify(DECISION)}, runId: ${JSON.stringify(RUN_ID)}, itemKey: ${JSON.stringify(ITEM_KEY)} }
  try {
    const resolved = await invoke('delivery_acceptance_resolve', { request: {
      runId: ${JSON.stringify(RUN_ID)},
      itemKey: ${JSON.stringify(ITEM_KEY)},
      decision: ${JSON.stringify(DECISION)},
    } })
    out.ok = resolved?.ok ?? null
    out.error = resolved?.error ?? null
    out.outcome = resolved?.data ?? null
  } catch (error) { out.threw = String(error) }
  try {
    const after = await invoke('delivery_acceptance_state', { request: {
      runId: ${JSON.stringify(RUN_ID)}, itemKey: ${JSON.stringify(ITEM_KEY)} } })
    out.after = after?.data ?? after?.error ?? null
  } catch (error) { out.afterThrew = String(error) }
  return out
})()`

const result = await evaluateAsync(script, 60000)
console.log('U1 ' + JSON.stringify(result, null, 1))
finish(0)
