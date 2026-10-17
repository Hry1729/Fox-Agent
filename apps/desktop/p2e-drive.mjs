// Package-2-E acceptance: drive the REAL normal producer entry.
//
// This is the card's requirement 3 in executable form. It does not insert a
// contract, does not call `classify_v2_requirement`, and does not use a test
// factory: it creates a real conversation through `conversation_create` and
// starts a real task through `run_start`, which is the same command the UI's
// composer calls. The contract is frozen by the Host inside that call
// (`RuntimeHost::start_kernel_run` → `delivery::freeze_task_delivery` →
// `content_requirements::freeze` → `add`), and the verdict is produced by the
// real live loop driving the real configurable tool executor.
//
// Everything printed is read back through the real IPC, so the evidence is the
// product's own view of its own state.
//
// Usage:
//   node p2e-start.mjs <label>
//   node p2e-inspect.mjs <conversationId> [runId]

import { evaluateAsync, finish } from './cdp-drive.mjs'

const MODE = process.argv[2]
const ARG = process.argv[3]
const RUN_ARG = process.argv[4]

const script = MODE === 'start'
  ? `(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const out = { label: ${JSON.stringify(ARG)} }
  try {
    const created = await invoke('conversation_create', { request: {
      agentId: 'fox-general',
      title: 'P2E ' + ${JSON.stringify(ARG)},
      projectRoot: ${JSON.stringify(process.env.FOX_ACCEPTANCE_PROJECT_ROOT)},
      permissionMode: 'allow',
    } })
    out.createOk = created?.ok ?? null
    out.createError = created?.error ?? null
    out.conversationId = created?.data?.id ?? null
  } catch (error) { out.createThrew = String(error); return out }
  if (!out.conversationId) return out
  try {
    const started = await invoke('run_start', { request: {
      conversationId: out.conversationId,
      text: ${JSON.stringify(process.env.FOX_ACCEPTANCE_TASK)},
    } })
    out.startOk = started?.ok ?? null
    out.startError = started?.error ?? null
    out.runId = started?.data?.run?.id ?? null
  } catch (error) { out.startThrew = String(error) }
  return out
})()`
  : `(async () => {
  const invoke = window.__TAURI_INTERNALS__.invoke
  const conversationId = ${JSON.stringify(ARG)}
  const runIdArg = ${JSON.stringify(RUN_ARG ?? null)}
  try {
    const loaded = await invoke('conversation_load', { request: { conversationId } })
    const data = loaded?.data ?? {}
    const out = {
      loadOk: loaded?.ok ?? null,
      loadError: loaded?.error ?? null,
      lastRun: data.lastRun ? { id: data.lastRun.id, status: data.lastRun.status, terminalReason: data.lastRun.terminalReason ?? null } : null,
      kernelState: data.kernelSnapshot?.state ?? null,
      kernelTerminal: data.kernelSnapshot?.terminalReason ?? null,
      runs: (data.runs ?? []).map((run) => ({ id: run.id, status: run.status, terminalReason: run.terminalReason ?? null })),
      checklist: (data.deliveryChecklist ?? []).map((item) => ({
        itemKey: item.itemKey, status: item.status, targetPath: item.targetPath ?? null,
        finding: item.finding ?? null,
      })),
    }
    const runId = runIdArg || data.lastRun?.id
    out.checkedRunId = runId ?? null
    if (runId) {
      for (const item of (data.deliveryChecklist ?? [])) {
        const state = await invoke('delivery_acceptance_state', { request: { runId, itemKey: item.itemKey } })
        out.acceptanceState = out.acceptanceState ?? {}
        out.acceptanceState[item.itemKey] = { ok: state?.ok ?? null, error: state?.error ?? null, data: state?.data ?? null }
      }
    }
    return out
  } catch (error) { return { error: String(error) } }
})()`

const result = await evaluateAsync(script, 120000)
console.log((MODE === 'start' ? 'START ' : 'INSPECT ') + JSON.stringify(result, null, 1))
finish(0)
