import { expect, test } from 'bun:test'
import {
  steeringStatusCopy,
  steeringStripView,
} from '../src/features/chat/components/steering-presentation'
import { managedFileVersionPresentation } from '../src/features/chat/components/managed-file-presentation'

const row = (messageId: string, status: string, content = '检查 Sheet2') => ({
  messageId,
  content,
  status,
})

test('an active Run offers the editor and shows every state distinctly', () => {
  const view = steeringStripView({
    active: true,
    runId: 'run-1',
    runState: 'running',
    acceptsSteering: true,
    rows: [row('a', 'received'), row('b', 'delivered'), row('c', 'applied')],
  })
  expect(view.visible).toBe(true)
  expect(view.editable).toBe(true)
  expect(view.historyOnly).toBe(false)
  expect(view.label).toBe('运行中补充要求')
  expect(view.pending).toBe(2)
  expect(steeringStatusCopy('received').label).toBe('已接收')
  expect(steeringStatusCopy('delivered').label).toBe('已投递')
  expect(steeringStatusCopy('applied').label).toBe('已应用')
  expect(steeringStatusCopy('cancelled').label).toBe('未应用')
})

test('a finished Run keeps its four-state history visible but cannot be steered', () => {
  for (const state of ['completed', 'failed', 'cancelled']) {
    const view = steeringStripView({
      active: false,
      runId: 'run-1',
      runState: state,
      acceptsSteering: false,
      rows: [row('a', 'applied'), row('b', 'cancelled')],
    })
    // History survives the end of the task and a reopen: the records are shown.
    expect(view.visible).toBe(true)
    // …but no new input may be accepted by a Run that is over.
    expect(view.editable).toBe(false)
    expect(view.historyOnly).toBe(true)
    expect(view.label).toContain('已结束')
    expect(view.hint).toContain(state)
    expect(view.pending).toBe(0)
  }
})

test('the strip stays hidden when the conversation has no records at all', () => {
  const view = steeringStripView({
    active: false,
    runId: null,
    runState: null,
    acceptsSteering: false,
    rows: [],
  })
  expect(view.visible).toBe(false)
  expect(view.editable).toBe(false)
  expect(view.historyOnly).toBe(false)
})

test('an active Run with no records yet is still not shown until a request exists', () => {
  const view = steeringStripView({
    active: true,
    runId: 'run-1',
    runState: 'running',
    acceptsSteering: true,
    rows: [],
  })
  expect(view.visible).toBe(false)
})

test('outstanding bytes count UTF-8 bytes, so Chinese text is measured like the Host measures it', () => {
  const view = steeringStripView({
    active: true,
    runId: 'run-1',
    runState: 'running',
    acceptsSteering: true,
    // Each Chinese character is three UTF-8 bytes; 8000 of them are 24 000 bytes.
    rows: [row('a', 'received', '中'.repeat(8_000))],
  })
  expect(view.outstanding).toBe(24_000)
})

// --- R1: what one managed-file version row promises -------------------------

const version = (over: Partial<Parameters<typeof managedFileVersionPresentation>[0]> = {}) => ({
  id: 'file-version:1',
  versionNo: 1,
  changeKind: 'modified',
  tool: 'write_file',
  beforeSize: 10,
  afterSize: 20,
  restoreSize: 20,
  canRestore: true,
  restoreBlocker: null,
  sourceKind: 'host_capture',
  drifted: false,
  backupAvailable: true,
  ...over,
})

test('a version advertises the size of the content a restore would write', () => {
  // The pre-write backup is 10 bytes, but this row records 20 bytes of content:
  // the label must describe the row's own content, not the content it replaced.
  const view = managedFileVersionPresentation(version())
  expect(view.sizeLabel).toBe(20)
  expect(view.restorable).toBe(true)
  expect(view.blocker).toBeNull()
  expect(view.current).toBe(false)
})

test('a created version is restorable, and a legacy row says so explicitly', () => {
  const created = managedFileVersionPresentation(
    version({ changeKind: 'created', beforeSize: null, backupAvailable: false, sourceKind: 'host_capture' }),
  )
  expect(created.restorable).toBe(true)
  expect(created.blocker).toBeNull()

  const legacy = managedFileVersionPresentation(
    version({
      canRestore: false,
      restoreBlocker: '该版本记录于内容快照启用之前，其自身内容无法验证，不能用于恢复（记录保持不变）',
      sourceKind: 'legacy_unknown',
    }),
  )
  expect(legacy.restorable).toBe(false)
  expect(legacy.blocker).toContain('无法验证')
  // An unavailable version is never silently hidden.
  expect(legacy.visible).toBe(true)
})

test('the latest recorded version is marked current, and a drifted file is not', () => {
  expect(managedFileVersionPresentation(version(), { isLatest: true }).current).toBe(true)
  const drifted = managedFileVersionPresentation(version(), { isLatest: true, drifted: true })
  expect(drifted.current).toBe(false)
  expect(drifted.forceRequired).toBe(true)
})

// --- N6: content a restore overwrote is a version of its own ----------------

test('the content replaced by a restore is selectable like any other version', () => {
  // A `replaced` row carries its own content (the bytes that were overwritten),
  // so the panel must offer it for restore instead of hiding it.
  const replaced = managedFileVersionPresentation(
    version({
      id: 'file-version:9',
      versionNo: 9,
      changeKind: 'replaced',
      tool: 'restore',
      afterSize: 21,
      restoreSize: 21,
      sourceKind: 'restore',
      // Its sibling `restored` row is the latest one, so this row is not current.
    }),
  )
  expect(replaced.visible).toBe(true)
  expect(replaced.restorable).toBe(true)
  expect(replaced.sizeLabel).toBe(21)
  expect(replaced.blocker).toBeNull()
  // It is not the file's current content, so restoring it needs no force.
  expect(replaced.current).toBe(false)
  expect(replaced.forceRequired).toBe(false)
})
