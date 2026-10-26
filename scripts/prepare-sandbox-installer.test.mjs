import { test } from 'node:test'
import assert from 'node:assert/strict'
import fs from 'node:fs/promises'
import os from 'node:os'
import path from 'node:path'
import { fileHash, verifyPackage } from './prepare-sandbox-installer.mjs'

test('installer rejects a missing, changed or unpinned runtime package', async () => {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'fox-installer-'))
  try {
    const file = path.join(dir, 'runtime.zip')
    await fs.writeFile(file, 'verified package fixture')
    const release = { version: '0.1.3', sha256: await fileHash(file) }
    await verifyPackage(file, release)
    await fs.appendFile(file, 'changed')
    await assert.rejects(verifyPackage(file, release), /SHA256 mismatch/)
    await assert.rejects(verifyPackage(file, { ...release, version: '../escape' }), /Invalid/)
    await assert.rejects(verifyPackage(file, { ...release, sha256: 'unknown' }), /Invalid/)
    await assert.rejects(verifyPackage(path.join(dir, 'missing.zip'), release), /ENOENT/)
  } finally {
    await fs.rm(dir, { recursive: true, force: true })
  }
})
