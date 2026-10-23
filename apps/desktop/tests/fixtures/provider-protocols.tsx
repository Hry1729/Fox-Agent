import { useState } from 'react'
import { createRoot } from 'react-dom/client'
import { TooltipProvider } from '../../src/components/ui/tooltip'
import { ProviderEditor } from '../../src/features/settings/settings-pages'
import { AgentModelConnectionCard, type AgentModelConnection } from '../../src/features/chat/components/AgentModelConnectionCard'
import { desktopClient } from '../../src/features/conversations/api/desktop-client'
import '../../src/styles/globals.css'
import '../../src/styles/themes.css'
import '../../src/styles/workspace-pages.css'
const fixture: any = { saved: null, tests: [] }
;(window as any).protocolFixture = fixture
desktopClient.testModelService = async (...args: any[]) => {
  fixture.tests.push(args.map((value, index) => index === 1 && value ? '<synthetic-key>' : value))
  return { ok: true, baseUrl: args[0], connectionType: 'remote', status: 'connected', latencyMs: 7, models: [args[3]] }
}
function Fixture() {
  const [connection, setConnection] = useState<AgentModelConnection | null>(null)
  fixture.setConnection = setConnection
  const [form, setForm] = useState<any>({ name: '测试供应商', baseUrl: 'https://example.com/v1', apiType: 'anthropic-messages', apiKey: '', clearApiKey: false, isDefault: true, models: [{ modelId: 'native-model', displayName: '', contextWindow: 128000, maxOutputTokens: 8192, supportsImageInput: false, isDefault: true }], protocolProfiles: [], protocolKeys: {} })
  fixture.form = form
  return <TooltipProvider><main style={{ maxWidth: 1000, margin: '24px auto', padding: 16 }}><h1>模型服务</h1><ProviderEditor form={form} busy={false} credentialConfigured={false} onChange={setForm} onCancel={() => {}} onSave={() => { fixture.saved = form }} onTest={() => {}} />{connection && <AgentModelConnectionCard backend="codex" connection={connection} onSave={async choice => { fixture.connectionChoice = choice; setConnection({ ...connection, status: 'ready' }) }} />}</main></TooltipProvider>
}
document.documentElement.dataset.colorTheme = 'graphite'
createRoot(document.getElementById('root')!).render(<Fixture />)
