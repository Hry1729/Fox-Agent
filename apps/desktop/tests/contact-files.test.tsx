import { afterEach, describe, expect, test } from 'bun:test'
import { cleanup, fireEvent, render, waitFor, within } from '@testing-library/react'
import { mockIPC, clearMocks } from '@tauri-apps/api/mocks'
import { ContactsContext, type ContactsContextValue } from '../src/features/companions/context'
import { ContactFiles, FILES_WORKDIR_HINT, formatLibraryRelative, libraryMediaKind, libraryTimeGroup } from '../src/features/companions/files'
import { ContactProfile } from '../src/features/companions/profile'
import { formatSearchTime } from '../src/features/companions/chat-search'
import type { Companion, CompanionFileEntry, CompanionFiles } from '../src/features/companions/api'

const contact: Companion = {
  id: 'contact', conversationId: 'chat', templateAgentId: 'expert', displayName: '资料伙伴', avatarRef: '📚',
  duty: '', personaOverride: null, workspaceDir: 'D:/contact', workspaceState: 'ready', workspaceError: null,
  isDefault: false, pinnedAt: null, lastReadOrdinal: 0, modelProviderId: null, modelRef: null,
  unreadCount: 0, activeRunStatus: null, archivedAt: null, deletingAt: null,
}

const now = Date.now()
const entry = (patch: Partial<CompanionFileEntry> = {}): CompanionFileEntry => ({
  kind: 'attachment', id: 'att-1', name: '报价单.pdf', updatedAt: now - 60_000, byteSize: 1024,
  mediaType: 'application/pdf', origin: 'chat', taskName: null,
  source: { bubbleId: 'msg-1', messageId: 'msg-1', at: now - 60_000 }, missing: false,
  ...patch,
})
const files = (patch: Partial<CompanionFiles> = {}): CompanionFiles => ({
  provided: [entry()],
  generated: [
    entry({ kind: 'artifact', id: 'art-1', name: '分析结果.md', updatedAt: now - 30_000, mediaType: 'text/markdown', source: { bubbleId: 'run-1:response:0', messageId: 'msg-2', at: now - 30_000 } }),
    entry({ kind: 'artifact', id: 'art-2', name: '周报.docx', origin: 'task', taskName: '每周周报', updatedAt: now - 20_000, source: { bubbleId: 'dc-report-run-2', messageId: 'dc-report-run-2', at: now - 20_000 } }),
    entry({ kind: 'artifact', id: 'art-3', name: '丢失的产物.txt', updatedAt: now - 10_000, source: null, missing: true }),
  ],
  ...patch,
})

const context = (current: Companion, jumpToMessage?: (bubbleId: string) => void): ContactsContextValue => ({
  current, privateChat: true, list: { companions: [current], pendingDeletions: [] }, projects: [], model: '全局模型', permission: 'ask', error: '',
  refresh: async () => {}, refreshConversation: async () => {}, open: () => {}, add: () => {}, showProfile: () => {},
  leave: () => {}, setPermission: () => {}, openSharedMemory: () => {}, askToOrganize: () => {},
  locateRequest: null, jumpToMessage,
})

interface Harness { gets: number; calls: Record<string, unknown>[] }
function mockFiles(data: CompanionFiles = files(), harness: Harness = { gets: 0, calls: [] }) {
  mockIPC((command, args) => {
    if (command === 'companion_files_list') { harness.gets += 1; return { ok: true, data } }
    if (command === 'companion_file_open' || command === 'companion_file_save' || command === 'project_folder_open') {
      harness.calls.push({ command, ...(args as Record<string, unknown>) }); return { ok: true, data: null }
    }
    throw new Error('Unexpected command: ' + command)
  })
  return harness
}

const renderFiles = (jumpToMessage?: (bubbleId: string) => void) =>
  render(<ContactsContext.Provider value={context(contact, jumpToMessage)}><ContactFiles contact={contact} /></ContactsContext.Provider>)

const openMenu = (row: HTMLElement) => fireEvent.click(within(row).getByLabelText(`${row.getAttribute('aria-label')} 的操作`))

afterEach(() => { cleanup(); clearMocks() })

