import { createRoot } from 'react-dom/client'
import { RuntimeApprovalPrompt } from '../../src/features/chat/components/RuntimeApprovalPrompt'
import { AgentNetworkPanel } from '../../src/features/chat/components/AgentNetworkPanel'
import type { ApprovalRecord } from '../../src/features/conversations/model/types'
import '../../src/styles/globals.css'
import '../../src/styles/workbench.css'
import '../../src/features/chat/components/agent-backend-bar.css'

document.documentElement.dataset.colorTheme = 'graphite'
document.documentElement.classList.toggle('dark', new URLSearchParams(location.search).has('dark'))
const preview = { pid: 123, sessionKey: 'fixture-only', port: 8080, url: 'http://127.0.0.1:8080/' }
const target = { host: 'registry.npmjs.org', port: 443 }
const record = (value: unknown) => { (window as any).__networkAction = value; return Promise.resolve(true) }
const client = {
  agentBackendNetworkStatus: async () => ({ previews: [preview], conversationRules: [target], projectRules: [{ host: 'cdn.example.com', port: 443 }], projectId: 'fixture-project' }),
  agentBackendPreviewAction: (...args: unknown[]) => record(args),
  agentBackendNetworkRule: (...args: unknown[]) => record(args),
}
const approval = { id: 'fixture-network-ticket', toolName: 'agent_permission', request: { hostNetworkRequest: true, input: { host: 'registry.npmjs.org', port: 443 }, availableDecisions: ['deny', 'allow_once', 'allow_conversation'] } } as unknown as ApprovalRecord
createRoot(document.getElementById('root')!).render(<main style={{ margin: '40px auto', padding: '16px', maxWidth: '760px' }}>
  <RuntimeApprovalPrompt approval={approval} onResolve={(...args) => record(args)} />
  <AgentNetworkPanel client={client} conversationId="fixture-conversation" />
</main>)
