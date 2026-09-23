import { describe, expect, test, mock } from 'bun:test'

// The restore request identity is the contract that lets the Host deduplicate a
// repeated delivery of ONE user action. These tests pin the client half of it:
// the identity is forwarded verbatim, never regenerated per call.
const realTauriCore = await import('@tauri-apps/api/core')
const invokeCalls: Array<{ command: string; args: Record<string, any> }> = []
mock.module('@tauri-apps/api/core', () => ({
  ...realTauriCore,
  invoke: async (command: string, args: Record<string, any>) => {
    invokeCalls.push({ command, args })
    return { ok: true, data: { id: 'v-new', versionNo: 3 } }
  },
}))

const { desktopClient } = await import(
  '../src/features/conversations/api/desktop-client'
)

const lastRequest = () => invokeCalls.at(-1)!.args.request as Record<string, unknown>

describe('managed file restore request identity', () => {
  test('forwards a stable requestId verbatim on every resend', async () => {
    invokeCalls.length = 0
    const requestId = 'ui-action-42'
    await desktopClient.restoreManagedFileVersion('conv-1', 'ver-1', false, requestId)
    await desktopClient.restoreManagedFileVersion('conv-1', 'ver-1', false, requestId)

    expect(invokeCalls).toHaveLength(2)
    for (const call of invokeCalls) {
      expect(call.command).toBe('managed_file_restore')
      const request = call.args.request as Record<string, unknown>
      expect(request.requestId).toBe(requestId)
      expect(request.conversationId).toBe('conv-1')
      expect(request.versionId).toBe('ver-1')
    }
  })

  test('sends null when the caller supplies no identity, so the Host mints one', async () => {
    invokeCalls.length = 0
    await desktopClient.restoreManagedFileVersion('conv-1', 'ver-1', false)
    expect(lastRequest().requestId).toBeNull()
  })

  test('keeps force and the identity independent', async () => {
    invokeCalls.length = 0
    await desktopClient.restoreManagedFileVersion('conv-1', 'ver-1', true, 'ui-action-7')
    expect(lastRequest().force).toBe(true)
    expect(lastRequest().requestId).toBe('ui-action-7')
  })
})
