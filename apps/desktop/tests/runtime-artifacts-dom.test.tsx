import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
import type { ArtifactRecord } from '../src/features/conversations/model/types'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { cleanup, fireEvent, render, screen, within } = await import('@testing-library/react')
const { RuntimeArtifacts, artifactVerificationLabel } = await import('../src/features/chat/runtime-artifacts')
const { finalArtifactIds } = await import('../src/features/chat/final-artifact-references')

afterEach(cleanup)

function artifact(id: string, artifactClass: string, displayName: string): ArtifactRecord {
  return {
    id, conversationId: 'conversation-1', runId: 'run-1', displayName,
    artifactType: 'created_file', artifactClass, artifactOrigin: 'project',
    storagePath: `C:/project/${displayName}`, mediaType: null, byteSize: 1024,
    sha256: null, status: 'ready', createdAt: 1, updatedAt: 1,
  }
}

// Since 4b54e68/0607664 a reply's delivered files are exactly the ones the model declared
// through `present_files` (a non-null `declaredAt`). `artifactClass`, the reply text and the
// file extension no longer decide delivery; every undeclared file is a process file behind
// the collapsed "过程文件" disclosure, and the whole area is the "文件结果" region.
function declared(record: ArtifactRecord, declaredAt = 1, declaredDescription: string | null = null): ArtifactRecord {
  return { ...record, declaredAt, declaredDescription }
}

