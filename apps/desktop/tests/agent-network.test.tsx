import { expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { AgentNetworkPanel } from '../src/features/chat/components/AgentNetworkPanel'
import type { AgentNetworkClient } from '../src/features/conversations/model/agent-network'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

test('preview actions preserve conversation and Host preview identity', async () => {
  const host = document.createElement('div'), root = createRoot(host)
  const preview = { pid: 123, sessionKey: 'host-session-key', port: 8080, url: 'http://127.0.0.1:8080/' }
  const calls: unknown[] = []
  const client: AgentNetworkClient = {
    agentBackendNetworkStatus: async () => ({ previews: [preview], projectId: 'project-one' }),
    agentBackendPreviewAction: async (...args) => { calls.push(['preview', ...args]); return true },
  }
  try {
    await act(async () => root.render(<AgentNetworkPanel client={client} conversationId="conversation-one" />))
    await act(async () => { const details = host.querySelector('details')!; details.open = true; details.dispatchEvent(new Event('toggle')); await new Promise(resolve => setTimeout(resolve, 20)) })
    const button = (label: string) => [...host.querySelectorAll('button')].find(button => button.textContent === label)!
    expect(host.textContent).not.toContain('网络授权')
    await act(async () => button('打开预览').click())
    await act(async () => button('停止预览和当前 Agent').click())
    expect(calls).toEqual([
      ['preview', 'conversation-one', preview, false],
      ['preview', 'conversation-one', preview, true],
    ])
  } finally { await act(async () => root.unmount()) }
})