describe('联系人资料库', () => {
  test('悬浮卡片与菜单保留提供/生成区分、来源与任务名', async () => {
    const harness = mockFiles()
    const view = renderFiles()
    await waitFor(() => expect(harness.gets).toBe(1))
    const provided = view.getByLabelText('报价单.pdf')
    expect(within(provided).getByText('我提供的文件')).toBeDefined()
    expect(within(provided).getAllByText(/来源消息/).length).toBeGreaterThan(0)
    const task = view.getByLabelText('周报.docx')
    expect(within(task).getByText('联系人生成的结果')).toBeDefined()
    expect(within(task).getAllByText(/定时任务「每周周报」/).length).toBeGreaterThan(0)
    expect(within(task).getAllByText(/来源消息/).length).toBeGreaterThan(0)
    expect(view.container.querySelector('section[aria-label="今天"]')).not.toBeNull()
    expect(view.getByText(FILES_WORKDIR_HINT)).toBeDefined()
  })

  test('缺失文件写明状态并禁用打开与另存，无来源消息禁用回到消息', async () => {
    mockFiles()
    const view = renderFiles()
    const missing = await waitFor(() => view.getByLabelText('丢失的产物.txt'))
    expect(within(missing).getByText('文件缺失')).toBeDefined()
    openMenu(missing)
    expect(within(missing).getAllByText(/暂无来源消息/).length).toBeGreaterThan(0)
    expect((within(missing).getByText('打开') as HTMLButtonElement).disabled).toBe(true)
    expect((within(missing).getByText('另存为') as HTMLButtonElement).disabled).toBe(true)
    expect((within(missing).getByText('回到消息') as HTMLButtonElement).disabled).toBe(true)
    const ok = view.getByLabelText('报价单.pdf')
    openMenu(ok)
    expect((within(ok).getByText('打开') as HTMLButtonElement).disabled).toBe(false)
  })

  test('打开与另存为带上联系人与文件标识交给后端核验', async () => {
    const harness = mockFiles()
    const view = renderFiles()
    const row = await waitFor(() => view.getByLabelText('分析结果.md'))
    openMenu(row)
    fireEvent.click(within(row).getByText('打开'))
    await waitFor(() => expect(harness.calls).toHaveLength(1))
    openMenu(row)
    fireEvent.click(within(row).getByText('另存为'))
    await waitFor(() => expect(harness.calls).toHaveLength(2))
    expect(harness.calls[0]).toEqual({ command: 'companion_file_open', request: { companionId: 'contact', kind: 'artifact', fileId: 'art-1' } })
    expect(harness.calls[1]).toEqual({ command: 'companion_file_save', request: { companionId: 'contact', kind: 'artifact', fileId: 'art-1' } })
  })

  test('回到消息使用稳定气泡标识，含后台任务回报消息', async () => {
    const jumps: string[] = []
    mockFiles()
    const view = renderFiles(id => jumps.push(id))
    const provided = await waitFor(() => view.getByLabelText('报价单.pdf'))
    openMenu(provided)
    fireEvent.click(within(provided).getByText('回到消息'))
    const task = view.getByLabelText('周报.docx')
    openMenu(task)
    fireEvent.click(within(task).getByText('回到消息'))
    expect(jumps).toEqual(['msg-1', 'dc-report-run-2'])
  })

  test('⋯ 菜单保持原生 details，外部点击与 Escape 收起', async () => {
    mockFiles()
    const view = renderFiles()
    const row = await waitFor(() => view.getByLabelText('报价单.pdf'))
    const menu = row.querySelector('details')!
    expect(menu).not.toBeNull()
    menu.open = true
    fireEvent.pointerDown(document.body)
    expect(menu.open).toBe(false)
    menu.open = true
    fireEvent.keyDown(document, { key: 'Escape' })
    expect(menu.open).toBe(false)
  })

  test('筛选 pills 区分媒体与文件，选中态互斥', async () => {
    mockFiles(files({ provided: [entry(), entry({ id: 'att-2', name: '截图.png', mediaType: 'image/png' })] }))
    const view = renderFiles()
    await waitFor(() => view.getByLabelText('截图.png'))
    const media = view.getByRole('button', { name: '媒体' })
    const docs = view.getByRole('button', { name: '文件' })
    const all = view.getByRole('button', { name: '全部' })
    expect(all.getAttribute('aria-pressed')).toBe('true')
    fireEvent.click(media)
    expect(media.getAttribute('aria-pressed')).toBe('true')
    expect(all.getAttribute('aria-pressed')).toBe('false')
    expect(view.getByLabelText('截图.png')).toBeDefined()
    expect(view.queryByLabelText('报价单.pdf')).toBeNull()
    fireEvent.click(docs)
    expect(view.getByLabelText('报价单.pdf')).toBeDefined()
    expect(view.queryByLabelText('截图.png')).toBeNull()
    fireEvent.click(all)
    expect(view.getByLabelText('截图.png')).toBeDefined()
    expect(view.getByLabelText('报价单.pdf')).toBeDefined()
  })

  test('按今天/更早分组，超过五条折叠并可展开收起', async () => {
    const generated = Array.from({ length: 7 }, (_, index) => entry({
      kind: 'artifact', id: `art-t${index}`, name: `结果${index}.md`, mediaType: 'text/markdown', updatedAt: now - index * 60_000,
    }))
    generated.push(entry({ kind: 'artifact', id: 'art-old', name: '旧结果.md', mediaType: 'text/markdown', updatedAt: now - 40 * 86_400_000 }))
    mockFiles(files({ provided: [], generated }))
    const view = renderFiles()
    const today = await waitFor(() => {
      const section = view.container.querySelector('section[aria-label="今天"]')
      if (!section) throw new Error('等待今天分组')
      return section as HTMLElement
    })
    const earlier = view.container.querySelector('section[aria-label="更早"]') as HTMLElement
    expect(today.querySelectorAll('.fox-library-row')).toHaveLength(5)
    expect(earlier.querySelectorAll('.fox-library-row')).toHaveLength(1)
    expect(earlier.querySelector('.fox-library-more')).toBeNull()
    const more = within(today).getByText('展开')
    expect(more.getAttribute('aria-expanded')).toBe('false')
    fireEvent.click(more)
    expect(today.querySelectorAll('.fox-library-row')).toHaveLength(7)
    const collapse = within(today).getByText('收起')
    expect(collapse.getAttribute('aria-expanded')).toBe('true')
    fireEvent.click(collapse)
    expect(today.querySelectorAll('.fox-library-row')).toHaveLength(5)
  })

  test('悬浮卡片展示分组、类别、完整文件名与联系人来源', async () => {
    const name = '生产安全培训系统_开发报价_修订版.pdf'
    mockFiles(files({ provided: [entry({ name })] }))
    const view = renderFiles()
    const row = await waitFor(() => view.getByLabelText(name))
    const hover = row.querySelector('.fox-library-hover') as HTMLElement
    expect(hover.textContent).toContain('今天')
    expect(hover.textContent).toContain('我提供的文件')
    expect(hover.textContent).toContain(name)
    expect(hover.textContent).toContain('资料伙伴')
    expect(hover.textContent).toContain(formatSearchTime(now - 60_000))
    expect(hover.querySelector('.fox-contact-avatar')).not.toBeNull()
  })

  test('类型图标按扩展名着色，媒体显示色块', async () => {
    mockFiles(files({ provided: [entry(), entry({ id: 'att-2', name: '说明.docx', mediaType: 'application/vnd.openxmlformats-officedocument.wordprocessingml.document' }), entry({ id: 'att-3', name: '截图.png', mediaType: 'image/png' })] }))
    const view = renderFiles()
    const pdf = await waitFor(() => view.getByLabelText('报价单.pdf'))
    const more = view.queryByText('展开')
    if (more) fireEvent.click(more)
    expect(pdf.querySelector('.fox-library-glyph rect')?.getAttribute('fill')).toBe('#e5484d')
    const docx = view.getByLabelText('说明.docx')
    expect(docx.querySelector('.fox-library-glyph rect')?.getAttribute('fill')).toBe('#3e63dd')
    const png = view.getByLabelText('截图.png')
    expect(png.querySelector('.fox-library-glyph.is-media.is-image')).not.toBeNull()
    const md = view.getByLabelText('分析结果.md')
    expect(md.querySelector('.fox-library-glyph rect')).toBeNull()
    expect(md.querySelector('.fox-library-line')).not.toBeNull()
  })

  test('空列表给出两类空态提示', async () => {
    mockFiles(files({ provided: [], generated: [] }))
    const view = renderFiles()
    await waitFor(() => expect(view.getByText('还没有通过消息提供过文件。')).toBeDefined())
    expect(view.getByText('还没有生成过文件。')).toBeDefined()
  })

  test('筛选无结果时给出筛选空态', async () => {
    mockFiles()
    const view = renderFiles()
    await waitFor(() => view.getByLabelText('报价单.pdf'))
    fireEvent.click(view.getByRole('button', { name: '媒体' }))
    expect(view.getByText('当前筛选下没有文件。')).toBeDefined()
  })

  test('读取失败显示原因并可重试', async () => {
    let fail = true
    const harness = { gets: 0, calls: [] }
    mockIPC(command => {
      if (command === 'companion_files_list') {
        harness.gets += 1
        return fail ? { ok: false, error: { code: 'companion.file_list_failed', message: '读取失败', retryable: true } } : { ok: true, data: files() }
      }
      throw new Error('Unexpected command: ' + command)
    })
    const view = renderFiles()
    await waitFor(() => expect(view.getByRole('alert').textContent).toContain('读取失败'))
    fail = false
    fireEvent.click(view.getByText('重试'))
    await waitFor(() => expect(view.getByLabelText('报价单.pdf')).toBeDefined())
    expect(harness.gets).toBe(2)
  })

  test('打开工作目录走既有入口', async () => {
    const harness = mockFiles()
    const view = renderFiles()
    fireEvent.click(await waitFor(() => view.getByText('打开工作目录')))
    await waitFor(() => expect(harness.calls).toHaveLength(1))
    expect(harness.calls[0]).toEqual({ command: 'project_folder_open', request: { conversationId: 'chat' } })
  })

  test('资料页入口打开资料库，并可返回资料', async () => {
    const harness: Harness = { gets: 0, calls: [] }
    mockIPC(command => {
      if (command === 'companion_template_status') return { ok: true, data: { latestHash: null, boundSnapshot: null, latestSnapshot: null, updateAvailable: false, dismissed: false } }
      if (command === 'model_providers_list') return { ok: true, data: [] }
      if (command === 'companion_files_list') { harness.gets += 1; return { ok: true, data: files() } }
      throw new Error('Unexpected command: ' + command)
    })
    const view = render(<ContactsContext.Provider value={context(contact)}>
      <ContactProfile contact={contact} />
    </ContactsContext.Provider>)
    fireEvent.click(await waitFor(() => view.getByText('查看')))
    await waitFor(() => expect(harness.gets).toBe(1))
    await waitFor(() => expect(view.getByLabelText('报价单.pdf')).toBeDefined())
    expect(view.getByText('资料库')).toBeDefined()
    fireEvent.click(view.getByText('返回资料'))
    await waitFor(() => expect(view.getByText('查看')).toBeDefined())
    expect(view.queryByLabelText('报价单.pdf')).toBeNull()
  })
})

