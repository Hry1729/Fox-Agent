// Package-2-D closeout §5: configure the isolated local model service through
// the REAL `model_service_save` command, invoked from the real WebView.
//
// The service points at this package's own loopback fixture provider on
// 127.0.0.1:1433, which never contacts a remote model. Nothing is mocked: this
// is the real command writing the real row, and the real provider answers the
// real HTTP requests.
//
// §5 change: the context window is sized for the isolated fixture. The measured
// budget for a continuation request is system 8,808 + tools 12,041 + history 403
// + append 1,366 + reserve 2,048 + overhead 2,048 tokens, plus the uncalibrated
// 50% safety margin on the estimated components (11,309) — about 38k tokens.
// A 32k window therefore reports `within_budget: false` and the Run stops with
// `kernel.context_compaction_no_candidates` before it ever reaches the model,
// even though the history is only 403 tokens. 200k leaves the same fixture with
// ample head-room. This is a TEST CONFIGURATION for the isolated service only:
// no production compaction policy, threshold or margin is touched.

import { evaluateAsync, finish } from './cdp-drive.mjs'

const WINDOW = process.env.FOX_CLOSEOUT_CONTEXT_WINDOW || '200000'

const result = await evaluateAsync(`(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const out = {}
  try {
    out.saved = await invoke('model_service_save', { request: {
      name: '2-D 收口隔离服务',
      baseUrl: 'http://127.0.0.1:1433/v1',
      modelId: 'fox-kernel-acceptance',
      apiType: 'openai-completions',
      contextWindow: Number(${JSON.stringify(WINDOW)}),
      maxOutputTokens: 2048,
      clearApiKey: true,
    } })
  } catch (error) {
    out.saved = 'ERROR ' + String(error)
  }
  return out
})()`)
console.log('MODEL_SERVICE ' + JSON.stringify(result, null, 1))
finish(0)

