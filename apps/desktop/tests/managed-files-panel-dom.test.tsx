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
import { clearMocks, mockIPC } from '@tauri-apps/api/mocks'
import type { ComponentProps } from 'react'
import type { Companion } from '../src/features/companions/api'
import type { ConversationDetail } from '../src/features/conversations/model/types'

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
let listImpl = async (_conversationId: string) => versions
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
    listManagedFileVersions: async (conversationId: string) => {
      listCalls += 1
      if (listError) throw listError
      return listImpl(conversationId)
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
const { act, cleanup, fireEvent, render, screen, waitFor, within } = await import(
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
const { ContactFiles } = await import('../src/features/companions/files')
// The forced restore asks through the in-app confirmation; the stub answers it.
const { setConfirmDialogPresenter } = await import('../src/components/ui/confirm-dialog')
let confirmAnswer = true
let confirmPending: Promise<boolean> | null = null
const confirmTitles: string[] = []
setConfirmDialogPresenter(async (options) => {
  confirmTitles.push(options.title)
  return confirmPending ?? confirmAnswer
})

afterEach(() => {
  cleanup()
  clearMocks()
  listError = null
  listCalls = 0
  listImpl = async () => versions
  restoreCalls.length = 0
  settledByRequestId.clear()
  restoreImpl = async () => ({ id: 'v-restored', versionNo: 9 })
  versions = [
    versionRow({ id: 'v2', versionNo: 2, createdAt: 2, currentHash: 'hash-2' }),
    versionRow({ id: 'v1', versionNo: 1, createdAt: 1 }),
  ]
  confirmAnswer = true
  confirmPending = null
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
  test('两个界面首次只读一次，读取中收到多次刷新时只补读一次', async () => {
    const first = Promise.withResolvers<unknown[]>()
    listImpl = async () => listCalls === 1 ? first.promise : versions
    const conversationId = nextConvId()
    const view = render(<><Harness conversationId={conversationId} refreshKey="start" /><Harness conversationId={conversationId} refreshKey="start" /></>)
    await waitFor(() => expect(listCalls).toBe(1))
    view.rerender(<><Harness conversationId={conversationId} refreshKey="end" /><Harness conversationId={conversationId} refreshKey="end" /></>)
    expect(listCalls).toBe(1)
    await act(async () => first.resolve([]))
    expect(screen.getAllByRole('button', { name: '2 个版本' })).toHaveLength(2)
    expect(listCalls).toBe(2)
  })

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
    act(() => screen.getByRole('button', { name: '版本记录范围' }).focus())
    expect((await screen.findByRole('tooltip')).textContent).toContain('不在版本记录内')
  })
})

