// Package-2-D closeout, R-3: read the app's own conversation list through the
// real IPC, to see what the real command returns for the isolated database.

import { evaluateAsync, finish } from './cdp-drive.mjs'

const result = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const out = {}
  try {
    out.list = await invoke('conversations_list', {})
  } catch (error) {
    out.list = 'ERROR ' + String(error)
  }
  return out
})()`)
console.log('IPC ' + JSON.stringify(result, null, 1))
finish(0)
