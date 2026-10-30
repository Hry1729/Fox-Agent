import { createRoot } from 'react-dom/client'
import { AgentNetworkPanel } from '../../src/features/chat/components/AgentNetworkPanel'
import '../../src/styles/globals.css'
import '../../src/styles/workbench.css'
import '../../src/features/chat/components/agent-backend-bar.css'

document.documentElement.dataset.colorTheme = 'graphite'
document.documentElement.classList.toggle('dark', new URLSearchParams(location.search).has('dark'))
const preview = { pid: 123, sessionKey: 'fixture-only', port: 8080, url: 'http://127.0.0.1:8080/' }
const record = (value: unknown) => { (window as any).__networkAction = value; return Promise.resolve(true) }
const client = {
  agentBackendNetworkStatus: async () => ({ previews: [preview], projectId: 'fixture-project' }),
  agentBackendPreviewAction: (...args: unknown[]) => record(args),
}
createRoot(document.getElementById('root')!).render(<main style={{ margin: '40px auto', padding: '16px', maxWidth: '760px' }}>
  <AgentNetworkPanel client={client} conversationId="fixture-conversation" />
</main>)
