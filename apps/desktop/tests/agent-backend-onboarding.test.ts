// UX batch ①②: login guidance + permission mode of an external backend.
// Model behaviour (three probe states, their copy and actions, names only)
// plus source assertions on AgentBackendBar / AgentBackendCredentialCard.
import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import {
  canRevokeFullAccess,
  credentialCardModel,
  effectiveDraftPermissionMode,
  externalPermissionModeOptions,
  FULL_ACCESS_PRIVATE_HOME_NOTE,
  FULL_ACCESS_RISK_POINTS,
  importConfirmationRows,
  importReceipt,
  permissionChipLabel,
  REVOKE_FULL_ACCESS_NOTICE,
  type AgentBackendPermissionStatus,
  type CredentialProbe,
  type ImportItem,
} from '../src/features/conversations/model/agent-backend-onboarding'

// A value that must never reach the UI model. The Host never sends one; the
// probe below carries it nowhere either, and every rendered string is checked.
const SECRET = 'sk-test-0123456789abcdef'

function probe(overrides: Partial<CredentialProbe> = {}): CredentialProbe {
  return {
    backend: 'deepseek',
    mode: 'sandboxed',
    state: 'missing_credentials',
    privateHome: '/fox/private-home/p1/deepseek',
    credentialsPresent: false,
    loopbackBlocked: false,
    presentKeys: [],
    presentFiles: [],
    requiredKeys: ['DEEPSEEK_API_KEY'],
    requiredFiles: ['.dsh/.env'],
    templates: [
      { id: 'deepseek_official', title: '官方端点', note: '把 key 填进 .dsh/.env', files: [{ path: '.dsh/.env', content: 'DEEPSEEK_API_KEY=<你的 key>' }] },
      { id: 'magpie_loopback', title: '本机 Magpie', note: '需要完全访问', files: [{ path: '.dsh/.env', content: 'DEEPSEEK_BASE_URL=http://127.0.0.1:8080' }] },
    ],
    ...overrides,
  }
}

const allowed: AgentBackendPermissionStatus = { fullAccessForbidden: false, forbiddenReason: null, engineeringSwitch: false }
const forbidden: AgentBackendPermissionStatus = { fullAccessForbidden: true, forbiddenReason: '管理员已禁止完全访问', engineeringSwitch: false }

function noValue(model: unknown) {
  expect(JSON.stringify(model)).not.toContain(SECRET)
  expect(JSON.stringify(model)).not.toMatch(/sk-[A-Za-z0-9]{8,}/)
}

describe('credentialCardModel: three probe states', () => {
  test('ready → no card', () => {
    expect(credentialCardModel(probe({ state: 'ready', credentialsPresent: true, presentKeys: ['DEEPSEEK_API_KEY'] }), allowed)).toBeNull()
    expect(credentialCardModel(null)).toBeNull()
  })

  test('missing_credentials → private HOME, required names, templates, import offered', () => {
    const card = credentialCardModel(probe(), allowed)!
    expect(card.kind).toBe('missing')
    expect(card.title).toBe('这个后端还没有登录')
    expect(card.description).toContain('私有 HOME')
    expect(card.description).toContain('从真实 HOME 导入')
    expect(card.privateHome).toBe('/fox/private-home/p1/deepseek')
    expect(card.requiredKeys).toEqual(['DEEPSEEK_API_KEY'])
    expect(card.requiredFiles).toEqual(['.dsh/.env'])
    expect(card.templates.map((t) => t.id)).toEqual(['deepseek_official', 'magpie_loopback'])
    expect(card.offerImport).toBe(true)
    noValue(card)
  })

  test('loopback_requires_full_access → different card: full access at creation or official endpoint, no import', () => {
    const p = probe({ state: 'loopback_requires_full_access', credentialsPresent: true, loopbackBlocked: true, presentKeys: ['DEEPSEEK_API_KEY'], presentFiles: ['.dsh/.env'] })
    const card = credentialCardModel(p, allowed)!
    expect(card.kind).toBe('loopback')
    expect(card.title).toBe('本机网关在沙箱模式下不可用')
    expect(card.description).toContain('新建会话并选择“完全访问”')
    expect(card.description).toContain('https://api.deepseek.com/anthropic')
    expect(card.offerImport).toBe(false)
    expect(card.requiredKeys).toEqual([])
    expect(card.templates.map((t) => t.id)).toEqual(['deepseek_official'])
    noValue(card)
  })

  test('loopback under FORBIDDEN → only the official endpoint is suggested', () => {
    const p = probe({ state: 'loopback_requires_full_access', loopbackBlocked: true })
    const card = credentialCardModel(p, forbidden)!
    expect(card.description).toContain('本机已禁止完全访问')
    expect(card.description).not.toContain('选择“完全访问”')
  })
})

