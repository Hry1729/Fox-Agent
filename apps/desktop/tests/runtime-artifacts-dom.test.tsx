import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import type { ArtifactRecord } from '../src/features/conversations/model/types'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { cleanup, fireEvent, render, screen, within } = await import('@testing-library/react')
const { RuntimeArtifacts } = await import('../src/features/chat/runtime-artifacts')

afterEach(cleanup)

function artifact(id: string, artifactClass: string, displayName: string): ArtifactRecord {
  return {
    id, conversationId: 'conversation-1', runId: 'run-1', displayName,
    artifactType: 'created_file', artifactClass, artifactOrigin: 'project',
    storagePath: `C:/project/${displayName}`, mediaType: null, byteSize: 1024,
    sha256: null, status: 'ready', createdAt: 1, updatedAt: 1,
  }
}

describe('reply file results', () => {
  test('two buttons share one row and clicking either shows only its files below', () => {
    const opened: string[] = []
    render(React.createElement(RuntimeArtifacts, {
      artifacts: [
        artifact('deliverable', 'deliverable', '报告.docx'),
        artifact('preview', 'preview', '预览.html'),
        artifact('process', 'process', '计算.csv'),
      ],
      onOpenArtifact: (item: ArtifactRecord) => opened.push(item.id),
    }))

    const result = screen.getByRole('button', { name: /本次文件结果/ })
    const process = screen.getByRole('button', { name: /过程文件/ })
    expect(result.parentElement).toBe(process.parentElement)
    expect(result.getAttribute('aria-expanded')).toBe('false')
    expect(process.getAttribute('aria-expanded')).toBe('false')
    expect(screen.queryByRole('region', { name: '本次文件结果' })).toBeNull()

    fireEvent.click(result)
    expect(result.getAttribute('aria-expanded')).toBe('true')
    expect(screen.getByRole('region', { name: '本次文件结果' }).id).toBe(result.getAttribute('aria-controls'))
    expect(screen.getByRole('button', { name: /报告.docx/ })).toBeTruthy()
    expect(screen.queryByRole('button', { name: /计算.csv/ })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: /报告.docx/ }))
    expect(opened).toEqual(['deliverable'])

    fireEvent.click(process)
    const panel = screen.getByRole('region', { name: '过程文件' })
    expect(panel.id).toBe(process.getAttribute('aria-controls'))
    expect(result.getAttribute('aria-expanded')).toBe('false')
    expect(process.getAttribute('aria-expanded')).toBe('true')
    expect(screen.queryByRole('button', { name: /报告.docx/ })).toBeNull()
    expect(within(panel).getByRole('button', { name: /预览.html/ })).toBeTruthy()
    expect(within(panel).getByRole('button', { name: /计算.csv/ })).toBeTruthy()

    fireEvent.click(process)
    expect(screen.queryByRole('region', { name: '过程文件' })).toBeNull()
    expect(process.getAttribute('aria-expanded')).toBe('false')
  })

  test('an empty category stays visible but cannot open a blank panel', () => {
    render(React.createElement(RuntimeArtifacts, { artifacts: [artifact('process', 'process', '计算.csv')] }))
    const result = screen.getByRole('button', { name: /本次文件结果/ })
    const process = screen.getByRole('button', { name: /过程文件/ })
    expect((result as HTMLButtonElement).disabled).toBe(true)
    fireEvent.click(result)
    expect(screen.queryByRole('region', { name: '本次文件结果' })).toBeNull()
    fireEvent.click(process)
    expect(screen.getByRole('region', { name: '过程文件' })).toBeTruthy()
  })

  test('Host classification keeps a requested CSV deliverable separate from a process DOCX', () => {
    render(React.createElement(RuntimeArtifacts, {
      artifacts: [
        artifact('requested-csv', 'deliverable', '交付数据.csv'),
        artifact('intermediate-docx', 'process', '草稿.docx'),
      ],
    }))
    fireEvent.click(screen.getByRole('button', { name: /本次文件结果/ }))
    expect(screen.getByRole('button', { name: /交付数据.csv/ })).toBeTruthy()
    expect(screen.queryByRole('button', { name: /草稿.docx/ })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: /过程文件/ }))
    expect(screen.getByRole('button', { name: /草稿.docx/ })).toBeTruthy()
    expect(screen.queryByRole('button', { name: /交付数据.csv/ })).toBeNull()
  })
})
