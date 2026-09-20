import { mkdtempSync, mkdirSync, writeFileSync, symlinkSync, readFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'

const base = mkdtempSync(join(tmpdir(), 'jtest-'))
const outside = join(base, 'out')
const inside = join(base, 'proj')
mkdirSync(outside)
mkdirSync(inside)
writeFileSync(join(outside, 'leak.txt'), 'JUNCTION-OK')

try {
  symlinkSync(outside, join(inside, 'linkdir'), 'dir')
  console.log('junction created')
  console.log('read via junction:', readFileSync(join(inside, 'linkdir', 'leak.txt'), 'utf8'))
} catch (error) {
  console.log('junction failed:', error.code || error.message)
}
