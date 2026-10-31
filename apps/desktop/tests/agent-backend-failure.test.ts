import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { join } from 'node:path'
import { agentBackendFailureHint, lastAgentBackendFailure } from '../src/features/conversations/model/agent-backend-failure'
import type { ConversationDetail, RunRecord } from '../src/features/conversations/model/types'

// Plan C4: every start / failure code the Host reports maps to one actionable
// hint; nothing here retries or falls back.

const repoRoot = join(import.meta.dir, '..', '..', '..')

function run(status: string, errorCode: string | null): RunRecord {
  return {
    id: 'run-1',
    conversationId: 'conv-1',
    runtimeSessionId: null,
    status,
    model: 'm',
    startedAt: 1,
    finishedAt: 2,
    errorCode,
    errorMessage: errorCode ? 'failed' : null,
    lastSeq: 1,
  }
}

function detail(agentBackend: string | undefined, lastRun: RunRecord | null): ConversationDetail {
  return { conversation: { id: 'conv-1', agentBackend }, lastRun } as unknown as ConversationDetail
}

describe('agentBackendFailureHint', () => {
  test('C4 launch codes each have an action', () => {
    expect(agentBackendFailureHint('agent_backend_spawn_failed')?.action).toBe('install')
    expect(agentBackendFailureHint('agent_backend_exited_on_start')?.action).toBe('login')
    expect(agentBackendFailureHint('agent_backend_start_timeout')?.action).toBe('retry')
    expect(agentBackendFailureHint('agent_backend_protocol_error')?.action).toBe('upgrade')
    expect(agentBackendFailureHint('agent_backend_protocol_mismatch')?.action).toBe('upgrade')
  })

  test('C3 Fox tool bridge codes each have an action', () => {
    expect(agentBackendFailureHint('agent_backend_fox_tools_unavailable')?.action).toBe('install')
    expect(agentBackendFailureHint('agent_backend_fox_tools_unsupported')?.action).toBe('upgrade')
    expect(agentBackendFailureHint('agent_backend_fox_tools_rejected')?.action).toBe('configure')
  })

  test('the hint carries the code, a label and a non-empty detail', () => {
    const hint = agentBackendFailureHint('agent_backend_missing_credentials')
    expect(hint).not.toBeNull()
    expect(hint?.code).toBe('agent_backend_missing_credentials')
    expect(hint?.label).toBe('登录')
    expect(hint?.detail.length).toBeGreaterThan(0)
  })

  test('no code, or a code with no user action, has no hint', () => {
    expect(agentBackendFailureHint(null)).toBeNull()
    expect(agentBackendFailureHint(undefined)).toBeNull()
    expect(agentBackendFailureHint('')).toBeNull()
    expect(agentBackendFailureHint('agent_backend_turn_busy')).toBeNull()
    expect(agentBackendFailureHint('something_else')).toBeNull()
  })

  test('every Host failure_codes constant has a hint', () => {
    const source = readFileSync(join(repoRoot, 'crates/fox-agent-backend/src/external.rs'), 'utf8')
    const block = source.slice(source.indexOf('pub mod failure_codes'))
    const codes = [...block.slice(0, block.indexOf('\n}\n')).matchAll(/"(agent_backend_[a-z_]+)"/g)].map((match) => match[1])
    expect(codes.length).toBeGreaterThan(5)
    for (const code of codes) expect(agentBackendFailureHint(code)).not.toBeNull()
  })
})

describe('lastAgentBackendFailure', () => {
  test('an external conversation whose last run failed with a known code', () => {
    expect(lastAgentBackendFailure(detail('codex', run('failed', 'agent_backend_start_timeout')))?.action).toBe('retry')
    expect(lastAgentBackendFailure(detail('pi', run('failed', 'agent_backend_fox_tools_unsupported')))?.action).toBe('upgrade')
  })

  test('null for the built-in route, a missing backend, no detail or no run', () => {
    expect(lastAgentBackendFailure(detail('fox', run('failed', 'agent_backend_start_timeout')))).toBeNull()
    expect(lastAgentBackendFailure(detail(undefined, run('failed', 'agent_backend_start_timeout')))).toBeNull()
    expect(lastAgentBackendFailure(null)).toBeNull()
    expect(lastAgentBackendFailure(undefined)).toBeNull()
    expect(lastAgentBackendFailure(detail('codex', null))).toBeNull()
  })

  test('null for a running or successful run and for an unknown code', () => {
    expect(lastAgentBackendFailure(detail('codex', run('running', 'agent_backend_start_timeout')))).toBeNull()
    expect(lastAgentBackendFailure(detail('codex', run('completed', null)))).toBeNull()
    expect(lastAgentBackendFailure(detail('deepseek', run('failed', 'model.unknown')))).toBeNull()
  })
})
