import test from 'node:test'
import assert from 'node:assert/strict'
import { finalizeKernelAnswer, completionPreview, KernelIncompleteResponseError } from '../src/kernel-completion.mjs'
import { prepareKernelModelResponse } from '../src/pi-kernel-batch-resume.mjs'

const prepared = { initial: true, runId: 'run', turnId: 'turn', checkpointSeq: 1, requireCompletion: true }
const answer = text => ({ role: 'assistant', stopReason: 'stop', content: [{ type: 'text', text }] })

test('an unmarked promise, blank answer, thinking alone and a marker alone cannot declare completion', () => {
  for (const message of [answer('Let me thoroughly analyze the data from the spreadsheet to produce accurate statistics for the report.'),
    answer(''), answer(' \n\t'), answer('<fox-final/>'), answer('\n<fox-final/>'),
    { ...answer(''), content: [] }, { ...answer(''), content: [{ type: 'thinking', thinking: 'I should analyze the workbook' }] }]) {
    assert.throws(() => prepareKernelModelResponse(message, prepared), KernelIncompleteResponseError)
  }
  for (const content of [[], [{ type: 'thinking', thinking: 'hidden' }], [{ type: 'text', text: '  ' }]]) {
    assert.throws(() => prepareKernelModelResponse({ ...answer(''), content }, { ...prepared, requireCompletion: false }), KernelIncompleteResponseError)
  }
})

test('model final decisions include short answers and concrete blockers without forcing tools', () => {
  for (const text of ['42', '请上传原始表格，目前没有可读取的附件。', '文件已保存到工具返回的路径。']) {
    const original = answer(`${text}\n<fox-final/>`)
    const before = structuredClone(original)
    assert.equal(prepareKernelModelResponse(original, prepared).assistantMessage.content[0].text, text)
    assert.deepEqual(original, before)
  }
  assert.deepEqual(finalizeKernelAnswer(answer('old frozen reply'), false), answer('old frozen reply'))
  const tool = { role: 'assistant', stopReason: 'toolUse', content: [{ type: 'toolCall', id: 'read-1', name: 'read', arguments: { path: 'a.txt' } }] }
  assert.deepEqual(prepareKernelModelResponse(tool, prepared).assistantMessage, tool)
})

test('split footer is removed while reasoning, inline literals and final previews remain intact', () => {
  const message = { ...answer(''), content: [
    { type: 'thinking', thinking: 'provider reasoning', thinkingSignature: 'signed' },
    { type: 'text', text: 'The literal <fox-final/> is inline.\r\n<fox-' }, { type: 'text', text: 'final/>\n' },
  ] }
  const result = finalizeKernelAnswer(message, true)
  assert.deepEqual(result.content[0], message.content[0])
  assert.equal(result.content.filter(block => block.type === 'text').map(block => block.text).join(''), 'The literal <fox-final/> is inline.')
  for (let i = 1; i <= '<fox-final/>'.length; i++) {
    assert.equal(completionPreview(`42\r\n${'<fox-final/>'.slice(0, i)}`, true), '42')
  }
  assert.equal(completionPreview('42\n<fox-final/>\n', true), '42')
})
