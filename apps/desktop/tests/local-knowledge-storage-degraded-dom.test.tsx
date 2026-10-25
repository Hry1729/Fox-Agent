// sandbox.md 13.26 #4: when startup could not open the knowledge storage for
// writing, the storage dialog names the path (「知识库路径不可写：<路径>」) and
// offers to record another writable folder (or the default) for the next start
// instead of the migration, which cannot run against a read-only database.
import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { act, cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const { StorageLocationDialog } = await import('../src/features/local-knowledge/local-knowledge-pages')
const { mapLocalKnowledgeStorageStatus } = await import('../src/features/local-knowledge/tauri-gateway')

afterEach(() => cleanup())

const reason = '知识库路径不可写：D:\\ReadOnly\\knowledge'
const baseStatus = { rootPath: 'D:\\ReadOnly\\knowledge', databasePath: 'D:\\ReadOnly\\knowledge\\knowledge.db', writable: false, schemaVersion: 7, pendingRootPath: null, restartRequired: false, migrationId: null }

function gateway(degraded: boolean, resets: Array<string | null>, migrations: string[]) {
  return {
    getStorageStatus: async () => ({ ...baseStatus, degradedReason: degraded ? reason : null }),
    pickStorageDirectory: async () => 'E:\\Fox\\knowledge',
    resetStoragePath: async (path: string | null) => { resets.push(path); return { ...baseStatus, degradedReason: reason, pendingRootPath: path ?? 'default', restartRequired: true } },
    startStorageMigration: async (path: string) => { migrations.push(path); return { operationId: 'op', acceptedAt: new Date().toISOString() } },
    getStorageMigration: async () => { throw new Error('unused') },
  } as any
}
const button = (name: string) => {
  const found = [...document.querySelectorAll('button')].find((item) => item.textContent?.trim() === name)
  if (!found) throw new Error(`no button ${name}`)
  return found as HTMLButtonElement
}

describe('degraded knowledge storage', () => {
  test('the DTO mapping keeps the degraded reason', () => {
    expect(mapLocalKnowledgeStorageStatus({ ...baseStatus, degradedReason: reason } as any).degradedReason).toBe(reason)
    expect(mapLocalKnowledgeStorageStatus({ ...baseStatus } as any).degradedReason).toBeNull()
  })

  test('the dialog names the path and records a new folder for the next start', async () => {
    const resets: Array<string | null> = []
    const migrations: string[] = []
    await act(async () => { render(React.createElement(StorageLocationDialog, { gateway: gateway(true, resets, migrations), open: true, onOpenChange: () => {} })) })
    await waitFor(() => { if (!document.querySelector('[data-testid="fox-kb-storage-degraded"]')) throw new Error('no degraded notice') })
    expect(document.querySelector('[data-testid="fox-kb-storage-degraded"]')!.textContent).toContain(reason)
    expect(() => button('确认迁移')).toThrow()
    expect(button('使用此位置').disabled).toBe(true)
    await act(async () => { fireEvent.click(button('选择文件夹')) })
    await waitFor(() => expect(button('使用此位置').disabled).toBe(false))
    await act(async () => { fireEvent.click(button('使用此位置')) })
    await waitFor(() => expect(resets).toEqual(['E:\\Fox\\knowledge']))
    expect(migrations).toEqual([])
    await waitFor(() => expect(document.body.textContent).toContain('已记录新位置，重启 Fox 后生效。'))
  })

  test('恢复默认位置 resets to the default path', async () => {
    const resets: Array<string | null> = []
    await act(async () => { render(React.createElement(StorageLocationDialog, { gateway: gateway(true, resets, []), open: true, onOpenChange: () => {} })) })
    await waitFor(() => button('恢复默认位置'))
    await act(async () => { fireEvent.click(button('恢复默认位置')) })
    await waitFor(() => expect(resets).toEqual([null]))
  })

  test('a healthy store keeps the migration flow', async () => {
    await act(async () => { render(React.createElement(StorageLocationDialog, { gateway: gateway(false, [], []), open: true, onOpenChange: () => {} })) })
    await waitFor(() => button('确认迁移'))
    expect(document.querySelector('[data-testid="fox-kb-storage-degraded"]')).toBeNull()
    expect(() => button('使用此位置')).toThrow()
  })
})
