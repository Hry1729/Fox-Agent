import assert from 'node:assert/strict'
import test from 'node:test'
import { isUsableMediaSrc, usableMediaSrc } from '../src/utils/mediaSrc.js'

test('拒绝空值和站点根路径头像', () => {
  assert.equal(isUsableMediaSrc(''), false)
  assert.equal(isUsableMediaSrc('/'), false)
  assert.equal(isUsableMediaSrc('http://localhost:3006/'), false)
  assert.equal(isUsableMediaSrc('https://example.com'), false)
  assert.equal(usableMediaSrc('/'), undefined)
})

test('接受正常图片地址', () => {
  assert.equal(isUsableMediaSrc('/avatars/a.png'), true)
  assert.equal(isUsableMediaSrc('https://cdn.example.com/a.png'), true)
  assert.equal(usableMediaSrc('  /a.png  '), '/a.png')
})
