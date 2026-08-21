import { describe, expect, test } from 'bun:test'
import {
  activeConversationExpertBinding,
  canRemoveConversationExpert,
  conversationExpertBindingLockError,
} from '../src/features/conversations/hooks/use-desktop-conversation'
import type { ConversationDetail, ConversationExpertBinding } from '../src/features/conversations/model/types'

function binding(state: ConversationExpertBinding['state'] = 'active'): ConversationExpertBinding {
  return {
    id: `binding-${state}`,
    conversationId: 'conversation-1',
    expertId: 'fox-debugger',
    state,
    activationSource: 'draft',
    displaySnapshot: {
      name: 'Debugger',
      description: '',
      icon: null,
      category: 'development',
    },
    packageSnapshot: {},
    activatedAt: 1,
  }
}

function detail(): ConversationDetail {
  return {
    conversation: {
      id: 'conversation-1',
      agentId: 'fox-general',
      agentName: 'Fox',
      title: 'Draft',
      projectId: null,
      projectRoot: null,
      status: 'active',
      archived: false,
      createdAt: 1,
      updatedAt: 1,
      lastMessageAt: null,
    },
    expertBindings: [binding()],
    messages: [],
    runtimeEvents: [],
    toolCalls: [],
    approvals: [],
    attachments: [],
    artifacts: [],
    knowledgeBindings: [],
    lastRun: null,
    hasEarlierMessages: false,
    goals: [],
    tasks: [],
    evidence: [],
    planRevisions: [],
    reviewFindings: [],
    acceptances: [],
  }
}

describe('conversation expert state', () => {
  test('restores only the active expert binding', () => {
    const current = detail()
    current.expertBindings = [binding('replaced'), binding('active')]

    expect(activeConversationExpertBinding(current)?.state).toBe('active')
  })

  test('allows removal from a draft or empty materialized conversation', () => {
    expect(canRemoveConversationExpert(null)).toBe(true)
    expect(canRemoveConversationExpert(detail())).toBe(true)
  })

  test('blocks removal after messages, during active runs, or when archived', () => {
    const withMessage = detail()
    withMessage.messages.push({
      id: 'message-1',
      conversationId: 'conversation-1',
      runId: null,
      role: 'user',
      kind: 'text',
      content: 'hello',
      status: 'completed',
      ordinal: 1,
      createdAt: 1,
      updatedAt: 1,
    })
    const withRun = detail()
    withRun.lastRun = {
      id: 'run-1',
      conversationId: 'conversation-1',
      runtimeSessionId: null,
      status: 'running',
      model: '',
      startedAt: 1,
      finishedAt: null,
      errorCode: null,
      errorMessage: null,
      lastSeq: 0,
    }
    const archived = detail()
    archived.conversation.archived = true

    expect(canRemoveConversationExpert(withMessage)).toBe(false)
    expect(conversationExpertBindingLockError(withMessage)?.code).toBe('conversation.expert_binding_locked')
    expect(canRemoveConversationExpert(withRun)).toBe(false)
    expect(canRemoveConversationExpert(archived)).toBe(false)
  })

  test('does not treat host-only timeline records as conversation messages', () => {
    const current = detail()
    current.messages.push({
      id: 'message-system',
      conversationId: 'conversation-1',
      runId: null,
      role: 'system',
      kind: 'expert_activation',
      content: '',
      status: 'completed',
      ordinal: 1,
      createdAt: 1,
      updatedAt: 1,
    })

    expect(canRemoveConversationExpert(current)).toBe(true)
    expect(conversationExpertBindingLockError(current)).toBeNull()
  })
})
