import { validateWireValue } from '../../../packages/fox-engine-protocol/index.mjs'
import { nativeToolInputDigest, nativeHostToolResult, NATIVE_RESPONSE_DIGEST, NATIVE_NONEXECUTION } from './pi-native-tool-bridge.mjs'

export const NATIVE_RUN_ENTRY = 'fox.runtime-run'
export const TOOL_CALL_FACTS = 'tool_call.facts'

function recoveryError(code, message) {
  const error = new Error(message)
  error.code = code
  return error
}

export function markNativeSessionRun(manager, { runId, contextGeneration, executorRoute }) {
  manager.appendCustomEntry(NATIVE_RUN_ENTRY, {
    schemaVersion: 1, runId, contextGeneration, executorRoute: executorRoute ?? null,
  })
}

export function nativeSessionToolCalls(manager, contextGeneration) {
  const branch = manager.getBranch()
  const calls = []
  const byId = new Map()
  let scope = null
  for (const entry of branch) {
    if (entry.type === 'custom' && entry.customType === NATIVE_RUN_ENTRY) {
      scope = entry.data
      if (scope?.schemaVersion !== 1 || !scope.runId || !Number.isSafeInteger(scope.contextGeneration)) {
        throw recoveryError('session.native_recovery_inconsistent', 'The Pi session has an invalid Fox Run boundary.')
      }
      if (scope.executorRoute === 'pi_native' && scope.contextGeneration !== contextGeneration) {
        throw recoveryError('session.native_generation_mismatch', 'The Pi session contains a managed Run from another context generation.')
      }
      continue
    }
    if (scope?.executorRoute !== 'pi_native' || entry.type !== 'message') continue
    const message = entry.message
    if (message?.role === 'assistant') {
      for (const call of message.content ?? []) {
        if (call?.type !== 'toolCall') continue
        if (!call.id || !call.name || byId.has(call.id)) {
          throw recoveryError('session.native_recovery_inconsistent', 'The Pi session has an invalid or repeated managed tool call ID.')
        }
        const record = { callId: call.id, tool: call.name, inputDigest: nativeToolInputDigest(call.arguments ?? {}),
          hasResult: false, entryId: entry.id, runId: scope.runId }
        calls.push(record)
        byId.set(call.id, record)
      }
    } else if (message?.role === 'toolResult') {
      const call = byId.get(message.toolCallId)
      if (!call || call.hasResult || (message.toolName && message.toolName !== call.tool)) {
        throw recoveryError('session.native_recovery_inconsistent', 'A managed Pi tool result has no matching, unique tool call.')
      }
      call.hasResult = true
      call.result = message
    }
  }
  return { branch, calls }
}

function validatePayload(schema, payload) {
  const errors = validateWireValue(schema, payload)
  if (errors.length) throw recoveryError('session.native_recovery_protocol', `Invalid ${schema}: ${errors.join('; ')}`)
}

function recoveredResult(call, fact) {
  const result = nativeHostToolResult(call.tool, fact.response, { runId: call.runId, toolCallId: call.callId })
  return { ...result, role: 'toolResult', toolCallId: call.callId, toolName: call.tool,
    isError: result.isError === true || fact.response?.isError === true, timestamp: Date.now() }
}

function resultMatches(call, fact, expected) {
  if (!call.hasResult) return false
  const local = call.result
  const nonexecution = local?.details?.[NATIVE_NONEXECUTION]
  if (fact.state === 'not_executed' && nonexecution?.schemaVersion === 1
    && nonexecution.tool === call.tool && nonexecution.inputDigest === call.inputDigest
    && ['unknown_tool', 'validation_failed', 'intercepted', 'aborted', 'not_started'].includes(nonexecution.reason)
    && (local.isError === true || (call.tool === 'continuation_propose' && nonexecution.reason === 'intercepted'))) return true
  const recordedDigest = local?.details?.[NATIVE_RESPONSE_DIGEST]
  if (recordedDigest !== undefined && recordedDigest !== expected.details[NATIVE_RESPONSE_DIGEST]) return false
  const view = message => ({ content: message.content ?? [], isError: message.isError === true })
  return nativeToolInputDigest(view(local)) === nativeToolInputDigest(view(expected))
}