describe('reply file results', () => {
  test('a declared private PDF is delivered while intermediate files stay collapsed, without changing Host facts', () => {
    const pdf = declared({ ...artifact('compute-artifact:pdf', 'process', 'AGV报告.pdf'), artifactOrigin: 'host_private' })
    const artifacts = [pdf, ...[1, 2, 3, 4].map(i => artifact(`chart-${i}`, 'process', `chart-${i}.svg`))]
    let opened: ArtifactRecord | undefined
    render(React.createElement(RuntimeArtifacts, { artifacts, onOpenArtifact: item => { opened = item } }))
    const results = screen.getByRole('region', { name: '文件结果' })
    const delivered = within(results).getByRole('region', { name: '交付文件' })
    const card = within(delivered).getByRole('button', { name: /AGV报告.pdf/ })
    expect(card.getAttribute('title')).toBe(pdf.storagePath)
    expect(card.textContent).toContain('Fox 内部文件')
    expect(screen.queryByRole('button', { name: /chart-1.svg/ })).toBeNull()
    expect(screen.getByRole('button', { name: /过程文件/ }).textContent).toContain('4')
    fireEvent.click(card)
    expect(opened).toBe(pdf)
    expect(opened?.artifactClass).toBe('process')
  })

  test('legacy delivery section resolves unique exact names, not missing files, drafts or ambiguous names', () => {
    const records = [artifact('pdf', 'process', '报告.pdf'), artifact('csv', 'process', '数据.csv'), artifact('draft', 'process', '草稿.pdf')]
    const text = '## 交付物\n- **报告.pdf**\n- `数据.csv`\n- `不存在.csv`\n- 未生成 **草稿.pdf**\n## 核心结论\n**草稿.pdf**'
    expect([...finalArtifactIds(text, records)]).toEqual(['pdf', 'csv'])
    expect([...finalArtifactIds(text, [...records, artifact('duplicate', 'process', '报告.pdf')])]).toEqual(['csv'])
    expect([...finalArtifactIds('报告.pdf\n[失效](#fox-artifact=missing)\n[错误](#fox-artifact=%ZZ)', records)]).toEqual([])
    expect([...finalArtifactIds('```md\n[示例](#fox-artifact=pdf)\n```', records)]).toEqual([])
  })
  test('larger batches show declared files in declaration order, keep process disclosures, and opening keeps Host identity', () => {
    const opened: string[] = []
    render(React.createElement(RuntimeArtifacts, {
      artifacts: [
        declared(artifact('appendix', 'deliverable', '附录.xlsx'), 20),
        declared(artifact('deliverable', 'deliverable', '报告.docx'), 10),
        artifact('preview', 'preview', '预览.html'),
        artifact('process', 'process', '计算.csv'),
        artifact('process-2', 'process', '日志.txt'),
      ],
      onOpenArtifact: (item: ArtifactRecord) => opened.push(item.id),
    }))

    const process = screen.getByRole('button', { name: /过程文件/ })
    expect(process.getAttribute('aria-expanded')).toBe('false')
    expect(screen.getByRole('region', { name: '文件结果' })).toBeTruthy()
    const delivered = screen.getByRole('region', { name: '交付文件' })
    expect(within(delivered).getAllByRole('button').map(button => button.textContent)).toEqual([
      expect.stringContaining('报告.docx'), expect.stringContaining('附录.xlsx'),
    ])
    expect(screen.queryByRole('button', { name: /计算.csv/ })).toBeNull()
    fireEvent.click(screen.getByRole('button', { name: /报告.docx/ }))
    expect(opened).toEqual(['deliverable'])

    fireEvent.click(process)
    const panel = screen.getByRole('region', { name: '过程文件' })
    expect(panel.id).toBe(process.getAttribute('aria-controls'))
    expect(process.getAttribute('aria-expanded')).toBe('true')
    expect(screen.getByRole('button', { name: /报告.docx/ })).toBeTruthy()
    expect(within(panel).getByRole('button', { name: /预览.html/ })).toBeTruthy()
    expect(within(panel).getByRole('button', { name: /计算.csv/ })).toBeTruthy()
    expect(within(panel).queryByRole('button', { name: /报告.docx/ })).toBeNull()

    fireEvent.click(process)
    expect(screen.queryByRole('region', { name: '过程文件' })).toBeNull()
    expect(process.getAttribute('aria-expanded')).toBe('false')
  })

  test('one process file sits behind the disclosure and no empty delivery category is rendered', () => {
    const record = artifact('process', 'process', '计算.csv')
    let opened: ArtifactRecord | undefined
    const view = render(React.createElement(RuntimeArtifacts, { artifacts: [record], onOpenArtifact: item => { opened = item } }))
    expect(screen.getByRole('region', { name: '文件结果' })).toBeTruthy()
    expect(screen.queryByRole('region', { name: '交付文件' })).toBeNull()
    expect(screen.queryByText('交付文件')).toBeNull()
    const process = screen.getByRole('button', { name: /过程文件/ })
    expect(process.getAttribute('aria-expanded')).toBe('false')
    expect(screen.queryByRole('button', { name: /计算.csv/ })).toBeNull()
    fireEvent.click(process)
    expect(screen.getByRole('region', { name: '过程文件' })).toBeTruthy()
    expect(view.container.querySelector('[data-slot="file-root"]')).not.toBeNull()
    expect(view.container.querySelector('[data-slot="file-icon"] svg')).not.toBeNull()
    expect(view.container.querySelector('[data-slot="file-name"]')?.textContent).toBe('计算.csv')
    expect(view.container.querySelector('[data-slot="file-size"]')?.textContent).toBe('1.0 KB')
    fireEvent.click(screen.getByRole('button', { name: /计算.csv/ }))
    expect(opened).toBe(record)
  })

  test('no files renders no result area at all', () => {
    const view = render(React.createElement(RuntimeArtifacts, { artifacts: [] }))
    expect(view.container.innerHTML).toBe('')
    expect(screen.queryByRole('region', { name: '文件结果' })).toBeNull()
  })

  test('a card shows the file name and keeps the long storage path for hover only', () => {
    const longPath = '\\\\?\\D:\\python\\projects\\Fox\\output\\fox-merge-verify-20261004\\human-tests-20261007-r2\\O06-project\\out\\slides_outline.json'
    const record = declared({ ...artifact('deliverable', 'deliverable', 'slides_outline.json'), storagePath: longPath })
    render(React.createElement(RuntimeArtifacts, { artifacts: [record] }))
    const card = screen.getByRole('button', { name: /slides_outline\.json/ })
    // The card itself paints the name and the summary, never the raw path.
    expect(card.textContent).toContain('slides_outline.json')
    expect(card.textContent).not.toContain('python\\projects')
    expect(card.textContent).not.toContain('human-tests-20261007-r2')
    // The directory is still one hover away.
    const titled = [card.getAttribute('title'), ...Array.from(card.querySelectorAll<HTMLElement>('[title]')).map((element) => element.getAttribute('title'))]
    expect(titled).toContain(longPath)
  })

  test('the compact declared row shows filename, size and the model note, never delivery metadata', () => {
    const published = declared({
      ...artifact('deliverable', 'deliverable', 'slides_outline.json'),
      byteSize: 4012,
      delivery: {
        versionId: 'version-1', versionNo: 3, sourceToolCallId: 'call-1', toolName: 'write_file',
        verificationStatus: 'passed', summary: '验收通过：结构与指标均已核对', targetPath: 'out/slides_outline.json',
      },
    } as ArtifactRecord, 1, '幻灯片大纲')
    render(React.createElement(RuntimeArtifacts, { artifacts: [published] }))
    const card = screen.getByRole('button', { name: /slides_outline\.json/ })
    expect(card.getAttribute('title')).toBe(published.storagePath)
    expect(card.querySelector('.fox-message-artifact-description')?.textContent).toBe('幻灯片大纲')
    expect(card.querySelector('[data-slot="file-size"]')?.textContent).toBe('3.9 KB')
    expect(card.querySelector('[data-slot="file-name"]')?.parentElement).toBe(card.querySelector('[data-slot="file-size"]')?.parentElement)
    for (const leaked of ['4012', 'v3', '验收通过', '已发布到项目', '新建', '验收']) {
      expect(card.textContent).not.toContain(leaked)
    }
  })

  test('a failed verification is left to the detail view: the card shows no verdict', () => {
    const failed = declared({
      ...artifact('deliverable', 'deliverable', 'metrics.csv'),
      delivery: {
        versionId: 'version-2', versionNo: 1, sourceToolCallId: 'call-2', toolName: 'write_file',
        verificationStatus: 'failed', summary: '逐项核对失败：列名与契约不一致（metric,key,value）',
        targetPath: 'metrics.csv',
      },
    } as ArtifactRecord)
    render(React.createElement(RuntimeArtifacts, { artifacts: [failed] }))
    const card = screen.getByRole('button', { name: /metrics\.csv/ })
    for (const leaked of ['内容需核对', '发布未通过', '列名与契约不一致']) {
      expect(card.textContent).not.toContain(leaked)
      expect(card.getAttribute('title')).not.toContain(leaked)
    }
    expect(card.textContent).toContain('项目文件')
    // The detail panel still names the verdict, and says checking content, not publishing.
    expect(artifactVerificationLabel('failed')).toBe('内容需核对')
    expect(artifactVerificationLabel('declared')).toBe('交付文件')
  })

  test('the present_files declaration, not artifactClass, separates delivered files from process files', () => {
    render(React.createElement(RuntimeArtifacts, {
      artifacts: [
        declared(artifact('requested-csv', 'process', '交付数据.csv')),
        artifact('intermediate-docx', 'deliverable', '草稿.docx'),
        // The same file recorded again as an undeclared row is not shown twice.
        { ...artifact('requested-csv-copy', 'process', '交付数据.csv') },
      ],
    }))
    expect(within(screen.getByRole('region', { name: '交付文件' })).getByRole('button', { name: /交付数据.csv/ })).toBeTruthy()
    fireEvent.click(screen.getByRole('button', { name: /过程文件/ }))
    const panel = screen.getByRole('region', { name: '过程文件' })
    expect(within(panel).getByRole('button', { name: /草稿.docx/ })).toBeTruthy()
    expect(within(panel).queryByRole('button', { name: /交付数据.csv/ })).toBeNull()
    expect(screen.getAllByRole('button', { name: /交付数据.csv/ })).toHaveLength(1)
  })

  test('a long filename keeps its full tooltip and shares a column with the independent size row', () => {
    const name='审批验证_保留完整中文文件名_approval_execution_integration_repeated_checks_20261011.py'
    const record=declared({...artifact('long-name','process',name),byteSize:127})
    let opened: ArtifactRecord | undefined
    const view=render(React.createElement(RuntimeArtifacts,{artifacts:[record],onOpenArtifact: item=>{opened=item}}))
    const card=screen.getByRole('button',{name:new RegExp(name)})
    const filename=card.querySelector('[data-slot="file-name"]')!
    const size=card.querySelector('[data-slot="file-size"]')!
    expect(filename.textContent).toBe(name)
    expect(filename.getAttribute('title')).toBe(name)
    expect(size.textContent).toBe('127 B')
    expect(filename.parentElement).toBe(size.parentElement)
    expect(view.container.querySelector('.aui-file-open-icon')).not.toBeNull()
    fireEvent.click(card)
    expect(opened).toBe(record)
  })
})
