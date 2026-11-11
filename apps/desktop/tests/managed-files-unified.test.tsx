// 统一侧边栏「文件版本」两条不变量的行为测试：
// 1) 分组、指纹、文件配对都来自共享 model（资料库与右侧边栏同一份规则）；
// 2) 未读圆点遵循「首次数据是基线、不变不打扰、变化才点亮、看到即清除」。
// 只测纯函数与 hook，不碰 IPC，也就不需要任何 mock。
import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const { act, renderHook } = await import('@testing-library/react')
const {
  groupManagedFileVersions,
  managedFilesFingerprint,
  managedFilePathKey,
  formatVersionTime,
  managedFileGroupAt,
  toolStatusSignal,
} = await import('../src/features/chat/components/managed-files/model')
const { useManagedFileUnseen } = await import('../src/features/chat/components/managed-files/use-managed-file-unseen')

type Row = Record<string, unknown>
const row = (overrides: Row): Row => ({
  id: 'r', runId: 'run', toolCallId: null, tool: 'write_file',
  storagePath: 'C:/proj/a.md', displayName: 'a.md', versionNo: 1, changeKind: 'modified',
  beforeHash: null, beforeSize: null, afterHash: 'h1', afterSize: 10, backupPath: null,
  restoredFromId: null, createdAt: Date.now(), backupAvailable: false,
  currentHash: 'h1', drifted: false, restoreSize: 10, canRestore: true,
  sourceKind: 'host_capture', restoreBlocker: null, ...overrides,
})
const group = (rows: Row[]) => groupManagedFileVersions(rows.map(item => item as never))

afterEach(() => globalThis.localStorage.clear())

describe('版本分组与配对', () => {
  test('按文件分组、版本倒序、组按最新版本时间倒序', () => {
    const groups = group([
      row({ id: 'b1', storagePath: 'C:/proj/b.md', displayName: 'b.md', versionNo: 1, createdAt: 500 }),
      row({ id: 'a2', storagePath: 'C:/proj/a.md', displayName: 'a.md', versionNo: 2, createdAt: 400 }),
      row({ id: 'a1', storagePath: 'C:/proj/a.md', displayName: 'a.md', versionNo: 1, createdAt: 300 }),
    ])
    expect(groups.map(item => item.displayName)).toEqual(['b.md', 'a.md'])
    expect(groups[1].versions.map(item => item.versionNo)).toEqual([2, 1])
    expect(managedFileGroupAt(groups[1])).toBe(400)
  })

  test('漂移只记在最新行；最新内容哈希消失即判定删除', () => {
    const [drift] = group([row({ drifted: true, currentHash: 'other' })])
    expect(drift.drifted).toBe(true)
    expect(drift.deleted).toBe(false)
    const [gone] = group([row({ drifted: true, currentHash: null })])
    expect(gone.deleted).toBe(true)
    const [clean] = group([row({})])
    expect(clean.drifted).toBe(false)
  })

  test('指纹随版本数或最新变化而变，随无关重排不变', () => {
    const one = group([row({ id: 'a1', versionNo: 1, createdAt: 10 })])
    const same = group([row({ id: 'a1', versionNo: 1, createdAt: 10 })])
    expect(managedFilesFingerprint(one)).toBe(managedFilesFingerprint(same))
    const grown = group([
      row({ id: 'a2', versionNo: 2, createdAt: 20 }),
      row({ id: 'a1', versionNo: 1, createdAt: 10 }),
    ])
    expect(managedFilesFingerprint(grown)).not.toBe(managedFilesFingerprint(one))
    const drift = group([row({ id: 'a1', versionNo: 1, createdAt: 10, drifted: true })])
    expect(managedFilesFingerprint(drift)).not.toBe(managedFilesFingerprint(one))
  })

  test('跨界面配对只用规范化路径：分隔符、大小写、空白、重复斜杠都要归一', () => {
    expect(managedFilePathKey('C:\\Docs\\报价.PDF')).toBe(managedFilePathKey(' c:/docs/报价.pdf '))
    expect(managedFilePathKey('./notes//readme.Md/')).toBe('notes/readme.md')
    // 同名不同路径是两份互不相干的历史，绝不能并成一组。
    expect(managedFilePathKey('C:/a/report.md')).not.toBe(managedFilePathKey('C:/b/report.md'))
  })

  test('版本时间：今天只显示时分，更早带月日', () => {
    const now = new Date(2026, 10, 11, 15, 0, 0).getTime()
    expect(formatVersionTime(new Date(2026, 10, 11, 9, 5).getTime(), now)).toBe('09:05')
    // new Date(2026, 10, 9) 是 11 月 9 日（月份从 0 起）；输出手工拼接，不依赖 locale 分隔符。
    expect(formatVersionTime(new Date(2026, 10, 9, 9, 5).getTime(), now)).toBe('11-09 09:05')
  })

  test('运行信号只在工具状态变化时改变，流式内容不会触发', () => {
    const running = [{ status: 'running' }, { status: 'completed' }]
    expect(toolStatusSignal(running)).toBe(toolStatusSignal([...running]))
    expect(toolStatusSignal(running)).not.toBe(toolStatusSignal([{ status: 'completed' }, { status: 'completed' }]))
    expect(toolStatusSignal([])).toBe('')
    expect(toolStatusSignal(null)).toBe('')
  })
})

describe('未读版本圆点', () => {
  const mount = (conversationId: string, fingerprint: string | null, viewing = false) =>
    renderHook(() => useManagedFileUnseen({ surface: 'files', conversationId, fingerprint, viewing }))

  test('第一次拿到的数据只写基线，不点亮圆点', () => {
    const view = mount('chat-1', 'fp-1')
    expect(view.result.current[0]).toBe(false)
    expect(globalThis.localStorage.getItem('fox:managed-file-seen:files:chat-1')).toBe('fp-1')
  })

  test('指纹变化才点亮；两个界面各自记账互不串线', () => {
    mount('chat-1', 'fp-1')
    const other = mount('chat-1', 'fp-2')
    expect(other.result.current[0]).toBe(true)
    const library = renderHook(() => useManagedFileUnseen({ surface: 'library', conversationId: 'chat-1', fingerprint: 'fp-2', viewing: false }))
    expect(library.result.current[0]).toBe(false)
  })

  test('正在查看列表时指纹落库并熄灭，包括查看中到达的新指纹', () => {
    mount('chat-1', 'fp-1')
    const view = mount('chat-1', 'fp-1')
    act(() => view.rerender())
    expect(view.result.current[0]).toBe(false)
    const open = renderHook(({ fp }) => useManagedFileUnseen({ surface: 'files', conversationId: 'chat-2', fingerprint: fp, viewing: true }), { initialProps: { fp: 'fp-1' as string | null } })
    expect(open.result.current[0]).toBe(false)
    open.rerender({ fp: 'fp-2' })
    expect(open.result.current[0]).toBe(false)
    expect(globalThis.localStorage.getItem('fox:managed-file-seen:files:chat-2')).toBe('fp-2')
  })

  test('换对话重新评估；指纹未知（加载中）不改动已有点亮状态', () => {
    mount('chat-1', 'fp-1')
    const switched = mount('chat-2', 'fp-9')
    expect(switched.result.current[0]).toBe(false)
    expect(globalThis.localStorage.getItem('fox:managed-file-seen:files:chat-2')).toBe('fp-9')
  })
})
