import { describe, expect, test } from 'bun:test'
import {
  expertErrorMessage,
  expertErrorPresentation,
  expertInteractionLocked,
  latestExpertToolAvailability,
  mergeExpertBindingsIntoTimeline,
} from '../src/features/chat/components/expert-binding-ui'
import type { AgentRecord, RunEventRecord } from '../src/features/conversations/model/types'

function expert(overrides: Partial<AgentRecord> = {}): AgentRecord {
  return {
    id: 'fox-debugger',
    name: 'Debugger',
    description: '',
    runtimeType: 'pi',
    defaultModel: '',
    icon: null,
    category: 'development',
    openingSuggestions: [],
    systemPrompt: '',
    isBuiltin: true,
    packageVersion: '1',
    packageManifest: { allowedTools: ['read', 'write', 'search'] },
    capabilities: {},
    resources: {
      tools: [{ id: 'read', name: 'read', description: '' }],
      knowledges: [],
      mcps: [{ id: 'docs', name: 'docs', description: '' }],
      skills: [{ id: 'debug', name: 'debug', description: '' }],
    },
    configurableItems: {},
    isDefault: false,
    available: true,
    ...overrides,
  }
}

function snapshot(seq: number, event: Record<string, unknown>): RunEventRecord {
  return {
    runId: `run-${seq}`,
    seq,
    eventType: 'run.request_snapshot',
    event: { type: 'run.request_snapshot', ...event },
    createdAt: seq,
  }
}

describe('expert binding timeline UI', () => {
  test('keeps active and historical activation records in timestamp order', () => {
    const entries = mergeExpertBindingsIntoTimeline(
      [
        { id: 'message-1', createdAt: 100 },
        { id: 'message-2', createdAt: 300 },
      ],
      [
        { id: 'binding-active', expertId: 'expert-3', state: 'active' as const, activatedAt: 400 },
        { id: 'binding-replaced', expertId: 'expert-1', state: 'replaced' as const, activatedAt: 200 },
        { id: 'binding-removed', expertId: 'expert-2', state: 'removed' as const, activatedAt: 250 },
      ],
    )

    expect(entries.map((entry) => entry.kind === 'message' ? entry.message.id : entry.binding.id)).toEqual([
      'message-1',
      'binding-replaced',
      'binding-removed',
      'message-2',
      'binding-active',
    ])
  })

  test('locks changes during runs, approvals, and follow-up questions', () => {
    expect(expertInteractionLocked({ running: true, approvalPending: false, questionPending: false })).toBe(true)
    expect(expertInteractionLocked({ running: false, approvalPending: true, questionPending: false })).toBe(true)
    expect(expertInteractionLocked({ running: false, approvalPending: false, questionPending: true })).toBe(true)
    expect(expertInteractionLocked({ running: false, approvalPending: false, questionPending: false })).toBe(false)
  })

  test('reads the latest matching request snapshot and reports the expert tool intersection', () => {
    const availability = latestExpertToolAvailability([
      snapshot(1, {
        expertBinding: { expertId: 'fox-debugger' },
        expertPackage: { id: 'fox-debugger', packageManifest: { allowedTools: ['read', 'write', 'search'] } },
        toolNames: ['read'],
      }),
      snapshot(2, {
        expertBinding: { expertId: 'another-expert' },
        expertPackage: { id: 'another-expert', packageManifest: { allowedTools: ['other'] } },
        toolNames: ['other'],
      }),
      snapshot(3, {
        expertBinding: { expertId: 'fox-debugger' },
        expertPackage: { id: 'fox-debugger', packageManifest: { allowedTools: ['read', 'write', 'search'] } },
        assistantDeclaredToolNames: ['read', 'search', 'unrelated'],
        expertDeclaredToolNames: ['read', 'write', 'search'],
        effectiveToolNames: ['read', 'search', 'unrelated'],
        excludedTools: [{ name: 'write', reason: 'assistant_allowlist' }],
      }),
    ], expert())

    expect(availability).toMatchObject({
      source: 'snapshot',
      availableCount: 2,
      declaredCount: 3,
      label: '上次运行 2/3 工具可用',
      emptyIntersection: false,
    })
  })

  test('falls back to the declaration summary when no request snapshot exists', () => {
    const availability = latestExpertToolAvailability([], expert())

    expect(availability.source).toBe('declaration')
    expect(availability.label).toBe('声明能力：3 个工具 · 1 个 Skills · 1 个 MCP')
    expect(availability.availableCount).toBeNull()
  })

  test('keeps text answers available when the tool intersection is empty', () => {
    const availability = latestExpertToolAvailability([
      snapshot(1, {
        expertBinding: { expertId: 'fox-debugger' },
        expertPackage: { id: 'fox-debugger', packageManifest: { allowedTools: ['write'] } },
        expertDeclaredToolNames: ['write'],
        effectiveToolNames: [],
        toolNames: ['read'],
      }),
    ], expert())

    expect(availability.label).toBe('上次运行 0/1 工具可用')
    expect(availability.emptyIntersection).toBe(true)
    expect(availability.textOnlyAvailable).toBe(true)
  })

  test('supports legacy snapshots that only expose toolNames', () => {
    const availability = latestExpertToolAvailability([
      snapshot(1, {
        expertBinding: { expertId: 'fox-debugger' },
        expertPackage: { id: 'fox-debugger', packageManifest: { allowedTools: ['read', 'write'] } },
        toolNames: ['read'],
      }),
    ], expert())

    expect(availability.label).toBe('上次运行 1/2 工具可用')
  })

  test('maps structured expert errors to Chinese guidance and preserves unknown fallback', () => {
    expect(expertErrorPresentation({ code: 'conversation.expert_remote_unavailable', message: 'remote unavailable', retryable: true })).toMatchObject({ kind: 'offline', retryable: true })
    expect(expertErrorPresentation({ code: 'conversation.expert_locked', message: 'locked', retryable: false })).toMatchObject({ kind: 'locked', retryable: false })
    expect(expertErrorPresentation({ code: 'expert.not_found', message: 'missing', retryable: false })).toMatchObject({ kind: 'missing' })
    expect(expertErrorPresentation({ code: 'conversation.expert_invalid_role', message: 'invalid role', retryable: false })).toMatchObject({ kind: 'invalid' })
    expect(expertErrorPresentation({ code: 'conversation.expert_snapshot_invalid', message: 'invalid snapshot', retryable: false })).toMatchObject({ kind: 'snapshot' })
    expect(expertErrorPresentation({ code: 'model.failed', message: 'provider error', retryable: true })).toBeNull()
    expect(expertErrorMessage({ code: 'model.failed', message: 'provider error', retryable: true }, 'fallback')).toBe('provider error')
  })
})
