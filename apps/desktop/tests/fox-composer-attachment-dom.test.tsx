// 5D acceptance: the assistant-ui composer's attachment pipeline.
//
// SCOPE (honest): the component under test is the REAL `FoxThreadComposer`
// glue — the hidden file input, `aui.composer.addAttachment`, the attachment
// chips and their removal — mounted over the REAL assistant-ui external store
// runtime with an attachments adapter that mirrors the shipped contract in
// `use-fox-runtime.ts` (size/count gates, data: URL read, file parts on send).
// The Tauri conversation runtime behind `useFoxRuntime` is replaced by the
// external store adapter; the Host send boundary is asserted through `onNew`.

import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { act, cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const { AssistantRuntimeProvider, useExternalStoreRuntime } = await import('@assistant-ui/react')
const { FoxThreadComposer } = await import('../src/features/chat/thread/composer/fox-thread-composer')
const { TooltipProvider } = await import('../src/components/ui/tooltip')

afterEach(() => {
  cleanup()
  window.localStorage.clear()
})

const dataUrl = 'data:text/plain;base64,QUJD'

const settled = () => act(async () => {
  await new Promise((resolve) => setTimeout(resolve, 0))
})

type PendingLike = { id: string; name: string; file: File; contentType: string }
type AppendMessageLike = {
  content: Array<{ type: string; filename?: string }>
  attachments?: Array<{ name?: string; filename?: string }>
}

/** Mirrors the shipped adapter contract from use-fox-runtime.ts. */
function makeAttachmentsAdapter() {
  const removed: string[] = []
  const read: File[] = []
  return {
    removed,
    adapter: {
      accept: '.txt,.md,.pdf',
      add: async ({ file }: { file: File }): Promise<PendingLike> => {
        if (file.size > 5 * 1024 * 1024) throw new Error('单个附件不能超过 5 MB')
        return { id: `att-${file.name}`, name: file.name, file, contentType: file.type || 'application/octet-stream' }
      },
      remove: async (attachment: PendingLike) => {
        removed.push(attachment.id)
      },
      send: async (attachment: PendingLike) => {
        read.push(attachment.file)
        return {
          id: attachment.id,
          type: 'file',
          name: attachment.name,
          contentType: attachment.contentType,
          file: attachment.file,
          status: { type: 'complete' },
          content: [{ type: 'file', filename: attachment.name, mimeType: attachment.contentType, data: dataUrl, sourceType: 'url' as const }],
        }
      },
    },
  }
}

function Harness({ adapter, onSend }: { adapter: ReturnType<typeof makeAttachmentsAdapter>['adapter']; onSend: (message: AppendMessageLike) => void }) {
  const [draft, setDraft] = React.useState('')
  const runtime = useExternalStoreRuntime({
    isRunning: false,
    messages: [],
    onNew: (message: AppendMessageLike) => onSend(message),
    adapters: { attachments: adapter },
  })
  return React.createElement(
    AssistantRuntimeProvider,
    { runtime },
    React.createElement(TooltipProvider, null, React.createElement(FoxThreadComposer, {
      runtimeControlled: false,
      draft,
      setDraft,
      resetKey: 0,
    })),
  )
}

function mount() {
  const sent: AppendMessageLike[] = []
  const store = makeAttachmentsAdapter()
  render(React.createElement(Harness, { adapter: store.adapter, onSend: (message) => sent.push(message) }))
  const textarea = screen.getByRole('textbox') as HTMLTextAreaElement
  // The hidden input is the composer's single file entry point.
  const input = document.querySelector('input[type="file"]') as HTMLInputElement
  return { sent, textarea, input, removed: store.removed }
}

function textFile(name: string, size = 8) {
  const file = new File(['x'.repeat(size)], name, { type: 'text/plain' })
  Object.defineProperty(file, 'size', { value: size })
  return file
}

describe('the assistant-ui composer attachment pipeline', () => {
  test('a picked file becomes a removable attachment chip', async () => {
    const { input, removed } = mount()
    await settled()
    fireEvent.change(input, { target: { files: [textFile('说明.txt')] } })
    await waitFor(() => expect(screen.getByTitle('说明.txt')).not.toBeNull())
    expect(screen.getByTitle('说明.txt').textContent).toContain('说明.txt')

    // Removing the chip removes the attachment from the composer. The removal
    // is asserted directly (waitFor's failure path never times out in this
    // environment, so a broken removal would hang instead of failing).
    fireEvent.click(screen.getByTitle('说明.txt'))
    await settled()
    await settled()
    expect(screen.queryByTitle('说明.txt')).toBeNull()
    expect(removed).toEqual(['att-说明.txt'])
  })

  test('sending carries the attachments and the draft text, then clears the composer', async () => {
    const { sent, textarea, input } = mount()
    fireEvent.change(input, { target: { files: [textFile('报告.pdf', 12)] } })
    await waitFor(() => expect(screen.queryByTitle('报告.pdf')).not.toBeNull())
    fireEvent.change(textarea, { target: { value: '看一下这份报告' } })
    await waitFor(() => expect(textarea.value).toBe('看一下这份报告'))
    fireEvent.keyDown(textarea, { key: 'Enter' })
    await waitFor(() => expect(sent).toHaveLength(1))
    const message = sent[0]
    // The shipped send path reads `message.attachments` (see use-fox-runtime),
    // where each entry carries the completed file part.
    const attachment = (message.attachments ?? []).find((item) => item.name === '报告.pdf')
    expect(attachment).not.toBeUndefined()
    expect((attachment?.content ?? []).some((part) => part.type === 'file' && part.filename === '报告.pdf')).toBe(true)
    expect(message.content.some((part) => part.type === 'text' && part.text === '看一下这份报告')).toBe(true)
    // The composer releases the accepted draft and the consumed attachment.
    await waitFor(() => expect(textarea.value).toBe(''))
    await waitFor(() => expect(screen.queryByTitle('报告.pdf')).toBeNull())
  })
})
