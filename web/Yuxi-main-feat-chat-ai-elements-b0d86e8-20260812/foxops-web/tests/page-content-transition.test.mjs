import assert from 'node:assert/strict'
import fs from 'node:fs'
import path from 'node:path'
import test from 'node:test'
import { fileURLToPath } from 'node:url'

const root = path.resolve(path.dirname(fileURLToPath(import.meta.url)), '..')
const pageContentPath = path.join(root, 'src/components/core/layouts/art-page-content/index.vue')

test('ArtPageContent 避免双 Transition 并列卡死页面切换', () => {
  const source = fs.readFileSync(pageContentPath, 'utf8')
  const transitionCount = (source.match(/<Transition\b/g) || []).length

  assert.equal(transitionCount, 1, '应只有一个 Transition（用于非 keepAlive 页）')
  assert.match(
    source,
    /<KeepAlive[\s\S]*?v-if="route\.meta\.keepAlive"[\s\S]*?<\/KeepAlive>\s*<Transition/,
    'KeepAlive 应常驻且与 Transition 分离，避免 out-in 跨槽位等待'
  )
  assert.doesNotMatch(
    source,
    /<\/Transition>\s*(?:<!--[\s\S]*?-->\s*)?<Transition/,
    '不应再使用双 Transition 并列结构'
  )
})
