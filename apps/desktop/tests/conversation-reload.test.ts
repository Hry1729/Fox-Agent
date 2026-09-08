import { loadConversationWithRetry } from '../src/features/conversations/model/conversation-reload'

describe('conversation terminal reload', () => {
  test('retries transient failures and returns the persisted detail', async () => {
    let calls = 0
    const result = await loadConversationWithRetry(async () => {
      calls += 1
      if (calls < 3) throw new Error('database busy')
      return { id: 'conversation-1' }
    }, 'conversation-1', { attempts: 3, delayMs: 0 })

    expect(result).toEqual({ id: 'conversation-1' })
    expect(calls).toBe(3)
  })

  test('surfaces the final failure after the retry budget', async () => {
    let calls = 0
    await expect(loadConversationWithRetry(async () => {
      calls += 1
      throw new Error('still unavailable')
    }, 'conversation-1', { attempts: 2, delayMs: 0 })).rejects.toThrow('still unavailable')
    expect(calls).toBe(2)
  })
})
