// Package-2-E closeout U-1: after a REAL stop/restart with the switch closed,
// read every conversation through the real `conversations_list` +
// `conversation_load` commands and report the frozen v2 records and the parked
// Runs, so the preservation claim rests on the real read path.

import { evaluateAsync, finish } from './cdp-drive.mjs'

const result = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const out = { conversations: [] }
  const list = (await invoke('conversations_list', {}))?.data ?? []
  for (const conversation of list) {
    const data = (await invoke('conversation_load', { request: { conversationId: conversation.id } }))?.data ?? {}
    const runs = (data.runs ?? []).map((r) => ({ id: r.id, status: r.status }))
    const parked = runs.filter((r) => r.status === 'interrupted')
    const checklist = (data.deliveryChecklist ?? []).map((item) => {
      let requirements = null
      let policyPresent = null
      let schemaVersion = null
      try {
        const raw = item.requirementsJson ?? item.requirements_json ?? null
        if (typeof raw === 'string') {
          policyPresent = raw.includes('"policy"')
          const m = raw.match(/"schemaVersion":(\\d+)/)
          schemaVersion = m ? Number(m[1]) : null
          requirements = raw
        }
      } catch { /* leave null */ }
      return {
        itemKey: item.itemKey, status: item.status, runId: item.runId,
        targetPath: item.targetPath, schemaVersion, policyPresent,
        requirements,
      }
    })
    out.conversations.push({
      id: conversation.id, title: conversation.title,
      lastRunId: data.lastRun?.id ?? null,
      kernelState: data.kernelSnapshot?.state ?? null,
      runs, parked, checklist,
    })
  }
  return out
})()`)
console.log('PRESERVE ' + JSON.stringify(result, null, 1))
finish(0)
