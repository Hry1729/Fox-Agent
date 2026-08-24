import assert from 'node:assert/strict'
import { mkdtemp, rm, writeFile } from 'node:fs/promises'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import test from 'node:test'
import { validateContractFixtures, validateManifest } from './validate-contract-fixtures.mjs'

test('validates the committed contract fixture manifest', async () => {
  const result = await validateContractFixtures()
  assert.equal(result.fixtureCount > 0, true)
  assert.equal(result.enumCount > 0, true)
  assert.equal(result.enumMemberCount > 0, true)
})

test('rejects an enum manifest with an incomplete fixture declaration', () => {
  assert.throws(() => validateManifest({
    manifestVersion: 1,
    fixtures: [],
    enums: [{ name: 'status', members: [{ name: 'queued', fixture: '../queued.json' }] }],
  }), /fixture.*必须是仓库内相对 JSON 路径/)
})

test('rejects malformed JSON and mismatched enum fixtures', async () => {
  const root = await mkdtemp(join(tmpdir(), 'fox-contract-fixtures-'))
  try {
    await writeFile(join(root, 'manifest.json'), JSON.stringify({
      manifestVersion: 1,
      fixtures: [{ id: 'sample', path: 'sample.json' }],
      enums: [{ name: 'status', members: [{ name: 'queued', fixture: 'queued.json' }] }],
    }))
    await writeFile(join(root, 'sample.json'), '{not-json')
    await assert.rejects(() => validateContractFixtures(join(root, 'manifest.json')), /JSON 无法解析/)

    await writeFile(join(root, 'sample.json'), '{}')
    await writeFile(join(root, 'queued.json'), JSON.stringify({ enum: 'status', value: 'running' }))
    await assert.rejects(() => validateContractFixtures(join(root, 'manifest.json')), /与 manifest 不一致/)
  } finally {
    await rm(root, { recursive: true, force: true })
  }
})
