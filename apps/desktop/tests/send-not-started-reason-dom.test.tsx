// sandbox.md 13.26 #5: when the Host refuses to start a run (e.g.
// run.create_failed naming "<路径> 应为目录但是文件"), the composer shows
// 「发送未开始」 with a one-line, truncated reason that expands to the full text.
import { afterEach, describe, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const React = await import('react')
const { act, cleanup, fireEvent, render } = await import('@testing-library/react')
const { PromptInput, PromptInputTextarea, SEND_NOT_STARTED_MESSAGE } = await import('../src/components/ai-elements/prompt-input')
const { SendNotStartedNotice, SEND_FAILURE_SUMMARY_CHARS, truncateReason } = await import('../src/features/chat/composer/send-not-started-notice')

afterEach(() => cleanup())

const reason = 'run.create_failed: C:\\Users\\u\\AppData\\Roaming\\com.fox.desktop\\runtime-sessions\\kernel-run-locks 应为目录但是文件（数据库 C:\\Users\\u\\AppData\\Roaming\\com.fox.desktop\\fox.db：unable to open database file）'

describe('发送未开始 reason', () => {
  test('PromptInput passes the host reason through onError.detail', async () => {
    const errors: Array<{ code: string; message: string; detail?: string }> = []
    const { container } = render(React.createElement(PromptInput, {
      onSubmit: () => ({ sent: false as const, reason }),
      onError: (error: any) => errors.push(error),
    }, React.createElement(PromptInputTextarea, null)))
    const textarea = container.querySelector('textarea') as HTMLTextAreaElement
    await act(async () => { fireEvent.change(textarea, { target: { value: '你好' } }) })
    await act(async () => { fireEvent.submit(container.querySelector('form')!) ; await new Promise((resolve) => setTimeout(resolve, 20)) })
    expect(errors).toHaveLength(1)
    expect(errors[0]).toEqual({ code: 'attachment_send_failed', message: SEND_NOT_STARTED_MESSAGE, detail: reason })
    // The draft is kept for a retry.
    expect(textarea.value).toBe('你好')
  })

  test('a plain false keeps the old message without a detail', async () => {
    const errors: any[] = []
    const { container } = render(React.createElement(PromptInput, { onSubmit: () => false, onError: (error: any) => errors.push(error) }, React.createElement(PromptInputTextarea, null)))
    await act(async () => { fireEvent.change(container.querySelector('textarea')!, { target: { value: 'x' } }) })
    await act(async () => { fireEvent.submit(container.querySelector('form')!); await new Promise((resolve) => setTimeout(resolve, 20)) })
    expect(errors[0]).toEqual({ code: 'attachment_send_failed', message: SEND_NOT_STARTED_MESSAGE })
  })

  test('the notice truncates the summary and expands to the full reason', () => {
    const { getByTestId } = render(React.createElement(SendNotStartedNotice, { message: '发送未开始', detail: reason }))
    const notice = getByTestId('fox-send-not-started')
    expect(notice.textContent).toContain('发送未开始')
    const details = notice.querySelector('details')!
    expect(details.open).toBe(false)
    const summary = details.querySelector('summary')!
    expect(summary.textContent).toBe(`原因：${truncateReason(reason, SEND_FAILURE_SUMMARY_CHARS)}`)
    expect(summary.textContent!.length).toBeLessThanOrEqual(SEND_FAILURE_SUMMARY_CHARS + 4)
    expect(summary.textContent!.endsWith('…')).toBe(true)
    expect(details.querySelector('pre')!.textContent).toBe(reason)
  })

  test('without a reason only the message is shown', () => {
    const { getByTestId } = render(React.createElement(SendNotStartedNotice, { message: '发送未开始' }))
    expect(getByTestId('fox-send-not-started').querySelector('details')).toBeNull()
  })
})
