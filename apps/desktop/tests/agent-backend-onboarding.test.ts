// UX batch ①②: login guidance + permission mode of an external backend.
// Permission-mode model plus source assertions (UI 1103: the login card is gone).
import { describe, expect, test } from 'bun:test'
import { existsSync, readFileSync } from 'node:fs'
import {
  effectiveDraftPermissionMode,
  externalPermissionModeOptions,
  FULL_ACCESS_PRIVATE_HOME_NOTE,
  FULL_ACCESS_RISK_POINTS,
  permissionChipLabel,
  REVOKE_FULL_ACCESS_NOTICE,
  type AgentBackendPermissionStatus,
} from '../src/features/conversations/model/agent-backend-onboarding'

const allowed: AgentBackendPermissionStatus = { fullAccessForbidden: false, forbiddenReason: null, engineeringSwitch: false }
const forbidden: AgentBackendPermissionStatus = { fullAccessForbidden: true, forbiddenReason: '管理员已禁止完全访问', engineeringSwitch: false }

describe('permission mode (semantics A)', () => {
  test('full access option is disabled with the reason under FORBIDDEN', () => {
    const open = externalPermissionModeOptions(allowed)
    expect(open.map((o) => [o.value, o.disabled])).toEqual([['standard', false], ['full_access', false]])
    const closed = externalPermissionModeOptions(forbidden)
    expect(closed[1]).toMatchObject({ value: 'full_access', disabled: true, reason: '管理员已禁止完全访问' })
    expect(externalPermissionModeOptions({ ...forbidden, forbiddenReason: null })[1].reason).toBe('本机已禁止完全访问')
  })

  test('effective draft mode never sends full_access when forbidden', () => {
    expect(effectiveDraftPermissionMode('full_access', allowed)).toBe('full_access')
    expect(effectiveDraftPermissionMode('full_access', forbidden)).toBe('standard')
    expect(effectiveDraftPermissionMode('standard', allowed)).toBe('standard')
  })

  test('the bar names the Agent-native boundary, not a Fox lock', () => {
    expect(permissionChipLabel(null)).toBeNull()
    expect(permissionChipLabel({ ...allowed, mode: 'full_access', granted: true, locked: true })).toBe('隔离：Fox 不额外包装（Agent 原生）')
    expect(permissionChipLabel({ ...allowed, mode: 'standard', locked: true })).toBe('隔离：Fox 不额外包装（Agent 原生）')
  })

  test('risk copy describes the Agent native mode and private HOME', () => {
    expect(FULL_ACCESS_RISK_POINTS).toHaveLength(3)
    expect(FULL_ACCESS_RISK_POINTS[0]).toContain('Fox 不额外包装')
    expect(FULL_ACCESS_RISK_POINTS[1]).toContain('原生沙箱')
    expect(FULL_ACCESS_RISK_POINTS[2]).toContain('当前用户')
    expect(FULL_ACCESS_PRIVATE_HOME_NOTE).toContain('私有 HOME')
    expect(REVOKE_FULL_ACCESS_NOTICE).toContain('原生方式重建')
  })
})

describe('source assertions', () => {
  const bar = readFileSync(new URL('../src/features/chat/components/AgentBackendBar.tsx', import.meta.url), 'utf8')
  const panel = readFileSync(new URL('../src/features/chat/components/AgentBackendPanel.tsx', import.meta.url), 'utf8')

  const settings = readFileSync(new URL('../src/features/settings/agent-backend-settings.tsx', import.meta.url), 'utf8')

  test('there is no Fox-wide full-access switch or revoke control', () => {
    expect(settings).not.toContain('新外部会话使用完全访问')
    expect(bar).not.toContain('function DraftPermissionPicker')
    expect(bar).not.toContain('aria-label="新会话的权限模式"')
    expect(bar).not.toContain('撤销完全访问')
    // Composer batch: the bar no longer carries a permission picker or the
    // isolation note; the one approval selector is in the composer.
    expect(bar).not.toContain('隔离：Fox 不额外包装（Agent 原生）')
    expect(bar).not.toContain('{permissionPicker}')
  })

  const client = readFileSync(new URL('../src/features/conversations/api/desktop-client.ts', import.meta.url), 'utf8')


  test('a new conversation renders no login card and no permission chip: one supported driver', () => {
    // The only login / permission UI a new conversation could show belongs to an
    // external driver, and the single Pi mainline retired every one of them
    // (2026-10-27). The card stays wired for the records already made - an open
    // conversation - and is never offered for a new conversation.
    expect(bar).not.toContain('新会话：')
    const draft = bar.slice(bar.indexOf('data-access="draft"'))
    expect(draft).toContain('{refusalNotice}')
    expect(draft).not.toContain('permissionPicker')
    // A retired backend picked by an older caller is shown and created as the
    // built-in route, so it is never probed for credentials.
    expect(panel).toContain('isAgentBackendRetired(pickedDraftBackend, status)')
    expect(panel).toContain("agentBackendAccess(shownBackend, status) === 'runnable'")
  })

  test('UI 1103: 设置 → Agent probes each external Agent by backend, with no conversation needed', () => {
    expect(settings).toContain('const target: AgentBackendTarget = { backend, fullAccess: false }')
    expect(settings).toContain('credentials(target)')
    expect(client).toContain("command<CredentialProbe>('agent_backend_credentials', target)")
    expect(client).toContain('{ ...target, selected, allProjects, overwrite, reuseForNewProjects }')
  })

  test('UI 1103: the login card above the composer is gone; nothing above the composer probes or imports a login', () => {
    expect(existsSync(new URL('../src/features/chat/components/AgentBackendCredentialCard.tsx', import.meta.url))).toBe(false)
    expect(panel).not.toContain('AgentBackendCredentialCard')
    expect(panel).not.toContain('agentBackendCredentials')
    expect(panel).not.toContain('agentBackendImportCredentials')
    expect(bar).not.toContain('credentialCard')
    expect(bar).not.toContain('importReceipt')
  })
})