describe('版本行恢复按钮（真实 DOM 点击）', () => {
  test('恢复完成后补读已经开始的旧读取，并在此期间禁用所有版本的恢复', async () => {
    versions.push(versionRow({ id: 'v0', versionNo: 0, createdAt: 0 }))
    const oldRows = [...versions]
    const staleRead = Promise.withResolvers<unknown[]>()
    listImpl = async () => listCalls === 2 ? staleRead.promise : versions
    restoreImpl = async () => {
      const restored = versionRow({ id: 'v3', versionNo: 3, createdAt: 3, changeKind: 'restored', currentHash: 'hash-1' })
      versions = [restored, ...versions]
      return restored
    }
    render(<Harness conversationId={nextConvId()} />)
    fireEvent.click(await screen.findByRole('button', { name: '3 个版本' }))
    fireEvent.click(screen.getByRole('button', { name: '重新检查文件版本' }))
    await waitFor(() => expect(listCalls).toBe(2))
    fireEvent.click(screen.getAllByRole('button', { name: '恢复' })[0])
    await waitFor(() => expect(restoreCalls).toHaveLength(1))
    expect(screen.getAllByRole<HTMLButtonElement>('button', { name: '恢复' }).every(button => button.disabled)).toBe(true)
    await act(async () => staleRead.resolve(oldRows))
    expect(screen.getByRole('button', { name: '4 个版本' })).toBeDefined()
    expect(listCalls).toBe(3)
  })

  test.each(['切换对话', '卸载界面'])('%s后，已打开的强制恢复确认失效', async (action) => {
    restoreImpl = async () => { throw new Error('a.txt changed outside the recorded task history') }
    const answer = Promise.withResolvers<boolean>()
    confirmPending = answer.promise
    const view = render(<Harness conversationId={nextConvId()} />)
    fireEvent.click(await screen.findByRole('button', { name: '2 个版本' }))
    fireEvent.click(restoreButton())
    fireEvent.click(await screen.findByRole('button', { name: /强制恢复/ }))
    expect(confirmTitles).toHaveLength(1)
    if (action === '卸载界面') {
      view.unmount()
    } else {
      const nextRows = Promise.withResolvers<unknown[]>()
      listImpl = async () => nextRows.promise
      view.rerender(<Harness conversationId={nextConvId()} />)
      expect(screen.getByText('正在读取文件版本…')).toBeDefined()
      expect(screen.queryByRole('button', { name: '2 个版本' })).toBeNull()
      await act(async () => nextRows.resolve([]))
    }
    await act(async () => answer.resolve(true))
    expect(restoreCalls).toHaveLength(1)
  })

  test('强制确认返回时，另一个恢复还在进行就不再提交', async () => {
    versions.push(versionRow({ id: 'v0', versionNo: 0, createdAt: 0 }))
    restoreImpl = async () => { throw new Error('a.txt changed outside the recorded task history') }
    const answer = Promise.withResolvers<boolean>()
    confirmPending = answer.promise
    render(<Harness conversationId={nextConvId()} />)
    fireEvent.click(await screen.findByRole('button', { name: '3 个版本' }))
    fireEvent.click(screen.getAllByRole('button', { name: '恢复' })[0])
    const force = await screen.findByRole('button', { name: /强制恢复/ })
    fireEvent.click(force)
    fireEvent.click(force)
    expect(confirmTitles).toHaveLength(1)
    const pending = Promise.withResolvers<unknown>()
    restoreImpl = () => pending.promise
    fireEvent.click(restoreButton())
    expect(restoreCalls).toHaveLength(2)
    await act(async () => answer.resolve(true))
    expect(restoreCalls).toHaveLength(2)
    await act(async () => pending.resolve({ id: 'v-restored', versionNo: 3 }))
  })

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

describe('资料库与主对话文件页', () => {
  test('资料库按类别、记录 ID 和规范化路径配对，保留无匹配的版本', async () => {
    const conversationId = nextConvId()
    const contact = { id: 'contact', conversationId, displayName: '联系人' } as Companion
    const at = Date.now()
    versions = [
      versionRow({ id: 'a2', storagePath: '//?/C:/a/report.md', displayName: 'report.md', versionNo: 2, createdAt: at }),
      versionRow({ id: 'a1', storagePath: '//?/C:/a/report.md', displayName: 'report.md', createdAt: at - 1 }),
      versionRow({ id: 'b1', storagePath: 'C:/b/report.md', displayName: 'report.md', createdAt: at - 2 }),
      versionRow({ id: 'c1', storagePath: 'C:/c/report.md', displayName: 'report.md', createdAt: at - 3 }),
    ]
    const file = { name: 'report.md', byteSize: 1, mediaType: null, origin: 'chat', taskName: null, source: null, missing: false }
    mockIPC(() => ({ ok: true, data: {
      provided: [{ ...file, kind: 'attachment', id: 'a', updatedAt: at - 2 }],
      generated: [
        { ...file, kind: 'artifact', id: 'a', updatedAt: at },
        { ...file, kind: 'artifact', id: 'b', updatedAt: at - 1 },
      ],
    } }))
    const detail = {
      attachments: [{ id: 'a', storagePath: 'C:/uploads/report.md' }],
      artifacts: [{ id: 'a', storagePath: 'c:\\a\\REPORT.md' }, { id: 'b', storagePath: 'c:/b/report.md' }],
    } as Pick<ConversationDetail, 'attachments' | 'artifacts'>
    function Library() {
      return <ContactFiles contact={contact} detail={detail} managed={useManagedFiles(conversationId)} />
    }
    const view = render(<Library />)
    await waitFor(() => expect(screen.getAllByLabelText('report.md')).toHaveLength(3))
    const [first, second, upload] = screen.getAllByLabelText('report.md')
    expect(within(first).getByRole('button', { name: '2 个版本' })).toBeDefined()
    expect(within(second).getByRole('button', { name: '1 个版本' })).toBeDefined()
    expect(within(upload).queryByRole('button', { name: /个版本/ })).toBeNull()
    const unmatched = view.container.querySelector<HTMLElement>('.fox-managed-row')!
    expect(unmatched.title).toBe('C:/c/report.md')
    expect(within(unmatched).getByRole('button', { name: '1 个版本' })).toBeDefined()
  })

  test('消息文件读取失败仍可查看版本，并直接重试失败的恢复', async () => {
    const conversationId = nextConvId()
    const contact = { id: 'contact', conversationId, displayName: '联系人' } as Companion
    mockIPC(() => ({ ok: false, error: { code: 'list', message: '消息文件读取失败', retryable: true } }))
    restoreImpl = async () => { throw new Error('恢复请求超时') }
    function Library() {
      return <ContactFiles contact={contact} managed={useManagedFiles(conversationId)} />
    }
    render(<Library />)
    await screen.findByText('消息文件读取失败')
    fireEvent.click(await screen.findByRole('button', { name: '2 个版本' }))
    fireEvent.click(restoreButton())
    await screen.findByText('恢复请求超时')
    restoreImpl = async () => ({ id: 'v-restored', versionNo: 3 })
    fireEvent.click(screen.getByRole('button', { name: '重试恢复' }))
    await waitFor(() => expect(screen.queryByText('恢复请求超时')).toBeNull())
    expect(restoreCalls).toHaveLength(2)
    expect(restoreCalls[0].requestId).toBe(restoreCalls[1].requestId)
    expect(restoreCalls[1].force).toBe(false)
  })

  test('文件页始终显示版本，预览遮住列表时保留未读，返回列表后清除', async () => {
    const { RightPanel } = await import('../src/features/chat/right-panel/fox-right-panel')
    const { TooltipProvider } = await import('../src/components/ui/tooltip')
    const id = nextConvId()
    let projectError = false
    mockIPC((command) => {
      if (command === 'project_files_list' && !projectError) return { ok: true, data: [] }
      return { ok: false, error: { code: 'project', message: '项目文件读取失败', retryable: true } }
    })
    const props = {
      mode: 'files' as const, tabs: ['files' as const], width: 320, activity: {} as never,
      fileTabs: [], activeFileTabId: null, activeChildAgent: null,
      onMode: () => {}, onCloseMode: () => {}, onOpenFile: () => {}, onActivateFile: () => {},
      onCloseFile: () => {}, onOpenChildAgent: () => {}, onBackChildAgent: () => {},
      onToggleMaximized: () => {}, onCollapse: () => {},
    }
    const detail = {
      conversation: { id, projectRoot: null }, artifacts: [], toolCalls: [],
      knowledgeBindings: [], childRuns: [], messages: [], lastRun: { id: 'run', status: 'running' },
    } as ConversationDetail
    const panel = (overrides: Partial<ComponentProps<typeof RightPanel>>) => (
      <TooltipProvider><RightPanel {...props} detail={detail} {...overrides} /></TooltipProvider>
    )
    const view = render(panel({}))
    await screen.findByRole('button', { name: '2 个版本' })
    expect(screen.getByText('尚未选择项目')).toBeDefined()

    const project = { ...detail, conversation: { ...detail.conversation, projectRoot: 'C:/project' } }
    view.rerender(panel({ detail: project }))
    fireEvent.change(screen.getByPlaceholderText('搜索文件'), { target: { value: 'none' } })
    await screen.findByText('没有匹配的文件')
    expect(screen.getByRole('button', { name: '2 个版本' })).toBeDefined()

    const preview = { fileTabs: [{ id: 'a', path: 'a.txt', name: 'a.txt', artifactId: null }], activeFileTabId: 'a' }
    view.rerender(panel({ ...preview, detail: project }))
    versions = [versionRow({ id: 'v3', versionNo: 3, createdAt: 3 }), ...versions]
    const completed = { ...project, lastRun: { ...project.lastRun!, status: 'completed' as const } }
    view.rerender(panel({ ...preview, detail: completed }))
    await waitFor(() => expect(screen.getByRole('button', { name: '打开页面' }).querySelector('.fox-tab-dot')).not.toBeNull())
    expect(screen.queryByRole('button', { name: '3 个版本' })).toBeNull()

    projectError = true
    const changedRoot = { ...completed, conversation: { ...completed.conversation, projectRoot: 'C:/changed' } }
    view.rerender(panel({ detail: changedRoot }))
    await screen.findByText('项目文件读取失败')
    expect(screen.getByRole('button', { name: '3 个版本' })).toBeDefined()
    expect(view.container.querySelector('.fox-tab-dot')).toBeNull()
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