describe('资料库分组与相对时间', () => {
  const fixed = new Date(2026, 10, 11, 15, 0, 0).getTime()
  test('今天/本周/更早按本地周一周界划分', () => {
    expect(libraryTimeGroup(new Date(2026, 10, 11, 0, 30).getTime(), fixed)).toBe('today')
    expect(libraryTimeGroup(new Date(2026, 10, 10, 23, 59).getTime(), fixed)).toBe('week')
    expect(libraryTimeGroup(new Date(2026, 10, 9, 0, 0).getTime(), fixed)).toBe('week')
    expect(libraryTimeGroup(new Date(2026, 10, 8, 23, 59).getTime(), fixed)).toBe('earlier')
  })
  test('相对时间从刚刚到分钟、小时、天，再退回日期', () => {
    expect(formatLibraryRelative(fixed - 30_000, fixed)).toBe('刚刚')
    expect(formatLibraryRelative(fixed - 5 * 60_000, fixed)).toBe('5 分钟')
    expect(formatLibraryRelative(fixed - 2 * 3_600_000, fixed)).toBe('2 小时')
    expect(formatLibraryRelative(fixed - 3 * 86_400_000, fixed)).toBe('3 天')
    expect(formatLibraryRelative(fixed - 40 * 86_400_000, fixed)).toBe(formatSearchTime(fixed - 40 * 86_400_000, fixed))
  })
  test('媒体判定优先 MIME，缺失时回退扩展名', () => {
    expect(libraryMediaKind(entry({ mediaType: 'image/png', name: '无扩展名' }))).toBe('image')
    expect(libraryMediaKind(entry({ mediaType: null, name: '片段.MP4' }))).toBe('video')
    expect(libraryMediaKind(entry({ mediaType: null, name: '录音.m4a' }))).toBe('audio')
    expect(libraryMediaKind(entry({ mediaType: 'application/pdf', name: '报价单.pdf' }))).toBeNull()
  })
})