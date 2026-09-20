// 固定 fixture 构建器（harness-v1.0）：同名 fixture 生成确定性字节内容。
import { mkdir, writeFile } from 'node:fs/promises'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'

export async function buildFixture(name, spec, root) {
  const dir = join(root, name)
  await mkdir(dir, { recursive: true })
  if (spec.kind === 'text') {
    const text = spec.text ?? Array.from({ length: spec.lineCount }, (_, i) => `${spec.linePrefix}${i}`).join('\n') + '\n'
    const path = join(dir, 'content.txt')
    await writeFile(path, text, 'utf8')
    return { path, text, dir }
  }
  if (spec.kind === 'tree') {
    const sub = join(dir, 'files')
    await mkdir(sub, { recursive: true })
    for (let index = 0; index < spec.files; index += 1) {
      await writeFile(join(sub, `f${String(index).padStart(4, '0')}.txt`), `${spec.needle}\n`, 'utf8')
    }
    return { path: sub, text: null, dir, needle: spec.needle }
  }
  throw new Error(`unknown fixture kind: ${spec.kind}`)
}

// 产品运行器：从 --product-root 加载真实 Node 只读执行器（消费，不改写、不复制实现）。
export async function loadProductExecutor(productRoot) {
  const target = pathToFileURL(join(productRoot, 'services', 'agent-runtime', 'src', 'read-only-tool-executors.mjs')).href
  const module = await import(target)
  if (typeof module.executeReadOnlyTool !== 'function') {
    throw new Error(`product executor does not export executeReadOnlyTool: ${target}`)
  }
  return { execute: module.executeReadOnlyTool, entry: target }
}
