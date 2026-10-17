// Package-2-D closeout, R-3: list the real conversations and report the newest
// one, so the driver can bind its evidence to a specific run.

import { evaluateAsync, finish } from './cdp-drive.mjs'

const result = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const list = (await invoke('conversations_list', {}))?.data ?? []
  return list.map((item) => ({
    id: item.id, title: item.title, lastRunId: item.lastRunId, lastRunStatus: item.lastRunStatus,
    createdAt: item.createdAt,
  }))
})()`)
console.log('CONVERSATIONS ' + JSON.stringify(result, null, 1))
finish(0)
