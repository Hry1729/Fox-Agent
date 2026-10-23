import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import {
  AGENT_BACKEND_SESSION_EVENT,
  refreshesAgentBackendCapabilities,
} from '../src/features/conversations/model/agent-backend'

const notice = (conversationId: string, type: string) => ({
  conversationId,
  runtimeSessionId: null,
  runId: 'run-1',
  seq: 1,
  timestamp: '2026-10-21T00:00:00.000Z',
  event: { type },
})

describe('capability chips refresh on the backend handshake (item 7)', () => {
  test('agent_backend.session of the shown conversation refreshes', () => {
    expect(AGENT_BACKEND_SESSION_EVENT).toBe('agent_backend.session')
    expect(refreshesAgentBackendCapabilities(notice('c-1', 'agent_backend.session'), 'c-1')).toBe(true)
  })

  test('other conversations, other events and no conversation do not', () => {
    expect(refreshesAgentBackendCapabilities(notice('c-2', 'agent_backend.session'), 'c-1')).toBe(false)
    expect(refreshesAgentBackendCapabilities(notice('c-1', 'run.completed'), 'c-1')).toBe(false)
    expect(refreshesAgentBackendCapabilities(notice('c-1', 'run.started'), 'c-1')).toBe(false)
    expect(refreshesAgentBackendCapabilities(notice('c-1', 'agent_backend.session'), null)).toBe(false)
    expect(refreshesAgentBackendCapabilities(null, 'c-1')).toBe(false)
  })

  // Settings batch: the logic moved from the workbench into AgentBackendPanel.
  test('the panel reloads capabilities on the session event, not on lastRun status', () => {
    const source = readFileSync(new URL('../src/features/chat/components/AgentBackendPanel.tsx', import.meta.url), 'utf8')
    const start = source.indexOf('// Refresh the chips as soon as the backend')
    const end = source.indexOf('const permissionKey')
    expect(start).toBeGreaterThan(0)
    expect(end).toBeGreaterThan(start)
    const block = source.slice(start, end)
    expect(block).toContain('client.listenRuntimeEvents(')
    expect(block).toContain('refreshesAgentBackendCapabilities(notification, capabilityConversationId)')
    expect(block).toContain('sessionTick + eventTick')
    expect(block).not.toContain('lastRunStatus')
    const workbench = readFileSync(new URL('../src/features/chat/workbench.tsx', import.meta.url), 'utf8')
    expect(workbench).toContain('<AgentBackendPanel')
    expect(workbench).not.toContain('lastRun?.status ?? null\n  const [agentBackendCapabilities')
  })
})
