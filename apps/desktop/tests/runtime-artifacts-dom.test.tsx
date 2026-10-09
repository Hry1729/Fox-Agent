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

  test('a card shows the file name and keeps the long storage path for hover only', () => {
    const longPath = '\\\\?\\D:\\python\\projects\\Fox\\output\\fox-merge-verify-20261004\\human-tests-20261007-r2\\O06-project\\out\\slides_outline.json'
    const record = { ...artifact('deliverable', 'deliverable', 'slides_outline.json'), storagePath: longPath }
    render(React.createElement(RuntimeArtifacts, { artifacts: [record] }))
    fireEvent.click(screen.getByRole('button', { name: /本次文件结果/ }))
    const card = screen.getByRole('button', { name: /slides_outline\.json/ })
    // The card itself paints the name and the summary, never the raw path.
    expect(card.textContent).toContain('slides_outline.json')
    expect(card.textContent).not.toContain('python\\projects')
    expect(card.textContent).not.toContain('human-tests-20261007-r2')
    // The directory is still one hover away.
    const titled = Array.from(card.querySelectorAll<HTMLElement>('[title]'))
      .map((element) => element.getAttribute('title'))
    expect(titled).toContain(longPath)
  })

  test('a card carries one short summary and leaves the metadata to the detail view', () => {
    const published = {
      ...artifact('deliverable', 'deliverable', 'slides_outline.json'),
      byteSize: 4012,
      delivery: {
        versionId: 'version-1', versionNo: 3, sourceToolCallId: 'call-1', toolName: 'write_file',
        verificationStatus: 'passed', summary: '验收通过：结构与指标均已核对', targetPath: 'out/slides_outline.json',
      },
    }
    render(React.createElement(RuntimeArtifacts, { artifacts: [published] }))
    fireEvent.click(screen.getByRole('button', { name: /本次文件结果/ }))
    const card = screen.getByRole('button', { name: /slides_outline\.json/ })
    expect(card.textContent).toContain('已发布到项目')
    // Size, version, change kind and the verification tally are diagnostics: they
    // belong to the file detail view, not to this one line.
    for (const leaked of ['4.0 KB', '4012', 'v3', '验收通过', '新建', '验收']) {
      expect(card.textContent).not.toContain(leaked)
    }
  })

  test('a failed verification says the content needs checking, not that publishing failed', () => {
    const failed = {
      ...artifact('deliverable', 'deliverable', 'metrics.csv'),
      delivery: {
        versionId: 'version-2', versionNo: 1, sourceToolCallId: 'call-2', toolName: 'write_file',
        verificationStatus: 'failed', summary: '交付核验失败：列名与契约不一致（metric,key,value）',
        targetPath: 'metrics.csv',
      },
    }
    render(React.createElement(RuntimeArtifacts, { artifacts: [failed] }))
    fireEvent.click(screen.getByRole('button', { name: /本次文件结果/ }))
    const card = screen.getByRole('button', { name: /metrics\.csv/ })
    // A published file whose *content* failed verification: the base phrase keeps
    // the publish fact, and the warning describes the verification, not the write.
    expect(card.textContent).toContain('已发布到项目')
    expect(card.textContent).toContain('内容需核对')
    expect(card.textContent).not.toContain('发布未通过')
    expect(card.textContent).not.toContain('列名与契约不一致')
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
