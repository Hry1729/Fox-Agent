import assert from 'node:assert/strict'
import { test } from 'node:test'
import {
  buildChunkParamsPayload,
  buildChunkParserConfigPayload
} from '../src/utils/chunkPresets.ts'

test('buildChunkParserConfigPayload includes token, overlap and delimiter', () => {
  assert.deepEqual(
    buildChunkParserConfigPayload({
      chunk_token_num: 800,
      overlapped_percent: 10,
      delimiter: '\\n\\n'
    }),
    {
      chunk_token_num: 800,
      overlapped_percent: 10,
      delimiter: '\\n\\n'
    }
  )
})

test('buildChunkParamsPayload nests parser config under chunk_parser_config', () => {
  assert.deepEqual(
    buildChunkParamsPayload({
      chunk_preset_id: 'general',
      chunk_parser_config: {
        chunk_token_num: 512,
        overlapped_percent: 5,
        delimiter: '---'
      }
    }),
    {
      chunk_preset_id: 'general',
      chunk_parser_config: {
        chunk_token_num: 512,
        overlapped_percent: 5,
        delimiter: '---'
      }
    }
  )
})