describe('import confirmation and receipt: names only', () => {
  const items: ImportItem[] = [
    { id: 'file:.dsh/cordis.patch.yml', kind: 'file', name: 'cordis.patch.yml', destination: '.dsh/cordis.patch.yml' },
    { id: 'env:DEEPSEEK_API_KEY', kind: 'env_key', name: 'DEEPSEEK_API_KEY', destination: '.dsh/.env' },
  ]

  test('rows show file / key names and destinations', () => {
    const rows = importConfirmationRows(items)
    expect(rows.map((r) => r.id)).toEqual(['file:.dsh/cordis.patch.yml', 'env:DEEPSEEK_API_KEY'])
    expect(rows[0].label).toBe('cordis.patch.yml')
    expect(rows[1].label).toBe('DEEPSEEK_API_KEY（.dsh/.env 中的一行）')
    expect(rows[1].detail).toBe('复制到私有 HOME 的 .dsh/.env')
    noValue(rows)
  })

  test('receipt: one project vs all projects, skipped count, nothing copied', () => {
    expect(importReceipt({ backend: 'deepseek', homes: 1, files: ['.dsh/cordis.patch.yml'], keys: ['DEEPSEEK_API_KEY'], skipped: [] }))
      .toBe('已复制到本项目的私有 HOME：cordis.patch.yml、DEEPSEEK_API_KEY')
    expect(importReceipt({ backend: 'codex', homes: 3, files: ['.codex/auth.json'], keys: [], skipped: ['x'] }))
      .toBe('已复制到3 个项目的私有 HOME：auth.json；1 项已不在可导入列表中，未复制')
    expect(importReceipt({ backend: 'pi', homes: 1, files: [], keys: [], skipped: [] })).toBe('没有复制任何内容')
  })
})

describe('permission mode (semantics A)', () => {
  test('full access option is disabled with the reason under FORBIDDEN', () => {
    const open = externalPermissionModeOptions(allowed)
    expect(open.map((o) => [o.value, o.disabled])).toEqual([['sandboxed', false], ['full_access', false]])
    const closed = externalPermissionModeOptions(forbidden)
    expect(closed[1]).toMatchObject({ value: 'full_access', disabled: true, reason: '管理员已禁止完全访问' })
    expect(externalPermissionModeOptions({ ...forbidden, forbiddenReason: null })[1].reason).toBe('本机已禁止完全访问')
  })

  test('effective draft mode never sends full_access when forbidden', () => {
    expect(effectiveDraftPermissionMode('full_access', allowed)).toBe('full_access')
    expect(effectiveDraftPermissionMode('full_access', forbidden)).toBe('sandboxed')
    expect(effectiveDraftPermissionMode('sandboxed', allowed)).toBe('sandboxed')
  })

  test('lock chip and revocability', () => {
    expect(permissionChipLabel(null)).toBeNull()
    expect(permissionChipLabel({ ...allowed, mode: 'full_access', granted: true, locked: true })).toBe('权限：完全访问（已锁定）')
    expect(permissionChipLabel({ ...allowed, mode: 'sandboxed', locked: true })).toBe('权限：沙箱（已锁定）')
    expect(canRevokeFullAccess({ ...allowed, granted: true })).toBe(true)
    // Engineering switch without a user grant: nothing to revoke from the UI.
    expect(canRevokeFullAccess({ ...allowed, engineeringSwitch: true, mode: 'full_access' })).toBe(false)
  })

  test('risk copy: three points + private HOME; revoke notice says it restarts', () => {
    expect(FULL_ACCESS_RISK_POINTS).toHaveLength(3)
    expect(FULL_ACCESS_RISK_POINTS[0]).toContain('srt')
    expect(FULL_ACCESS_RISK_POINTS[1]).toContain('目录边界')
    expect(FULL_ACCESS_RISK_POINTS[2]).toContain('任何文件')
    expect(FULL_ACCESS_PRIVATE_HOME_NOTE).toContain('私有 HOME')
    expect(REVOKE_FULL_ACCESS_NOTICE).toContain('沙箱中重新启动')
  })
})

describe('source assertions', () => {
  const bar = readFileSync(new URL('../src/features/chat/components/AgentBackendBar.tsx', import.meta.url), 'utf8')
  const card = readFileSync(new URL('../src/features/chat/components/AgentBackendCredentialCard.tsx', import.meta.url), 'utf8')

  test('AgentBackendBar risk confirmation renders the three points and the private HOME line before enabling full access', () => {
    expect(bar).toContain('FULL_ACCESS_RISK_POINTS.map(')
    expect(bar).toContain('{FULL_ACCESS_PRIVATE_HOME_NOTE}')
    expect(bar).toContain('role="alertdialog" aria-label="完全访问风险确认"')
    // Picking full access only opens the confirmation; onChange('full_access') happens on the confirm button.
    expect(bar).toMatch(/if \(next === 'full_access'\) \{\s*if \(!forbidden\) setConfirming\(true\)\s*return/)
    expect(bar).toContain(`onClick={() => { setConfirming(false); onChange('full_access') }}>我已了解，使用完全访问`)
  })

  test('AgentBackendBar greys out full access under FORBIDDEN', () => {
    expect(bar).toContain('disabled={option.disabled}')
    expect(bar).toContain("value={forbidden ? 'sandboxed' : mode}")
    expect(bar).toContain("{option.disabled ? '（已禁止）' : ''}")
  })

  test('AgentBackendBar warns that revoking restarts before calling onRevoke', () => {
    expect(bar).toContain('{REVOKE_FULL_ACCESS_NOTICE}')
    expect(bar).toContain('onClick={() => setConfirming(true)}>撤销完全访问')
    expect(bar).toContain('onClick={() => { setConfirming(false); onRevoke() }}>撤销并重启到沙箱')
    expect(bar).toContain('canRevokeFullAccess(permission)')
  })

  test('AgentBackendCredentialCard: open folder, import confirmation by name, 套用到我所有项目', () => {
    expect(card).toContain('打开文件夹')
    expect(card).toContain('从真实 HOME 导入…')
    expect(card).toContain('importConfirmationRows(candidates)')
    expect(card).toContain('只复制，不引用；只显示文件名和 key 名')
    expect(card).toContain('套用到我所有项目')
    expect(card).toContain('disabled={busy || selected.length === 0}')
    // No value is ever read or rendered: only names / destinations from the Host.
    expect(card).not.toMatch(/\.value\b(?!s)/)
  })
})
