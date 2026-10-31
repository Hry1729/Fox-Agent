// sandbox.md 13.26 #5 原来在输入框上方铺红框「发送未开始 / 原因：code」。
// 现在这条原因只进通知：普通模式详情不带错误码，开发者模式才带上。
import { afterEach, describe, expect, mock, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'

if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true

const published: Array<{ message: unknown; options?: { description?: unknown; mergeKey?: string } }> = []
const actualNotifications = await import('../src/features/notifications')
mock.module('../src/features/notifications', () => ({
  ...actualNotifications,
  notify: Object.assign((message: unknown, options?: { description?: unknown; mergeKey?: string }) => {
    published.push({ message, options })
    return 'id'
  }, {
    error: (message: unknown, options?: { description?: unknown; mergeKey?: string }) => {
      published.push({ message, options })
      return 'id'
    },
    success: () => 'id',
    info: () => 'id',
    warning: () => 'id',
  }),
}))

const React = await import('react')
const { act, cleanup, fireEvent, render } = await import('@testing-library/react')
const { PromptInput, PromptInputTextarea, SEND_NOT_STARTED_MESSAGE } = await import('../src/components/ai-elements/prompt-input')
const { notifySendNotStarted } = await import('../src/features/chat/send-failure-notice')

afterEach(() => {
  cleanup()
  published.length = 0
})

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

  test('发送未开始改为发出通知，普通模式详情不含错误码', () => {
    notifySendNotStarted('run.create_failed', '应为目录但是文件', { developerUi: false })
    expect(published).toHaveLength(1)
    expect(published[0].message).toBe('发送未开始')
    expect(published[0].options?.mergeKey).toBe('send-not-started:run.create_failed')
    expect(String(published[0].options?.description)).not.toContain('run.create_failed')
    expect(String(published[0].options?.description)).toContain('消息没有发出去，草稿已保留')
  })

  test('开发者模式的通知详情带上错误码', () => {
    notifySendNotStarted('run.create_failed', '应为目录但是文件', { developerUi: true })
    expect(published).toHaveLength(1)
    expect(String(published[0].options?.description)).toContain('run.create_failed')
    expect(String(published[0].options?.description)).toContain('应为目录但是文件')
  })
})
