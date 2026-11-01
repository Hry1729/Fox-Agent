// 普通模式的 dsh 未登录卡不能露出路径、密钥名、网关和错误码，也不能留下原生 details。
import { expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import { act } from 'react'
import { createRoot } from 'react-dom/client'
import { AgentBackendCredentialCard } from '../src/features/chat/components/AgentBackendCredentialCard'
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
} as CredentialProbe

const leaks = ['DEEPSEEK_API_KEY', '.dsh/', '私有 HOME', 'Magpie', 'model.connection_required', 'provider.protocol_missing']

async function renderCard(developerUi: boolean) {
  const host = document.createElement('div')
  document.body.append(host)
  const root = createRoot(host)
  await act(async () => root.render(
    <AgentBackendCredentialCard
      probe={probe}
      developerUi={developerUi}
      modelConnection={{ status: 'missing_address', providerId: null, providerName: 'SenseNova', apiType: 'anthropic-messages', candidates: [], message: 'model.connection_required' }}
      onOpenFolder={async () => undefined}
      onLoadCandidates={async () => []}
      onImport={async () => ({ backend: 'deepseek', homes: 0, files: [], keys: [], skipped: [] })}
      onOpenModelSettings={() => undefined}
    />,
  ))
  return {
    text: host.textContent ?? '',
    details: host.querySelectorAll('details').length,
    buttons: [...host.querySelectorAll('button')].map((button) => button.textContent),
    unmount: async () => { await act(async () => root.unmount()); host.remove() },
  }
}

test('普通模式的 dsh 未登录卡只有一句人话和一个按钮', async () => {
  const view = await renderCard(false)
  try {
    expect(view.text).toContain('dsh 还没有登录，先在模型服务里配好 Anthropic 地址再发送。')
    expect(view.buttons).toEqual(['填写 Anthropic 地址'])
    expect(view.text).not.toContain('用已有登录')
    for (const leak of leaks) expect(view.text).not.toContain(leak)
    expect(view.details).toBe(0)
  } finally {
    await view.unmount()
  }
})

test('开发者模式看得到登录细节，但不用原生 details', async () => {
  const view = await renderCard(true)
  try {
    expect(view.text).toContain('DEEPSEEK_API_KEY')
    expect(view.text).toContain('.dsh/')
    expect(view.text).toContain('私有 HOME')
    expect(view.text).toContain('Magpie')
    expect(view.text).toContain('provider.protocol_missing')
    expect(view.text).toContain('从真实 HOME 导入')
    expect(view.details).toBe(0)
    expect(view.text).toContain('经 Magpie 本机网关')
  } finally {
    await view.unmount()
  }
})
