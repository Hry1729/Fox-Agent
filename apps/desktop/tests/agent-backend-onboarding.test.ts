// UX batch ①②: login guidance + permission mode of an external backend.
// Model behaviour (three probe states, their copy and actions, names only)
// plus source assertions on AgentBackendBar / AgentBackendCredentialCard.
import { describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import {
  credentialCardModel,
  credentialProbeTarget,
  credentialProbeTargetKey,
  IMPORT_RECEIPT_VISIBLE_MS,
  importReceiptLines,
  effectiveDraftPermissionMode,
  externalPermissionModeOptions,
  FULL_ACCESS_PRIVATE_HOME_NOTE,
  FULL_ACCESS_RISK_POINTS,
  importConfirmationRows,
  importReceipt,
  backupDirectoryLabel,
  permissionChipLabel,
  REVOKE_FULL_ACCESS_NOTICE,
  type AgentBackendPermissionStatus,
  type CredentialProbe,
  type ImportItem,
  unifiedLoginNote,
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
    expect(card.title).toBe('本机网关连接未就绪')
    expect(card.description).toContain('Fox 不再为外部 Agent 添加 SRT 网络代理')
    expect(card.description).toContain('https://api.deepseek.com/anthropic')
    expect(card.offerImport).toBe(false)
    expect(card.requiredKeys).toEqual([])
    expect(card.templates.map((t) => t.id)).toEqual(['deepseek_official'])
    noValue(card)
  })

  test('loopback no longer depends on a Fox full-access grant', () => {
    const p = probe({ state: 'loopback_requires_full_access', loopbackBlocked: true })
    const card = credentialCardModel(p, forbidden)!
    expect(card.description).toContain('本机网关正在运行')
    expect(card.description).not.toContain('新外部会话使用完全访问')
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
      .toBe('已复制到 3 个项目的私有 HOME：auth.json；1 项已不在可导入列表中，未复制')
    expect(importReceipt({ backend: 'pi', homes: 1, files: [], keys: [], skipped: [] })).toBe('没有复制任何内容')
  })

  // 80b1622 acceptance: "已复制到15 个项目" had no space; the count is the
  // projects actually written, and every number is spaced from its CJK text.
  test('receipt: spaced count of the projects actually written', () => {
    const line = importReceipt({ backend: 'codex', homes: 16, homesWritten: 15, files: ['.codex/auth.json'], keys: [], skipped: [] })
    expect(line).toBe('已复制到 15 个项目的私有 HOME：auth.json')
    for (const text of importReceiptLines({ backend: 'codex', homes: 16, homesWritten: 15, files: ['.codex/auth.json'], keys: [], skipped: ['a', 'b'], skippedExisting: [{ project: 'p1', label: '主项目', items: ['.codex/auth.json'] }] })) {
      expect(text).not.toMatch(/[\u4e00-\u9fff]\d|\d[\u4e00-\u9fff]/)
    }
  })

  test('receipt: existing logins kept are listed by project and file; nothing written says so', () => {
    const lines = importReceiptLines({
      backend: 'codex', homes: 3, homesWritten: 1, overwrite: false, files: ['.codex/auth.json'], keys: [], skipped: [],
      skippedExisting: [
        { project: 'p1', label: '主项目', items: ['.codex/auth.json'] },
        { project: 'p2', label: '第二个项目', items: ['.codex/auth.json', '.codex/config.toml'] },
      ],
      backups: [],
    })
    expect(lines).toEqual([
      '已复制到 1 个项目的私有 HOME：auth.json',
      '已有登录，未覆盖 2 个项目：主项目（auth.json）、第二个项目（auth.json、config.toml）',
    ])
    expect(importReceiptLines({ backend: 'deepseek', homes: 1, homesWritten: 0, files: [], keys: [], skipped: [], skippedExisting: [{ project: 'p1', label: '主项目', items: ['DEEPSEEK_API_KEY'] }] }))
      .toEqual(['没有复制任何内容', '已有登录，未覆盖 1 个项目：主项目（DEEPSEEK_API_KEY）'])
  })

  // Settings batch, item 3: the receipt names the backup RELATIVE to the
  // private HOME (no SID / profile path), the same form as the audit.
  test('receipt: overwrite names the backup location relative to the private HOME', () => {
    const report = {
      backend: 'codex', homes: 1, homesWritten: 1, overwrite: true, files: ['.codex/auth.json'], keys: [], skipped: [], skippedExisting: [],
      backups: [{ project: 'p1', label: '主项目', directory: '.fox-credential-backups/20261022-084512', items: ['.codex/auth.json'] }],
    }
    expect(importReceiptLines(report, { windows: true })[1]).toBe('已覆盖 主项目 的 auth.json，原文件备份在私有 HOME 下的 .fox-credential-backups\\20261022-084512')
    expect(importReceiptLines(report, { windows: false })[1]).toBe('已覆盖 主项目 的 auth.json，原文件备份在私有 HOME 下的 .fox-credential-backups/20261022-084512')
    // An older Host that still sent the absolute path: only the relative tail is shown.
    const legacy = { ...report, backups: [{ ...report.backups[0], directory: 'C:\\ProgramData\\Fox\\sandbox\\S-1-5-21-1\\homes\\p1\\codex\\.fox-credential-backups\\20261022-084512' }] }
    const line = importReceiptLines(legacy, { windows: true })[1]
    expect(line).toBe('已覆盖 主项目 的 auth.json，原文件备份在私有 HOME 下的 .fox-credential-backups\\20261022-084512')
    expect(line).not.toContain('S-1-5-21')
    expect(line).not.toContain('ProgramData')
    expect(backupDirectoryLabel('/home/u/.local/share/fox/sandbox-homes/p1/codex/.fox-credential-backups/20261022-084512-2', false)).toBe('.fox-credential-backups/20261022-084512-2')
    noValue(importReceiptLines(report))
  })
})

