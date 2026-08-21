import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'

test('chat uses the FoxOps user avatar and resolves each history row agent avatar', async () => {
  const source = await readFile(
    new URL('../src/views/chat/agent/index.vue', import.meta.url),
    'utf8'
  )
  assert.match(source, /foxops-avatar\.png/)
  assert.match(source, /usableMediaSrc\(userAvatar\.value\) \|\| defaultUserAvatar/)
  assert.match(source, /:src="getThreadAgentAvatar\(thread\)"/)
  assert.match(source, /const getThreadAgentAvatar/)
  assert.match(source, /getThreadAgent\(thread\)\?\.icon/)
  assert.doesNotMatch(source, /:src="agentAvatarSrc">\{\{ \(thread\.title/)
})

test('chat send failure can restore pending attachments outside the try block', async () => {
  const source = await readFile(
    new URL('../src/views/chat/agent/index.vue', import.meta.url),
    'utf8'
  )
  assert.match(source, /let threadId = currentThreadId\.value\s+let pendingSnapshot = \[\]\s+try/)
  assert.match(source, /pendingSnapshot = Array\.from\(pendingSet\)/)
  assert.doesNotMatch(source, /const pendingSnapshot = Array\.from\(pendingSet\)/)
})
