// 普通模式下，外部 Agent 没配好时用户能看到的地方都不能露出路径、密钥名、网关和错误码，也不能留下原生 details。
// UI 1103 起输入框上方不再有登录卡，这些地方是：设置 → Agent 的外部 Agent 行、发送没开始时的那一条通知，
// 以及输入框上方的 Agent 栏（普通模式下应当是空的）。
import { expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { AgentBackendSettingsSection } from '../src/features/settings/agent-backend-settings'
import { AgentBackendPanel } from '../src/features/chat/components/AgentBackendPanel'
import { sendNotStartedNotice } from '../src/features/chat/send-failure-notice'
import { DEFAULT_AGENT_BACKEND_PREFERENCES, type AgentBackendStatus } from '../src/features/conversations/model/agent-backend'
import type { CredentialProbe } from '../src/features/conversations/model/agent-backend-onboarding'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true

const probe = {
  backend: 'deepseek',
  state: 'missing_credentials',
  privateHome: 'D:\\fox-test\\data-1335\\agent-homes\\uuid\\deepseek',
  credentialsPresent: false,
  presentKeys: [],
  presentFiles: [],
  requiredKeys: ['DEEPSEEK_API_KEY'],
  requiredFiles: ['.dsh/.env', '.dsh/profiles/acp/cordis.patch.yml'],
  templates: [{
    id: 'magpie',
    title: '经 Magpie 本机网关',
    note: '直连官方 api.deepseek.com',
    files: [{ path: '.dsh/.env', content: 'DEEPSEEK_API_KEY=' }],
  }],
  unifiedLogin: {
    backend: 'deepseek',
    outcome: 'unsupported',
    provider: 'SenseNova',
    files: [],
    userFiles: [],
    reason: 'provider.protocol_missing: no anthropic-messages profile',
  },
  source: null,
  installed: true,
} as CredentialProbe

const leaks = ['DEEPSEEK_API_KEY', '.dsh/', '私有 HOME', 'Magpie', 'model.connection_required', 'provider.protocol_missing', 'agent_backend_', 'D:\\', 'cordis']

const status: AgentBackendStatus = {
  switchEnv: 'FOX_EXTERNAL_BACKENDS',
  anyExternalEnabled: true,
  supportedBackends: ['fox', 'pi', 'codex', 'deepseek'],
  retiredBackends: [],
  options: (['fox', 'pi', 'codex', 'deepseek'] as const).map((backend) => ({ backend, enabled: true, wired: true, experimental: backend !== 'fox', hidden: false, retired: false, reason: null })),
  unknownEntries: [],
}

async function mount(node: React.ReactNode) {
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  await act(async () => root.render(node))
  await act(async () => { await new Promise((resolve) => setTimeout(resolve, 20)) })
  return {
    host,
    unmount: async () => { await act(async () => root.unmount()); host.remove() },
  }
}

test('设置 → Agent 的外部 Agent 行只有状态和开关，不露出调试信息', async () => {
  const view = await mount(<AgentBackendSettingsSection
    client={{
      agentBackendPreferences: async () => DEFAULT_AGENT_BACKEND_PREFERENCES,
      saveAgentBackendPreferences: async () => DEFAULT_AGENT_BACKEND_PREFERENCES,
      probeCodexResponses: async () => null,
      agentBackendCredentials: async () => probe,
      agentBackendModelConnection: async () => ({ status: 'missing_address', providerId: null, providerName: 'SenseNova', apiType: 'anthropic-messages', candidates: [], message: 'model.connection_required' }),
    }}
    onOpenModelSettings={() => undefined}
  />)
  try {
    const text = view.host.textContent ?? ''
    expect(text).toContain('未配置')
    expect(text).toContain('去模型服务配置')
    for (const leak of leaks) expect(text).not.toContain(leak)
    expect(view.host.querySelectorAll('details').length).toBe(0)
  } finally {
    await view.unmount()
  }
})

test('普通模式下输入框上方没有任何外部 Agent 卡片', async () => {
  const client = {
    agentBackendCapabilities: async () => { throw new Error('not in normal mode') },
    agentBackendPermissionStatus: async () => ({ fullAccessForbidden: false, forbiddenReason: null, engineeringSwitch: false }),
    agentBackendRevokeFullAccess: async () => ({}),
    listenRuntimeEvents: async () => () => {},
  }
  for (const locked of [null, 'deepseek'] as const) {
    const view = await mount(<AgentBackendPanel client={client} status={status} lockedBackend={locked} conversationId={locked ? 'c-ds' : null} draftBackend="deepseek" developerUi={false} />)
    try {
      expect(view.host.textContent ?? '').toBe('')
      expect(view.host.querySelectorAll('details, button').length).toBe(0)
    } finally {
      await view.unmount()
    }
  }
})

test('外部 Agent 没配好时的那一条通知是人话，带去设置的入口', () => {
  const raw = 'model.connection_required: 请先在后端栏补充或验证当前供应商的模型连接 D:\\fox-test\\agent-homes DEEPSEEK_API_KEY .dsh/.env Magpie'
  for (const code of ['model.connection_required', 'agent_backend_missing_credentials', 'agent_backend_not_installed', 'agent_backend_exited_on_start', 'agent_backend_pi_source_choice_required', 'agent_backend_user_disabled']) {
    const notice = sendNotStartedNotice(code, raw, { backend: 'deepseek', developerUi: false })
    for (const leak of leaks) {
      expect(notice.title).not.toContain(leak)
      expect(notice.description).not.toContain(leak)
    }
    expect(notice.workspaceView).toBe('settings-ai')
  }
})
