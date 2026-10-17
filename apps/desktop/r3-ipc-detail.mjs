// Package-2-D closeout, R-3: read the real conversation detail (including the
// kernel snapshot) through the real IPC.

import { evaluateAsync, finish } from './cdp-drive.mjs'

const conversationId = process.argv[2]
if (!conversationId) {
  console.error('usage: node r3-ipc-detail.mjs <conversationId>')
  process.exit(2)
}

const result = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  try {
    const response = await invoke('conversation_load', { request: { conversationId: ${JSON.stringify(conversationId)} } })
    const data = response?.data ?? {}
    return {
      lastRun: data.lastRun ?? null,
      kernelSnapshot: data.kernelSnapshot ?? null,
      deliveryChecklist: (data.deliveryChecklist ?? []).map((item) => ({
        itemKey: item.itemKey, status: item.status, runId: item.runId,
        targetPath: item.targetPath, description: item.description,
      })),
      deliveryChecklistTruncated: data.deliveryChecklistTruncated ?? null,
      messages: (data.messages ?? []).length,
    }
  } catch (error) {
    return { error: String(error) }
  }
})()`)
console.log('DETAIL ' + JSON.stringify(result, null, 1).slice(0, 6000))
finish(0)
