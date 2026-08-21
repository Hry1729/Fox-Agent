import { describe, expect, test } from 'bun:test'
import { resolveConversationAgentId } from '../src/features/conversations/model/agent-initialization'

describe('conversation agent initialization', () => {
  test('uses an existing draft agent without initializing the runtime', async () => {
    let initializeCalls = 0
    const resolved = await resolveConversationAgentId(
      [null, ' fox-general ', null],
      async () => {
        initializeCalls += 1
        return { defaultAgentId: 'unused-agent' }
      },
    )

    expect(resolved).toEqual({ agentId: 'fox-general', initialized: false })
    expect(initializeCalls).toBe(0)
  })

  test('loads the Host default when React state is not ready yet', async () => {
    const resolved = await resolveConversationAgentId(
      [null, undefined, ''],
      async () => ({ defaultAgentId: 'fox-general' }),
    )

    expect(resolved).toEqual({ agentId: 'fox-general', initialized: true })
  })

  test('rejects an invalid Host initialization result', async () => {
    expect(resolveConversationAgentId(
      [null, undefined],
      async () => ({ defaultAgentId: '  ' }),
    )).rejects.toThrow('默认专家初始化失败')
  })
})
