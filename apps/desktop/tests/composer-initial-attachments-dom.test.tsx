import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { PromptInput, usePromptInputAttachments } = await import('../src/components/ai-elements/prompt-input')
const { ComposerInitialAttachmentBridge } = await import('../src/features/chat/composer/composer-initial-attachments')
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