export async function reconcileNativeSession({ manager, runId, contextGeneration, requestHost, signal }) {
  const { branch, calls } = nativeSessionToolCalls(manager, contextGeneration)
  if (!calls.length) return { checked: 0, appended: 0 }
  if (typeof runId !== 'string' || !runId.trim()) {
    throw recoveryError('session.native_recovery_identity_required', 'Managed Pi tool calls need a Host Run identity before the session can open.')
  }
  const payload = { runId, contextGeneration, calls: calls.map(({ callId, tool, inputDigest, hasResult }) => ({
    callId, tool, inputDigest, hasResult,
  })) }
  validatePayload('ToolCallFactsRequest', payload)
  const response = await requestHost(TOOL_CALL_FACTS, payload, signal)
  const body = response?.payload ?? {}
  if (body.isError) {
    const code = body.errorDetails?.code ?? 'runtime.native_recovery_inconsistent'
    throw recoveryError('session.native_recovery_inconsistent', `${code}: ${body.error ?? 'Host rejected native session reconciliation.'}`)
  }
  validatePayload('ToolCallFactsResponse', body)
  if (body.contextGeneration !== contextGeneration || body.facts.length !== calls.length) {
    throw recoveryError('session.native_recovery_protocol', 'Host facts do not cover this Pi session generation and every managed tool call.')
  }
  for (let index = 0; index < calls.length; index += 1) {
    if (body.facts[index].callId !== calls[index].callId) {
      throw recoveryError('session.native_recovery_protocol', 'Host facts changed the managed tool call order.')
    }
  }
  // Do not write any result or move the branch while an effect is unresolved
  // or an approval is still pending. A later reconciliation can recover the
  // original call ID after the Host receives its real settlement.
  if (body.facts.some(fact => fact.state === 'waiting_approval')) {
    throw recoveryError('session.native_waiting_approval', 'The Pi session is waiting for Host approval; no new prompt can start.')
  }
  if (body.blocked || body.facts.some(fact => fact.state === 'outcome_unknown')) {
    throw recoveryError('session.native_recovery_pending', 'A managed tool outcome is unknown and needs Host reconciliation before a new prompt.')
  }
  const expected = []
  for (let index = 0; index < calls.length; index += 1) {
    const fact = body.facts[index]
    if (!['committed', 'not_executed'].includes(fact.state) || !fact.response || typeof fact.response !== 'object'
      || Array.isArray(fact.response) || (fact.response.isError !== true && !Object.hasOwn(fact.response, 'result'))
      || (fact.state === 'not_executed' && fact.response.isError !== true)) {
      throw recoveryError('session.native_recovery_protocol', 'A settled Host fact must carry its original response payload.')
    }
    expected.push(recoveredResult(calls[index], fact))
  }
  const differences = calls.filter((call, index) => !resultMatches(call, body.facts[index], expected[index]))
  if (!differences.length) return { checked: calls.length, appended: 0 }
  const changedBatches = new Set(differences.map(call => call.entryId))
  if (changedBatches.size !== 1) {
    throw recoveryError('session.native_recovery_inconsistent', 'Host results disagree with more than one Pi assistant batch.')
  }
  const assistantId = differences[0].entryId
  const batchIndex = branch.findIndex(entry => entry.id === assistantId)
  if (['aborted', 'error'].includes(branch[batchIndex]?.message?.stopReason)) {
    throw recoveryError('session.native_recovery_inconsistent', 'The tool-call assistant itself is incomplete; its provider context cannot be rebuilt automatically.')
  }
  const batchCalls = calls.filter(call => call.entryId === assistantId)
  const batchIds = new Set(batchCalls.map(call => call.callId))
  if (branch.slice(batchIndex + 1).some(entry => entry.type === 'message'
    && !((entry.message?.role === 'toolResult' && batchIds.has(entry.message.toolCallId))
      || (entry.message?.role === 'assistant' && entry.message.stopReason === 'aborted'
        && !entry.message.content?.some(block => block.type === 'toolCall'))))) {
    throw recoveryError('session.native_recovery_inconsistent', 'The Pi session continued past a tool batch whose results disagree with the Host.')
  }
  const staged = calls.map((call, index) => ({ call, message: expected[index] }))
    .filter(({ call }) => call.entryId === assistantId).map(({ message }) => message)
  signal?.throwIfAborted()
  manager.branch(assistantId)
  for (const message of staged) manager.appendMessage(message)
  return { checked: calls.length, appended: staged.length }
}
