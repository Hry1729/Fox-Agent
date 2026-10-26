import { expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { AgentNetworkPanel } from '../src/features/chat/components/AgentNetworkPanel'
import { RuntimeApprovalPrompt } from '../src/features/chat/components/RuntimeApprovalPrompt'
import type { AgentNetworkClient } from '../src/features/conversations/model/agent-network'
import type { ApprovalRecord } from '../src/features/conversations/model/types'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

test('network approval identifies the exact port and submits the Host ticket and scope', async () => {
  const host = document.createElement('div'), root = createRoot(host)
  const calls: unknown[] = []
  const approval = { id: 'network-ticket', toolName: 'agent_permission', request: { hostNetworkRequest: true, input: { host: '127.0.0.1', port: 8080 }, availableDecisions: ['deny', 'allow_once', 'allow_conversation'] } } as unknown as ApprovalRecord
  try {
    await act(async () => root.render(<RuntimeApprovalPrompt approval={approval} onResolve={async (...args) => { calls.push(args); return true }} />))
    expect(host.textContent).toContain('连接网络')
    expect(host.textContent).toContain('127.0.0.1:8080')
    expect(host.textContent).not.toContain('读取文件')
    const button = [...host.querySelectorAll('button')].find(button => button.textContent === '本对话内允许')!
    await act(async () => button.click())
    expect(calls).toEqual([['network-ticket', 'allow_conversation']])
  } finally { await act(async () => root.unmount()) }
})

test('preview and rule actions preserve conversation and Host preview identity', async () => {
  const host = document.createElement('div'), root = createRoot(host)
  const preview = { pid: 123, sessionKey: 'host-session-key', port: 8080, url: 'http://127.0.0.1:8080/' }
  const target = { host: 'registry.npmjs.org', port: 443 }
  const calls: unknown[] = []
  const client: AgentNetworkClient = {
    agentBackendNetworkStatus: async () => ({ previews: [preview], conversationRules: [target], projectRules: [], projectId: 'project-one' }),
    agentBackendPreviewAction: async (...args) => { calls.push(['preview', ...args]); return true },
    agentBackendNetworkRule: async (...args) => { calls.push(['rule', ...args]); return true },
  }
  try {
    await act(async () => root.render(<AgentNetworkPanel client={client} conversationId="conversation-one" />))
    await act(async () => { const details = host.querySelector('details')!; details.open = true; details.dispatchEvent(new Event('toggle')); await new Promise(resolve => setTimeout(resolve, 20)) })
    const button = (label: string) => [...host.querySelectorAll('button')].find(button => button.textContent === label)!
    await act(async () => button('打开预览').click())
    await act(async () => button('停止预览和当前 Agent').click())
    await act(async () => button('此项目长期允许').click())
    await act(async () => button('撤销并停止当前 Agent').click())
    expect(calls).toEqual([
      ['preview', 'conversation-one', preview, false],
      ['preview', 'conversation-one', preview, true],
      ['rule', 'conversation-one', target, true, true],
      ['rule', 'conversation-one', target, false, false],
    ])
  } finally { await act(async () => root.unmount()) }
})
