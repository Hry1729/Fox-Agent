import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { PromptInput, usePromptInputAttachments } = await import('../src/components/ai-elements/prompt-input')
const { ComposerInitialAttachmentBridge } = await import('../src/features/chat/composer/composer-initial-attachments')
const { useCompanionDrafts } = await import('../src/features/chat/composer/use-companion-drafts')
import type { ComposerInitialAttachments } from '../src/features/chat/composer/composer-initial-attachments'

afterEach(cleanup)

function PendingFiles() {
  const attachments = usePromptInputAttachments()
  return <><output data-testid="files">{attachments.files.map((item) => item.filename).join('|')}</output>{attachments.files.map((item) => <button key={item.id} type="button" onClick={() => attachments.remove(item.id)}>移除 {item.filename}</button>)}</>
}

function Harness({ request, onConsumed }: { request: ComposerInitialAttachments | null; onConsumed: (id: number) => void }) {
  return <PromptInput maxFiles={8} maxFileSize={5 * 1024 * 1024} onSubmit={() => { throw new Error('must not send automatically') }}><ComposerInitialAttachmentBridge request={request} onConsumed={onConsumed} /><PendingFiles /></PromptInput>
}

describe('initial library attachments use the real composer file collection', () => {
  test('adds the original File once and does not revive a removed attachment on rerender', async () => {
    const request = { requestId: 7, files: [new File(['ABC'], '制度.md', { type: 'text/markdown' })] }
    const consumed: number[] = []
    const onConsumed = (id: number) => consumed.push(id)
    const view = render(<Harness request={request} onConsumed={onConsumed} />)
    await waitFor(() => expect(screen.getByTestId('files').textContent).toBe('制度.md'))
    expect(consumed).toEqual([7])
    fireEvent.click(screen.getByRole('button', { name: '移除 制度.md' }))
    view.rerender(<Harness request={{ ...request, files: [...request.files] }} onConsumed={onConsumed} />)
    expect(screen.getByTestId('files').textContent).toBe('')
    expect(consumed).toEqual([7])
  })

  test('consumes the parent request so a later composer mount starts empty', async () => {
    function Parent() {
      const [request, setRequest] = React.useState<ComposerInitialAttachments | null>({ requestId: 9, files: [new File(['ABC'], '制度.md')] })
      const [key, setKey] = React.useState(0)
      return <><button onClick={() => setKey((value) => value + 1)}>换个输入框</button><Harness key={key} request={request} onConsumed={() => setRequest(null)} /></>
    }
    render(<Parent />)
    await waitFor(() => expect(screen.getByTestId('files').textContent).toBe('制度.md'))
    fireEvent.click(screen.getByRole('button', { name: '换个输入框' }))
    expect(screen.getByTestId('files').textContent).toBe('')
  })
})

test('伙伴切换与设置返回各自恢复未发送的文字和附件', async () => {
  function Workspace() {
    const [selected, setSelected] = React.useState('A')
    const [settings, setSettings] = React.useState(false)
    const draft = useCompanionDrafts(selected, !settings)
    return <>
      <button onClick={() => setSelected('A')}>伙伴 A</button>
      <button onClick={() => setSelected('B')}>伙伴 B</button>
      <button onClick={() => setSettings(value => !value)}>设置</button>
      {!settings && <PromptInput key={selected} onSubmit={() => { throw new Error('drafts must not send') }}>
        <ComposerInitialAttachmentBridge request={draft.restoredAttachments} onFilesChange={draft.setFiles} />
        <textarea aria-label="草稿" value={draft.text} onChange={event => draft.setText(event.target.value)} />
        <PendingFiles />
      </PromptInput>}
    </>
  }
  const view = render(<Workspace />)
  const show = (label: string) => fireEvent.click(screen.getByRole('button', { name: label }))
  const type = (value: string) => fireEvent.change(screen.getByRole('textbox', { name: '草稿' }), { target: { value } })
  const attach = (name: string) => fireEvent.change(view.container.querySelector('input[type="file"]')!, {
    target: { files: [new File(['value\n1'], name, { type: 'text/csv' })] },
  })
  const text = () => (screen.getByRole('textbox', { name: '草稿' }) as HTMLTextAreaElement).value
  const files = () => screen.getByTestId('files').textContent
  type('汇总 A 的订单')
  attach('A.csv')
  await waitFor(() => expect(files()).toBe('A.csv'))
  show('伙伴 B')
  expect(text()).toBe('')
  expect(files()).toBe('')
  type('核对 B 的价格')
  attach('B.csv')
  show('伙伴 A')
  await waitFor(() => expect(files()).toBe('A.csv'))
  expect(text()).toBe('汇总 A 的订单')
  show('设置')
  expect(screen.queryByTestId('files')).toBeNull()
  show('设置')
  await waitFor(() => expect(files()).toBe('A.csv'))
  expect(text()).toBe('汇总 A 的订单')
  show('伙伴 B')
  await waitFor(() => expect(files()).toBe('B.csv'))
  expect(text()).toBe('核对 B 的价格')
})
