import { describe, expect, test } from 'bun:test'
import { withWorkspaceInitializationTimeout } from '../src/features/conversations/model/workspace-initialization'

describe('workspace initialization timeout', () => {
  test('returns a completed initialization result', async () => {
    await expect(withWorkspaceInitializationTimeout(Promise.resolve('ready'), '工作区', 20)).resolves.toBe('ready')
  })

  test('rejects a stalled invoke instead of leaving the workspace loading forever', async () => {
    await expect(withWorkspaceInitializationTimeout(new Promise(() => undefined), '工作区', 5)).rejects.toThrow('工作区超时')
  })
})
