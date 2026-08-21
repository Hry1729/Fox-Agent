import assert from 'node:assert/strict'
import { test } from 'node:test'
import {
  canIndexFile,
  canReindexFile,
  canShowIndexButton,
  getIndexActionIconClass,
  getIndexActionLabel,
  getIndexConfigTitle
} from '../src/utils/knowledgeFilePolicy.ts'

test('canIndexFile only allows parsed and error_indexing files', () => {
  assert.equal(canIndexFile({ status: 'parsed' }), true)
  assert.equal(canIndexFile({ status: 'error_indexing' }), true)
  assert.equal(canIndexFile({ status: 'uploaded' }), false)
  assert.equal(canIndexFile({ is_folder: true, status: 'parsed' }), false)
})

test('canReindexFile only allows indexed and done files', () => {
  assert.equal(canReindexFile({ status: 'indexed' }), true)
  assert.equal(canReindexFile({ status: 'done' }), true)
  assert.equal(canReindexFile({ status: 'parsed' }), false)
})

test('index action label and style switch after indexing completes', () => {
  const indexed = { status: 'indexed' }
  assert.equal(getIndexActionLabel(indexed), '重新入库')
  assert.equal(getIndexConfigTitle(indexed), '重新入库参数配置')
  assert.equal(getIndexActionIconClass(indexed), 'bg-info/12 text-info')
  assert.equal(canShowIndexButton(indexed), true)

  const parsed = { status: 'parsed' }
  assert.equal(getIndexActionLabel(parsed), '入库')
  assert.equal(getIndexActionIconClass(parsed), 'bg-theme/12 text-theme')
})
