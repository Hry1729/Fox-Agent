import test from 'node:test'
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'

// 守护：协议文档必须覆盖工具桥 REQUEST_SCHEMAS 的消息名，以及 pi-runtime 的 session.native_* 错误码。
test('rt2_every_native_protocol_message_is_documented', () => {
  const bridge = readFileSync(new URL('../src/pi-native-tool-bridge.mjs', import.meta.url), 'utf8')
  const runtime = readFileSync(new URL('../src/pi-runtime.mjs', import.meta.url), 'utf8')
  const doc = readFileSync(new URL('../../../docs/04-技术参考/Runtime协议.md', import.meta.url), 'utf8')

  const schemaBlock = bridge.match(/const REQUEST_SCHEMAS = \{([\s\S]*?)\n\}/)
  assert.ok(schemaBlock, 'REQUEST_SCHEMAS block missing')
  const keyIds = [...schemaBlock[1].matchAll(/\[([A-Z0-9_]+)\]/g)].map((match) => match[1])
  assert.ok(keyIds.length > 0, 'REQUEST_SCHEMAS has no keys')
  const messages = keyIds.map((id) => {
    const literal = bridge.match(new RegExp(`(?:export )?const ${id} = '([^']+)'`))
    assert.ok(literal, `REQUEST_SCHEMAS key ${id} has no string literal`)
    return literal[1]
  })

  const codes = [...new Set([...runtime.matchAll(/session\.native_[a-z_]+/g)].map((match) => match[0]))]
  assert.ok(codes.length > 0, 'no session.native_ codes in pi-runtime.mjs')

  for (const name of [...messages, ...codes]) {
    assert.ok(doc.includes(name), `${name} is missing from Runtime协议.md`)
  }
})