describe('probe target: the card shows before the first message (80b1622)', () => {
  test('a new conversation with an external backend and a project is probed by backend + project', () => {
    expect(credentialProbeTarget({ lockedBackend: null, conversationId: null, draftBackend: 'codex', draftProjectId: 'p1', draftProjectRoot: 'D:/proj', draftFullAccess: false }))
      .toEqual({ backend: 'codex', projectId: 'p1', projectRoot: 'D:/proj', fullAccess: false })
    expect(credentialProbeTarget({ lockedBackend: null, conversationId: null, draftBackend: 'deepseek', draftProjectRoot: 'D:/proj', draftFullAccess: true }))
      .toEqual({ backend: 'deepseek', projectRoot: 'D:/proj', fullAccess: true })
  })

  test('Fox, or a draft without a project, has no probe; a locked conversation is probed by id', () => {
    expect(credentialProbeTarget({ lockedBackend: null, conversationId: null, draftBackend: 'fox', draftProjectId: 'p1' })).toBeNull()
    expect(credentialProbeTarget({ lockedBackend: null, conversationId: null, draftBackend: 'codex' })).toBeNull()
    expect(credentialProbeTarget({ lockedBackend: 'codex', conversationId: 'c1', draftBackend: 'fox' })).toEqual({ conversationId: 'c1' })
    expect(credentialProbeTarget({ lockedBackend: 'fox', conversationId: 'c1', draftBackend: 'codex', draftProjectId: 'p1' })).toBeNull()
  })

  test('the key changes with the target only', () => {
    const a = credentialProbeTarget({ lockedBackend: null, conversationId: null, draftBackend: 'codex', draftProjectId: 'p1' })
    const b = credentialProbeTarget({ lockedBackend: null, conversationId: null, draftBackend: 'codex', draftProjectId: 'p1' })
    expect(credentialProbeTargetKey(a)).toBe(credentialProbeTargetKey(b))
    expect(credentialProbeTargetKey(a)).not.toBe(credentialProbeTargetKey(credentialProbeTarget({ lockedBackend: null, conversationId: null, draftBackend: 'pi', draftProjectId: 'p1' })))
    expect(credentialProbeTargetKey(null)).toBe('')
    expect(JSON.parse(credentialProbeTargetKey(a))).toEqual(a)
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

  test('the bar names the Agent-native boundary, not a Fox lock', () => {
    expect(permissionChipLabel(null)).toBeNull()
    expect(permissionChipLabel({ ...allowed, mode: 'full_access', granted: true, locked: true })).toBe('隔离：Fox 不额外包装（Agent 原生）')
    expect(permissionChipLabel({ ...allowed, mode: 'sandboxed', locked: true })).toBe('隔离：Fox 不额外包装（Agent 原生）')
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
  const card = readFileSync(new URL('../src/features/chat/components/AgentBackendCredentialCard.tsx', import.meta.url), 'utf8')

  const settings = readFileSync(new URL('../src/features/settings/agent-backend-settings.tsx', import.meta.url), 'utf8')

  test('there is no Fox-wide full-access switch or revoke control', () => {
    expect(settings).not.toContain('新外部会话使用完全访问')
    expect(bar).not.toContain('function DraftPermissionPicker')
    expect(bar).not.toContain('aria-label="新会话的权限模式"')
    expect(bar).not.toContain('撤销完全访问')
    expect(bar).toContain('隔离：Fox 不额外包装（Agent 原生）')
  })

  const workbench = readFileSync(new URL('../src/features/chat/workbench.tsx', import.meta.url), 'utf8')
  const client = readFileSync(new URL('../src/features/conversations/api/desktop-client.ts', import.meta.url), 'utf8')

  const panel = readFileSync(new URL('../src/features/chat/components/AgentBackendPanel.tsx', import.meta.url), 'utf8')

  test('AgentBackendBar shows the login card in the draft too (same as a locked conversation)', () => {
    expect(bar).toContain("{access === 'runnable' && credentialCard}")
    const draft = bar.slice(bar.indexOf('data-access="draft" aria-label="新会话的 Agent 后端"'))
    expect(draft).toContain('{credentialCard}')
    // The draft card is not hidden behind the refusal notice.
    expect(draft.indexOf('{credentialCard}')).toBeGreaterThan(draft.indexOf('{refusalNotice ??'))
  })

  test('the import receipt is rendered once, by the bar, keyed to its target', () => {
    expect(bar).toContain('function ImportReceiptNotice(')
    expect(bar).toContain('aria-label="导入回执"')
    expect(bar.match(/\{receiptNotice\}/g)?.length).toBe(2)
    expect(panel).toContain('setReceipt({ key: targetKey, value: importReceiptLines(report) })')
    expect(panel).toContain('importReceipt={visibleReceipt}')
    expect(panel).toContain('IMPORT_RECEIPT_VISIBLE_MS')
    expect(workbench).toContain('<AgentBackendPanel')
    // The card no longer echoes a successful import (fix ②).
    expect(card).not.toContain('importReceipt(')
    expect(IMPORT_RECEIPT_VISIBLE_MS).toBeGreaterThanOrEqual(10_000)
  })

  test('the panel probes a new conversation by backend + project, with no conversation needed', () => {
    expect(panel).toContain('credentialProbeTarget({')
    expect(panel).toContain('client.agentBackendCredentials(JSON.parse(key) as AgentBackendTarget)')
    expect(panel).toContain('key={targetKey}')
    expect(client).toContain("command<CredentialProbe>('agent_backend_credentials', target)")
    expect(client).toContain('{ ...target, selected, allProjects, overwrite, reuseForNewProjects }')
  })

  test('AgentBackendCredentialCard: 覆盖已有登录 is explicit and off by default', () => {
    expect(card).toContain('const [overwrite, setOverwrite] = useState(false)')
    expect(card).toContain('覆盖已有登录')
    expect(card).toContain('onImport(selected, allProjects, overwrite, reuseForNewProjects)')
    expect(card).toContain('默认跳过已经有这些文件或 key 的项目（包括本项目），不会覆盖。')
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

// Settings batch, item 6: the unified login (Fox「模型服务」) as the card sees it.
describe('unified login in the login card', () => {
  test('a provider that cannot drive the backend says why on the card; names only', () => {
    const unsupported = probe({ unifiedLogin: { backend: 'deepseek', outcome: 'unsupported', provider: 'DashScope', files: [], userFiles: [], reason: 'DeepSeek 后端（dsh）只支持 Anthropic Messages 协议' } })
    expect(unifiedLoginNote(unsupported)).toBe('没能用“模型服务”的供应商「DashScope」自动登录：DeepSeek 后端（dsh）只支持 Anthropic Messages 协议')
    const card = credentialCardModel(unsupported, allowed)!
    expect(card.description).toContain('DashScope')
    noValue(card)
    // Generated / unchanged: the Host already reports `ready`, so no card.
    expect(credentialCardModel(probe({ state: 'ready', credentialsPresent: true, unifiedLogin: { backend: 'pi', outcome: 'generated', provider: 'DeepSeek', files: ['.pi/agent/models.json'], userFiles: [], reason: null } }), allowed)).toBeNull()
    // No 模型服务: the card points at it as the first fix.
    expect(credentialCardModel(probe(), allowed)!.description).toContain('设置 → 模型服务')
    expect(unifiedLoginNote(probe({ unifiedLogin: null }))).toBeNull()
  })
})
