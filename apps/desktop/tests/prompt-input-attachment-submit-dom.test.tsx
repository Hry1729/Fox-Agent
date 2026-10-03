import { afterEach, beforeEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { cleanup, fireEvent, render, screen, waitFor } = await import('@testing-library/react')
const {
  PromptInput,
  PromptInputTextarea,
  usePromptInputAttachments,
} = await import('../src/components/ai-elements/prompt-input')

type Message = { text: string; files: Array<{ filename?: string; url?: string }> }
type ErrorCode = 'attachment_read_failed' | 'attachment_send_failed' | string

let readMode: 'success' | 'failure' = 'success'
let readBlobs: Blob[] = []
const dataUrl = 'data:text/plain;base64,QUJD'
const originalFileReader = Object.getOwnPropertyDescriptor(globalThis, 'FileReader')
const originalCreateObjectURL = Object.getOwnPropertyDescriptor(URL, 'createObjectURL')
const originalRevokeObjectURL = Object.getOwnPropertyDescriptor(URL, 'revokeObjectURL')

class ReaderStub {
  result: string | null = null
  onload: (() => void) | null = null
  onerror: (() => void) | null = null
  onabort: (() => void) | null = null

  readAsDataURL(blob: Blob) {
    readBlobs.push(blob)
    queueMicrotask(() => {
      if (readMode === 'failure') this.onerror?.()
      else {
        this.result = dataUrl
        this.onload?.()
      }
    })
  }
}

function restoreDescriptor(object: object, key: string, descriptor?: PropertyDescriptor) {
  if (descriptor) Object.defineProperty(object, key, descriptor)
  else Reflect.deleteProperty(object, key)
}

beforeEach(() => {
  readMode = 'success'
  readBlobs = []
  Object.defineProperty(globalThis, 'FileReader', { configurable: true, value: ReaderStub })
  Object.defineProperty(URL, 'createObjectURL', { configurable: true, value: () => 'blob:preview-only' })
  Object.defineProperty(URL, 'revokeObjectURL', { configurable: true, value: () => undefined })
})

afterEach(() => {
  cleanup()
  restoreDescriptor(globalThis, 'FileReader', originalFileReader)
  restoreDescriptor(URL, 'createObjectURL', originalCreateObjectURL)
  restoreDescriptor(URL, 'revokeObjectURL', originalRevokeObjectURL)
  window.localStorage.clear()
})

function AttachmentNames() {
  const { files } = usePromptInputAttachments()
  return React.createElement('output', { 'data-testid': 'attachments' }, files.map((file) => file.filename).join('|'))
}

function Composer({
  onSubmit,
  onSubmitStart,
  onError,
}: {
  onSubmit: (message: Message) => boolean | Promise<boolean>
  onSubmitStart?: (message: Message) => void
  onError?: (error: { code: ErrorCode }) => void
}) {
  const [draft, setDraft] = React.useState('附件说明')
  return React.createElement(
    PromptInput,
    {
      onSubmit: async (message: Message) => {
        const outcome = await onSubmit(message)
        // Workbench owns this controlled draft and clears it after acceptance.
        if (outcome !== false) setDraft('')
        return outcome
      },
      onSubmitStart,
      onError,
    },
    React.createElement(PromptInputTextarea, {
      value: draft,
      'aria-label': 'prompt',
      onChange: (event: React.ChangeEvent<HTMLTextAreaElement>) => setDraft(event.currentTarget.value),
    }),
    React.createElement(AttachmentNames),
    React.createElement('button', { type: 'submit' }, 'send'),
  )
}

function addFile() {
  const file = new File(['ABC'], 'notes.txt', { type: 'text/plain' })
  fireEvent.change(screen.getByLabelText('Upload files'), { target: { files: [file] } })
  expect(screen.getByTestId('attachments').textContent).toBe('notes.txt')
  return file
}

function sendWithEnter() {
  fireEvent.keyDown(screen.getByLabelText('prompt'), { key: 'Enter' })
}

describe('prompt attachment submit', () => {
  test('reads the original File before starting a send and passes data: content', async () => {
    const events: string[] = []
    const submitted: Message[] = []
    render(React.createElement(Composer, {
      onSubmitStart: () => events.push('start'),
      onSubmit: (message: Message) => {
        events.push('submit')
        submitted.push(message)
        return true
      },
    }))
    const file = addFile()
    sendWithEnter()

    await waitFor(() => expect(submitted).toHaveLength(1))
    expect(readBlobs).toEqual([file])
    expect(events).toEqual(['start', 'submit'])
    expect(submitted[0]).toEqual({
      text: '附件说明',
      files: [{ type: 'file', filename: 'notes.txt', mediaType: 'text/plain', url: dataUrl }],
    })
    await waitFor(() => expect(screen.getByTestId('attachments').textContent).toBe(''))
    expect((screen.getByLabelText('prompt') as HTMLTextAreaElement).value).toBe('')
  })

  test('a failed FileReader preserves the draft and attachment without starting a send', async () => {
    readMode = 'failure'
    const started: Message[] = []
    const submitted: Message[] = []
    const errors: ErrorCode[] = []
    render(React.createElement(Composer, {
      onSubmitStart: (message: Message) => started.push(message),
      onSubmit: (message: Message) => { submitted.push(message); return true },
      onError: (error: { code: ErrorCode }) => errors.push(error.code),
    }))
    const file = addFile()
    sendWithEnter()

    await waitFor(() => expect(errors).toEqual(['attachment_read_failed']))
    expect(readBlobs).toEqual([file])
    expect(started).toEqual([])
    expect(submitted).toEqual([])
    expect((screen.getByLabelText('prompt') as HTMLTextAreaElement).value).toBe('附件说明')
    expect(screen.getByTestId('attachments').textContent).toBe('notes.txt')
  })

  test('onSubmit false keeps the same draft and File available for a retry', async () => {
    const submitted: Message[] = []
    const errors: ErrorCode[] = []
    render(React.createElement(Composer, {
      onSubmit: async (message: Message) => {
        submitted.push(message)
        return submitted.length > 1
      },
      onError: (error: { code: ErrorCode }) => errors.push(error.code),
    }))
    const file = addFile()
    sendWithEnter()

    await waitFor(() => expect(errors).toEqual(['attachment_send_failed']))
    expect(submitted).toHaveLength(1)
    expect(submitted[0]?.files[0]?.url).toBe(dataUrl)
    expect((screen.getByLabelText('prompt') as HTMLTextAreaElement).value).toBe('附件说明')
    expect(screen.getByTestId('attachments').textContent).toBe('notes.txt')

    sendWithEnter()
    await waitFor(() => expect(submitted).toHaveLength(2))
    expect(readBlobs).toEqual([file, file])
    await waitFor(() => expect(screen.getByTestId('attachments').textContent).toBe(''))
    expect((screen.getByLabelText('prompt') as HTMLTextAreaElement).value).toBe('')
  })
})
