// Package-2-E closeout U-1: drive the NORMAL producer entry.
//
// Creates a real conversation bound to an isolated project root, then starts a
// real Run whose task text makes the Host's own `freeze_concat` derive and freeze
// a `content:concat` contract. With `FOX_DELIVERY_V2=1` the producer writes the
// v2 envelope; the model (the isolated fixture provider) proposes a real
// `write_file` that clears the target, which is a real managed write with a real
// receipt — the checker then classifies it `unconfirmed`, and the Run parks.
//
// Everything goes through the real registered commands over the real IPC.

import { evaluateAsync, finish } from './cdp-drive.mjs'

const PROJECT_ROOT = process.argv[2] || 'E:/fox-targets/g2e-closeout/project-prefix'
const LABEL = process.argv[3] || 'prefix'

const TASK =
  '[P2E_EMPTY] 在根目录中选出 3 个 .txt 文件，按字节从小到大选择，' +
  '大小相同按文件名升序，以完整文件名升序合并，条目之间只用一个空行分隔，' +
  '合并为 joined.txt，不得修改任何输入文件'

const result = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const out = { task: ${JSON.stringify(TASK)} }
  try {
    out.created = await invoke('conversation_create', { request: {
      agentId: 'fox-general',
      title: '2-E 收口验收（隔离，${LABEL}）',
      projectRoot: ${JSON.stringify(PROJECT_ROOT)},
      permissionMode: 'allow',
    } })
  } catch (error) { out.created = 'ERROR ' + String(error) }
  const conversationId = out.created?.data?.id ?? out.created?.id
  out.conversationId = conversationId ?? null
  if (conversationId) {
    try {
      out.started = await invoke('run_start', { request: {
        conversationId, text: ${JSON.stringify(TASK)},
      } })
    } catch (error) { out.started = 'ERROR ' + String(error) }
    out.runId = out.started?.data?.run?.id ?? out.started?.data?.runId ?? null
  }
  return out
})()`)
console.log('ENTRY ' + JSON.stringify(result, null, 1))
finish(0)
