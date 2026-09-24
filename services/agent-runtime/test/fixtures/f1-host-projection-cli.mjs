// Test bridge: take a real Host read envelope from Rust and apply the same
// Legacy Pi adapter or Kernel worker projection used in production.
import { readFileSync } from 'node:fs'
import { normalizeFoxToolResultForPi } from '../../src/tool-adapter.mjs'
import { modelToolResultContent } from '../../src/tool-view.mjs'

// Default test discovery may load fixture files. Only the Rust bridge opts in
// to stdin consumption; a discovered fixture exits immediately.
if (process.argv[2] !== '--f1-bridge') process.exit(0)
const input = JSON.parse(readFileSync(0, 'utf8'))
const result = input.result
const view = input.path === 'legacy'
  ? normalizeFoxToolResultForPi('read', result, {
      runId: input.runId,
      toolCallId: input.toolCallId,
      storage: input.storage,
    })
  : {
      content: modelToolResultContent('read', {
        ...result,
        resultRef: input.resultRef,
        storage: input.storage,
      }),
    }
process.stdout.write(JSON.stringify(view))
