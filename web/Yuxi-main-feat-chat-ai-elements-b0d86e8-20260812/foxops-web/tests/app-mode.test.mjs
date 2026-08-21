import assert from 'node:assert/strict'
import { readFile } from 'node:fs/promises'
import { test } from 'node:test'

test('FoxOps defaults to frontend routes when access mode is omitted', async () => {
  const source = await readFile(
    new URL('../src/hooks/core/useAppMode.ts', import.meta.url),
    'utf8'
  )
  const developmentEnv = await readFile(new URL('../.env.development', import.meta.url), 'utf8')
  const productionEnv = await readFile(new URL('../.env.production', import.meta.url), 'utf8')

  assert.match(source, /VITE_ACCESS_MODE\s*\|\|\s*'frontend'/)
  assert.match(developmentEnv, /VITE_ACCESS_MODE\s*=\s*frontend/)
  assert.match(productionEnv, /VITE_ACCESS_MODE\s*=\s*frontend/)
})
