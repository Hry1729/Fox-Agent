import { describe, expect, test } from 'bun:test'
import { extractedAttachmentContext } from '../src/features/conversations/model/attachment-content'

describe('attachment context extraction', () => {
  test('ignores non-PDF attachments', async () => {
    expect(await extractedAttachmentContext([{
      filename: 'notes.txt',
      mediaType: 'text/plain',
      url: 'data:text/plain;base64,aGVsbG8=',
    }])).toBe('')
  })
})
