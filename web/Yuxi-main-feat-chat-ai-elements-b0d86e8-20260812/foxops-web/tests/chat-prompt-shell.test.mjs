import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')

test('ElementsPromptShell 已接线到对话页并使用 PromptInput', () => {
  const shell = readFileSync(join(root, 'src/components/chat/ElementsPromptShell.vue'), 'utf8')
  assert.match(shell, /PromptInput/)
  assert.match(shell, /ai-elements\/prompt-input/)

  const page = readFileSync(join(root, 'src/views/chat/agent/index.vue'), 'utf8')
  assert.match(page, /ElementsPromptShell/)
  assert.match(page, /MessageInputComponent/)
})

test('Conversation 外壳把高度约束传给 StickToBottom', () => {
  const conv = readFileSync(
    join(root, 'src/components/ai-elements/conversation/Conversation.vue'),
    'utf8'
  )
  assert.match(conv, /chat-conversation-root/)
  assert.match(conv, /min-h-0/)
  assert.match(conv, /overflow-y:\s*auto/)
})

test('消息列表容器始终 flex 以打通滚动高度链', () => {
  const page = readFileSync(join(root, 'src/views/chat/agent/index.vue'), 'utf8')
  assert.match(page, /flex-1 min-h-0 flex flex-col overflow-hidden/)
  assert.doesNotMatch(
    page,
    /bubbleListItems\.length && !isReplyLoading && !isLoadingMessages \? 'flex flex-col'/
  )
})
