// Real-DOM interaction acceptance for the two user decision surfaces that carry
// a request / approval identity:
//
//   1. The managed-file version surfaces (`useManagedFiles` + `VersionRows`) —
//      the restore action ("恢复" / "覆盖恢复" / "强制恢复（先备份当前内容）")
//      and the restore request identity the shared store hands to the Host.
//   2. `RuntimeApprovalPrompt` — the whole-file replacement approval and the
//      approval identity it submits on confirm / deny.
//
// SCOPE (honest): the components under test, their event handlers, the store and
// the request-identity module are the REAL ones, mounted with React DOM into a
// REAL happy-dom document, driven by real DOM click events. Only the Tauri IPC
// boundary (`desktopClient`) and the in-app notification side effect are mocked,
// because neither can exist in this process.
//
// A click-through of the packaged Tauri desktop app is still a separate manual
// acceptance item; nothing here claims to be that.

import { afterAll, afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

// Several DOM suites share one process, so the first one to run owns registration.
// Registering twice throws and aborts this file's setup mid-run.
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

type RestoreCall = {
  conversationId: string
  versionId: string
  force: boolean
  requestId: string | null
}

const restoreCalls: RestoreCall[] = []
let listError: Error | null = null
let listCalls = 0
let restoreImpl: (call: RestoreCall) => Promise<unknown> = async () => ({
  id: 'v-restored',
  versionNo: 9,
})

// The Host's own dedup contract: a repeated delivery of the SAME request identity
// reads the recorded result instead of mutating the file again. Reproduced at the
// boundary so the surface is held to it, without inventing Host behaviour.
const settledByRequestId = new Map<string, unknown>()
const dedupAtBoundary = (call: RestoreCall) => {
  if (call.requestId && settledByRequestId.has(call.requestId)) {
    return settledByRequestId.get(call.requestId)
  }
  return undefined
}

const versionRow = (overrides: Record<string, unknown>) => ({
  id: 'v1',
  runId: 'run-1',
  toolCallId: 'call-1',
  tool: 'write_file',
  storagePath: 'C:/tmp/a.txt',
  displayName: 'a.txt',
  versionNo: 1,
  changeKind: 'modified',
  beforeHash: null,
  beforeSize: null,
  afterHash: 'hash-1',
  afterSize: 10,
  backupPath: 'C:/tmp/backup-1',
  restoredFromId: null,
  createdAt: 1,
  backupAvailable: true,
  currentHash: 'hash-2',
  drifted: false,
  restoreSize: 10,
  canRestore: true,
  sourceKind: 'host_capture',
  restoreBlocker: null,
  ...overrides,
})

// Newest first, as the Host returns them. v2 is the file's current content, so
// only v1 offers a restore action.
let versions: unknown[] = [
  versionRow({ id: 'v2', versionNo: 2, createdAt: 2, currentHash: 'hash-2' }),
  versionRow({ id: 'v1', versionNo: 1, createdAt: 1 }),
]

// The real modules are loaded first so the mocks below can SPREAD them: a
// `mock.module` factory replaces the module for the whole process, and a partial
// factory would break every other test file that imports the same module.
const actualDesktopClient = {
  ...await import('../src/features/conversations/api/desktop-client'),
}
const actualNotifications = { ...await import('../src/features/notifications') }

// Bun module mocks persist across test files. Restore this suite's boundary so
// subsequent client-contract tests reach the real IPC adapter.
afterAll(() => {
  mock.module('../src/features/conversations/api/desktop-client', () => actualDesktopClient)
  mock.module('../src/features/notifications', () => actualNotifications)
})

mock.module('../src/features/conversations/api/desktop-client', () => ({
  ...actualDesktopClient,
  desktopRuntimeAvailable: true,
  desktopClient: {
    ...actualDesktopClient.desktopClient,
    listManagedFileVersions: async () => {
      listCalls += 1
      if (listError) throw listError
      return versions
    },
    restoreManagedFileVersion: async (
      conversationId: string,
      versionId: string,
      force = false,
      requestId?: string,
    ) => {
      const call: RestoreCall = {
        conversationId,
        versionId,
        force,
        requestId: requestId ?? null,
      }
      restoreCalls.push(call)
      const recorded = dedupAtBoundary(call)
      if (recorded !== undefined) return recorded
      const result = await restoreImpl(call)
      if (call.requestId) settledByRequestId.set(call.requestId, result)
      return result
    },
  },
}))

mock.module('../src/features/notifications', () => ({
  ...actualNotifications,
  notify: Object.assign(() => {}, {
    success: () => {},
    error: () => {},
    info: () => {},
    warning: () => {},
  }),
}))

const React = await import('react')
const { cleanup, fireEvent, render, screen, waitFor } = await import(
  '@testing-library/react'
)
const { useManagedFiles } = await import(
  '../src/features/chat/components/managed-files/use-managed-files'
)
const { ManagedFileVersions } = await import(
  '../src/features/chat/components/managed-files/ManagedFileVersions'
)
const { RuntimeApprovalPrompt } = await import(
  '../src/features/chat/components/RuntimeApprovalPrompt'
)
// The forced restore asks through the in-app confirmation; the stub answers it.
const { setConfirmDialogPresenter } = await import('../src/components/ui/confirm-dialog')
let confirmAnswer = true
const confirmTitles: string[] = []
setConfirmDialogPresenter(async (options) => {
  confirmTitles.push(options.title)
  return confirmAnswer
})

afterEach(() => {
  cleanup()
  listError = null
  listCalls = 0
  restoreCalls.length = 0
  settledByRequestId.clear()
  restoreImpl = async () => ({ id: 'v-restored', versionNo: 9 })
  versions = [
    versionRow({ id: 'v2', versionNo: 2, createdAt: 2, currentHash: 'hash-2' }),
    versionRow({ id: 'v1', versionNo: 1, createdAt: 1 }),
  ]
  confirmAnswer = true
  confirmTitles.length = 0
})

/** The store caches one source per conversation id for the whole process, so
 *  each test mounts under its own id instead of inheriting another test's rows. */
let convSeq = 0
const nextConvId = () => `conv-dom-${(convSeq += 1)}`

/** The right-sidebar 文件 page shape: the controller plus the version list. */
function Harness({ conversationId, refreshKey }: { conversationId: string; refreshKey?: string }) {
  const managed = useManagedFiles(conversationId, refreshKey)
  return <div>{managed ? <ManagedFileVersions controller={managed} /> : <span data-testid="no-surface" />}</div>
}

/** Mount the list and expand the one file group so the version rows appear.
 *  Returns the conversation id the surface was mounted under. */
async function mountRows(): Promise<string> {
  confirmAnswer = true
  const conversationId = nextConvId()
  render(<Harness conversationId={conversationId} />)
  const toggle = await screen.findByRole('button', { name: '2 个版本' })
  fireEvent.click(toggle)
  return conversationId
}

const restoreButton = () => screen.getByRole('button', { name: '恢复' })

describe('版本列表渲染契约（ManagedFileVersions）', () => {
  test('首次挂载读取一次，refreshKey 变化只补充一次静默检查', async () => {
    versions = []
    const conversationId = nextConvId()
    const view = render(<Harness conversationId={conversationId} refreshKey="running" />)
    await waitFor(() => expect(listCalls).toBe(1))
    expect(document.querySelector('.fox-file-versions')).toBeNull()
    versions = [versionRow({ id: 'v1', versionNo: 1, createdAt: 1 })]
    view.rerender(<Harness conversationId={conversationId} refreshKey="idle" />)
    await screen.findByRole('button', { name: '1 个版本' })
    expect(listCalls).toBe(2)
  })

  test('首次读取失败留下可见的重试入口，重试成功后错误消失', async () => {
    listError = new Error('文件版本暂时无法读取')
    render(<Harness conversationId={nextConvId()} />)
    const alert = await screen.findByRole('alert')
    expect(alert.textContent).toContain('文件版本暂时无法读取')
    listError = null
    fireEvent.click(screen.getByRole('button', { name: '重试' }))
    await waitFor(() => expect(screen.queryByRole('alert')).toBeNull())
    await screen.findByRole('button', { name: '2 个版本' })
  })

  test('没有记录时列表整体不渲染，也不留空壳', async () => {
    versions = []
    render(<Harness conversationId={nextConvId()} />)
    await waitFor(() => expect(listCalls).toBe(1))
    expect(document.querySelector('.fox-file-versions')).toBeNull()
  })

  // Server rendering never runs `useEffect`, so the version list is still empty
  // and the surface takes its documented early return. This covers the "no
  // interactive surface until the Host has answered" rule only; the affordances
  // themselves are covered by the real-DOM clicks below.
  test('Host 未回答前不渲染任何可操作面', async () => {
    const { renderToStaticMarkup } = await import('react-dom/server')
    const html = renderToStaticMarkup(<Harness conversationId={nextConvId()} />)
    expect(html).not.toContain('button')
  })

  test('最新行是当前内容，只有旧版提供恢复', async () => {
    await mountRows()
    expect(screen.getByText('当前')).toBeDefined()
    expect(restoreButton()).toBeDefined()
  })
})

describe('版本行恢复按钮（真实 DOM 点击）', () => {
  test('连点两次只产生一个恢复请求', async () => {
    // Undetermined on purpose: the first request is still in flight.
    restoreImpl = () => new Promise(() => {})
    const conversationId = await mountRows()

    const button = restoreButton()
    fireEvent.click(button)
    // The row disabled the action while it is undetermined; a second click in
    // the same double click must not become a second execution request.
    fireEvent.click(button)

    expect(restoreCalls).toHaveLength(1)
    expect(restoreCalls[0].conversationId).toBe(conversationId)
    expect(restoreCalls[0].versionId).toBe('v1')
    expect(restoreCalls[0].force).toBe(false)
    expect(typeof restoreCalls[0].requestId).toBe('string')
    expect(restoreCalls[0].requestId).not.toBe('')
  })

  test('超时未决后的重发保持同一逻辑标识', async () => {
    // 1. A drift rejection turns the row's action into its force form.
    restoreImpl = async () => {
      throw new Error('a.txt changed outside the recorded task history')
    }
    await mountRows()
    fireEvent.click(restoreButton())
    const forceButton = await screen.findByRole('button', { name: /强制恢复/ })

    // 2. The force confirmation is delivered but never answered: the client gives
    //    up on the wait, so the outcome is UNDETERMINED, not a refusal. The Host
    //    may already have recorded this request identity.
    restoreImpl = async () => {
      throw new Error('request timed out')
    }
    fireEvent.click(forceButton)
    await waitFor(() => expect(restoreCalls).toHaveLength(2))
    const firstForceId = restoreCalls[1].requestId
    expect(restoreCalls[1].force).toBe(true)
    // The failed wait re-opened the affordance instead of wedging the row.
    const resendButton = await screen.findByRole('button', { name: /强制恢复/ })

    // 3. The resend must carry the SAME identity, so the Host recognizes one
    //    logical request instead of writing the file twice.
    fireEvent.click(resendButton)
    await waitFor(() => expect(restoreCalls).toHaveLength(3))
    expect(restoreCalls[2].requestId).toBe(firstForceId)
    expect(restoreCalls[2].force).toBe(true)

    // Exactly two logical actions: the rejected normal one and the forced one.
    expect(new Set(restoreCalls.map((call) => call.requestId)).size).toBe(2)
  })

  test('确定成功后的新一次确认使用新标识', async () => {
    await mountRows()
    fireEvent.click(restoreButton())
    await waitFor(() => expect(restoreCalls).toHaveLength(1))

    // The first action ended definitely (the Host recorded a result), so the
    // identity is released and the next confirmation is a NEW action.
    await waitFor(() => expect(restoreButton().hasAttribute('disabled')).toBe(false))
    fireEvent.click(restoreButton())
    await waitFor(() => expect(restoreCalls).toHaveLength(2))

    expect(restoreCalls[0].requestId).not.toBe(restoreCalls[1].requestId)
    expect(restoreCalls.every((call) => call.force === false)).toBe(true)
  })

  test('取消强制确认不发出任何请求', async () => {
    restoreImpl = async () => {
      throw new Error('a.txt changed outside the recorded task history')
    }
    await mountRows()
    fireEvent.click(restoreButton())
    await waitFor(() => expect(restoreCalls).toHaveLength(1))
    const forceButton = await screen.findByRole('button', { name: /强制恢复/ })

    confirmAnswer = false
    fireEvent.click(forceButton)
    await waitFor(() => expect(confirmTitles).toEqual(['强制恢复文件']))
    // Give the awaited confirmation a chance to settle.
    await new Promise((resolve) => setTimeout(resolve, 20))
    expect(restoreCalls).toHaveLength(1)
    expect(screen.getByRole('button', { name: /强制恢复/ }).hasAttribute('disabled')).toBe(false)
  })

  test('普通恢复转强制恢复绝不复用标识', async () => {
    restoreImpl = async () => {
      throw new Error('a.txt changed outside the recorded task history')
    }
    await mountRows()
    fireEvent.click(restoreButton())
    await waitFor(() => expect(restoreCalls).toHaveLength(1))

    // The rejection is shown and the force affordance appears.
    const forceButton = await screen.findByRole('button', { name: /强制恢复/ })
    expect(document.querySelector('.fox-version-state.is-error')?.textContent).toContain(
      'changed outside the recorded task history',
    )
    expect(document.querySelector('.fox-version-note.is-force')?.textContent).toContain(
      'changed outside the recorded task history',
    )

    restoreImpl = async () => ({ id: 'v-restored', versionNo: 9 })
    fireEvent.click(forceButton)
    await waitFor(() => expect(restoreCalls).toHaveLength(2))

    // A forced overwrite carries different parameters, so it must not inherit the
    // normal action's identity: the Host binds a request to exactly one intent.
    expect(restoreCalls[0].force).toBe(false)
    expect(restoreCalls[1].force).toBe(true)
    expect(restoreCalls[1].requestId).not.toBe(restoreCalls[0].requestId)
  })
})

const replacementRequest = {
  category: 'tool_execution',
  availableDecisions: ['allow_once', 'deny'],
  title: '允许 Fox 整文件替换 a.txt？',
  target: 'C:/tmp/a.txt',
  summary: '写入整个文件内容',
  input: { path: 'C:/tmp/a.txt', content: 'whole file' },
  wholeFileReplacement: {
    purpose: '整文件替换',
    requestDigest: 'sha256:req-abc',
    targetIdentity: 'C:/tmp/a.txt',
    baselineVersion: 'sha256:base-1',
    candidateDigest: 'sha256:cand-2',
  },
}

const approvalRecord = (overrides: Record<string, unknown> = {}) => ({
  // The Kernel mints a VERSIONED approval ticket; the Host re-checks that exact
  // string in the transaction that consumes it.
  id: 'kernel-approval:run-1:write_file:v7',
  toolCallId: 'write-1',
  runId: 'run-1',
  conversationId: 'conv-1',
  toolName: 'write_file',
  status: 'pending',
  requestedAction: 'write_file',
  request: replacementRequest,
  decision: null,
  requestedAt: 1,
  resolvedAt: null,
  ...overrides,
})

const approvalTicket = 'kernel-approval:run-1:write_file:v7'

describe('RuntimeApprovalPrompt (real DOM clicks)', () => {
  test('shows the real purpose and the immutable replacement binding', async () => {
    render(
      React.createElement(RuntimeApprovalPrompt, {
        approval: approvalRecord() as any,
        onResolve: async () => true,
      }),
    )

    const binding = document.querySelector('.fox-approval-replacement')
    expect(binding).not.toBeNull()
    const text = binding!.textContent ?? ''
    expect(text).toContain('整文件替换')
    expect(text).toContain('这不是普通写入审批')
    expect(text).toContain('sha256:req-abc')
    expect(document.querySelector('[data-slot="approval-card"] > .aui-approval-operation')?.textContent).toBe('C:/tmp/a.txt')
    expect(text).toContain('sha256:base-1')
    expect(text).toContain('sha256:cand-2')

    // A replacement approval authorizes exactly one dispatch, so the reusable
    // "allow this whole conversation" choice must not be offered.
    expect(screen.queryByRole('button', { name: /本次(?:运行|对话)内允许/ })).toBeNull()
    expect(screen.getByRole('button', { name: '拒绝' })).toBeDefined()
    expect(screen.getByRole('button', { name: '只允许这一次' })).toBeDefined()
  })

  test('an ordinary approval exposes the conversation-scoped reuse and its real limits', async () => {
    render(
      React.createElement(RuntimeApprovalPrompt, {
        approval: approvalRecord({
          request: {
            category: 'tool_execution',
            availableDecisions: ['allow_once', 'deny', 'allow_conversation'],
            title: '允许 Fox 运行命令？',
            target: 'npm test',
            summary: '运行测试',
          },
        }) as any,
        onResolve: async () => true,
      }),
    )

    expect(document.querySelector('.fox-approval-replacement')).toBeNull()
    // The grant is stored against the conversation (and is revoked when the
    // policy generation moves), so the label must not claim a narrower lifetime.
    expect(screen.getByRole('button', { name: '本对话内允许' })).toBeDefined()
    // The user is told the real scope, that it is revocable, and that revoking or
    // changing the mode ends it — instead of an unqualified "allow this run".
    expect(document.querySelector('.fox-approval-scope-note')?.textContent).toContain('可随时撤销')
    expect(document.querySelector('.fox-approval-scope-note')?.textContent).toContain('立即失效')
    expect(document.querySelector('.fox-approval-scope-note')?.textContent).toContain('不包含执行任何脚本或命令')
  })

  test('a new file is described as a creation, not as a one-dispatch replacement', async () => {
    render(
      React.createElement(RuntimeApprovalPrompt, {
        approval: approvalRecord({
          request: {
            category: 'tool_execution',
            availableDecisions: ['allow_once', 'allow_conversation', 'deny'],
            title: '允许 Fox 写入新文件？',
            target: 'C:/tmp/notes.md',
            input: { path: 'C:/tmp/notes.md', content: 'hello\n' },
            // A legacy card may still carry the binding with the absent-target
            // baseline; it must never be read as a replacement ticket.
            wholeFileReplacement: {
              ...replacementRequest.wholeFileReplacement,
              baselineVersion: 'missing',
            },
          },
        }) as any,
        onResolve: async () => true,
      }),
    )

    const scope = document.querySelector('.fox-approval-scope-note')?.textContent ?? ''
    expect(scope).toContain('新建')
    expect(scope).toContain('不覆盖任何已有内容')
    expect(scope).not.toContain('仅授权本次操作')
    // Nothing existing is destroyed, so the conversation-scoped decision the Host
    // declared stays available.
    expect(screen.getByRole('button', { name: '本对话内允许' })).toBeDefined()
    expect(screen.getByRole('button', { name: '只允许这一次' })).toBeDefined()
    expect(screen.getByRole('button', { name: '拒绝' })).toBeDefined()
  })

  test('confirm and deny submit the exact versioned approval identity', async () => {
    const resolved: Array<[string, string]> = []
    render(
      React.createElement(RuntimeApprovalPrompt, {
        approval: approvalRecord() as any,
        onResolve: async (approvalId: string, decision: string) => {
          resolved.push([approvalId, decision])
          return true
        },
      }),
    )

    fireEvent.click(screen.getByRole('button', { name: '只允许这一次' }))
    await waitFor(() => expect(resolved).toHaveLength(1))
    expect(resolved[0]).toEqual([approvalTicket, 'allow_once'])

    cleanup()
    resolved.length = 0
    render(
      React.createElement(RuntimeApprovalPrompt, {
        approval: approvalRecord() as any,
        onResolve: async (approvalId: string, decision: string) => {
          resolved.push([approvalId, decision])
          return true
        },
      }),
    )
    fireEvent.click(screen.getByRole('button', { name: '拒绝' }))
    await waitFor(() => expect(resolved).toHaveLength(1))
    expect(resolved[0]).toEqual([approvalTicket, 'deny'])
  })

  test('the surface locks while submitting and re-opens when delivery fails', async () => {
    let release: (value: boolean) => void = () => {}
    const pending = new Promise<boolean>((resolve) => {
      release = resolve
    })
    render(
      React.createElement(RuntimeApprovalPrompt, {
        approval: approvalRecord() as any,
        onResolve: () => pending,
      }),
    )

    const allow = screen.getByRole('button', { name: '只允许这一次' })
    const deny = screen.getByRole('button', { name: '拒绝' })
    fireEvent.click(allow)

    // While the decision is undetermined the user cannot submit a second one.
    await waitFor(() => expect(allow.hasAttribute('disabled')).toBe(true))
    expect(deny.hasAttribute('disabled')).toBe(true)
    fireEvent.click(deny)
    fireEvent.click(allow)

    // Delivery failed: the approval is still pending, so the surface re-opens.
    release(false)
    await waitFor(() => expect(allow.hasAttribute('disabled')).toBe(false))
    expect(deny.hasAttribute('disabled')).toBe(false)
  })
})
