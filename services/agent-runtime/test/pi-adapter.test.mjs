import assert from 'node:assert/strict'
import { readFile, readdir } from 'node:fs/promises'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import test from 'node:test'
import { PI_PACKAGE_VERSION } from '../src/pi-adapter.mjs'

const root = resolve(dirname(fileURLToPath(import.meta.url)), '..')

test('keeps all Pi packages on one pinned version', async () => {
  const manifest = JSON.parse(await readFile(resolve(root, 'package.json'), 'utf8'))
  for (const dependency of [
    '@earendil-works/pi-agent-core',
    '@earendil-works/pi-ai',
    '@earendil-works/pi-coding-agent',
  ]) {
    assert.equal(manifest.dependencies[dependency], PI_PACKAGE_VERSION)
  }
})

test('keeps direct Pi imports inside the Pi adapter', async () => {
  const files = (await readdir(resolve(root, 'src')))
    .filter((file) => file.endsWith('.mjs') && file !== 'pi-adapter.mjs')
  for (const file of files) {
    const source = await readFile(resolve(root, 'src', file), 'utf8')
    assert.doesNotMatch(source, /@earendil-works\/pi-/u, file)
  }
})
