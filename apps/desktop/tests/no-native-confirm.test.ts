// Guard: the native confirm / prompt / alert boxes must not come back. Under Tauri its title is the
// dev server address (127.0.0.1:1421), so every confirmation goes through the
// in-app `confirmDialog` from `src/components/ui/confirm-dialog.tsx` instead.
// Test files may still mention it (stubs, this guard); app code under src may not.

import { describe, expect, test } from 'bun:test'
import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join, relative } from 'node:path'

const SRC = join(import.meta.dir, '..', 'src')
const CODE = /\.(ts|tsx|js|jsx|mjs|cjs)$/
const TEST_FILE = /(\.test\.|\.spec\.|[\\/]__tests__[\\/])/

// `window.confirm(`, `globalThis.confirm(`, `self.confirm(`, `window['confirm']`,
// and a bare global `confirm(` (not a method such as `dialog.confirm(`).
const NATIVE_CONFIRM = [
  /\b(?:window|globalThis|self|top|parent)\s*\.\s*confirm\b/,
  /\b(?:window|globalThis|self)\s*\[\s*['"`]confirm['"`]\s*\]/,
  /(?<![\w$.])confirm\s*\(/,
]

// 13.23: the native prompt and alert boxes carry the same dev-server title, so
// they are banned too. Text input goes through `promptDialog`
// (`src/components/ui/prompt-dialog.tsx`), notices through the toast helper.
// Only the explicit global forms are matched: a bare `prompt(` / `alert(` is
// too common as a local name (e.g. an Agent `prompt`) to flag safely.
const NATIVE_PROMPT_ALERT = [
  /\b(?:window|globalThis|self|top|parent)\s*\.\s*(?:prompt|alert)\b/,
  /\b(?:window|globalThis|self)\s*\[\s*['"`](?:prompt|alert)['"`]\s*\]/,
]
const NATIVE_DIALOG = [...NATIVE_CONFIRM, ...NATIVE_PROMPT_ALERT]

function walk(dir: string, out: string[] = []): string[] {
  for (const name of readdirSync(dir)) {
    if (name === 'node_modules') continue
    const path = join(dir, name)
    if (statSync(path).isDirectory()) walk(path, out)
    else if (CODE.test(name) && !TEST_FILE.test(path)) out.push(path)
  }
  return out
}

function stripComments(source: string): string {
  return source.replace(/\/\*[\s\S]*?\*\//g, '').replace(/(^|[^:'"`])\/\/.*$/gm, '$1')
}

describe('no native confirm box in app code', () => {
  test('src contains no window.confirm / window.prompt / window.alert (test files excepted)', () => {
    const offenders: string[] = []
    for (const file of walk(SRC)) {
      const lines = stripComments(readFileSync(file, 'utf8')).split('\n')
      lines.forEach((line, index) => {
        if (NATIVE_DIALOG.some((pattern) => pattern.test(line))) {
          offenders.push(`${relative(SRC, file)}:${index + 1}: ${line.trim().slice(0, 120)}`)
        }
      })
    }
    expect(offenders).toEqual([])
  }, 120_000)

  test('the guard itself catches the forms it is meant to catch', () => {
    const hit = (line: string) => NATIVE_CONFIRM.some((pattern) => pattern.test(line))
    expect(hit("if (!window.confirm('删除？')) return")).toBe(true)
    expect(hit("globalThis.confirm('x')")).toBe(true)
    expect(hit("window['confirm']('x')")).toBe(true)
    expect(hit("if (confirm('x')) go()")).toBe(true)
    expect(hit("await confirmDialog({ title: '删除服务' })")).toBe(false)
    expect(hit("dialog.confirm()")).toBe(false)
    expect(hit("onConfirm={() => confirmFullAccess()}")).toBe(false)
  })

  test('the guard also catches native prompt and alert', () => {
    const hit = (line: string) => NATIVE_DIALOG.some((pattern) => pattern.test(line))
    expect(hit("const name = window.prompt('服务名称', provider.name)?.trim()")).toBe(true)
    expect(hit("window.alert('保存失败')")).toBe(true)
    expect(hit("globalThis.prompt('x')")).toBe(true)
    expect(hit("window['alert']('x')")).toBe(true)
    expect(hit("self.alert('x')")).toBe(true)
    expect(hit("await promptDialog({ title: '重命名服务' })")).toBe(false)
    expect(hit("systemPrompt: draft.prompt")).toBe(false)
    expect(hit("<Alert variant='destructive'>")).toBe(false)
    expect(hit("toast.error('保存失败')")).toBe(false)
  })

  test('the in-app prompt provider is mounted at the app root and rename uses it', () => {
    const app = readFileSync(join(SRC, 'app', 'App.tsx'), 'utf8')
    expect(app).toContain("from '@/components/ui/prompt-dialog'")
    expect(app).toContain('<PromptDialogProvider />')
    const settings = readFileSync(join(SRC, 'features/settings/settings-pages.tsx'), 'utf8')
    expect(settings).toContain("title: '重命名服务'")
  })

  test('the in-app confirmation provider is mounted at the app root', () => {
    const app = readFileSync(join(SRC, 'app', 'App.tsx'), 'utf8')
    expect(app).toContain("from '@/components/ui/confirm-dialog'")
    expect(app).toContain('<ConfirmDialogProvider />')
  })

  test('the 12 former call sites use confirmDialog with Chinese titles', () => {
    const read = (path: string) => readFileSync(join(SRC, path), 'utf8')
    const sites: Array<[string, string[]]> = [
      ['features/settings/settings-pages.tsx', ['删除服务', '删除连接器', '清理预览缓存', '恢复数据', '清理数据']],
      // 13.22: the Hook list moved into the shared section used by the
      // connector settings page and the plugin center「策略」tab.
      ['features/settings/lifecycle-hooks-section.tsx', ['删除 Hook']],
      ['features/agents/agent-pages.tsx', ['删除专家', '恢复专家版本']],
      ['features/agents/digital-colleague-manager.tsx', ['撤销数字同事']],
      ['features/chat/components/ManagedFilesPanel.tsx', ['强制恢复文件']],
      ['features/local-knowledge/local-knowledge-pages.tsx', ['放弃修改', '放弃修改']],
    ]
    let total = 0
    for (const [path, titles] of sites) {
      const source = read(path)
      const calls = source.match(/confirmDialog\(\{/g) ?? []
      expect({ path, calls: calls.length }).toEqual({ path, calls: titles.length })
      total += calls.length
      for (const title of titles) expect(source).toContain(`title: '${title}'`)
    }
    expect(total).toBe(12)
    // Destructive actions use the destructive confirm button.
    for (const title of ['删除服务', '清理预览缓存']) {
      expect(read('features/settings/settings-pages.tsx')).toMatch(new RegExp(`title: '${title}'[^)]*destructive: true`))
    }
    expect(read('features/settings/lifecycle-hooks-section.tsx')).toMatch(/title: '删除 Hook'[^)]*destructive: true/)
    // Both entry points render that one section, so the confirmation cannot drift.
    expect(read('features/settings/settings-pages.tsx')).toContain('<LifecycleHooksSection />')
    expect(read('features/plugins/plugin-center-page.tsx')).toContain('<LifecycleHooksSection />')
    expect(read('features/local-knowledge/local-knowledge-pages.tsx')).toMatch(/title: '放弃修改'[^)]*destructive: true/)
  })

  test('local knowledge discard checks close only after the confirmation resolves true', () => {
    const source = readFileSync(join(SRC, 'features/local-knowledge/local-knowledge-pages.tsx'), 'utf8')
    // Form: not dirty (or opening) passes straight through; dirty waits for the answer.
    expect(source).toMatch(/if \(nextOpen \|\| !dirty\) \{\s*onOpenChange\(nextOpen\)\s*return\s*\}/)
    expect(source).toMatch(/\.then\(\(confirmed\) => \{\s*discardConfirmingRef\.current = false\s*if \(confirmed\) onOpenChange\(false\)/)
    // Settings dialog: same for returning to the list.
    expect(source).toMatch(/if \(!dirty\) \{\s*returnToList\(\)\s*return\s*\}/)
    expect(source).toMatch(/\.then\(\(confirmed\) => \{\s*discardConfirmingRef\.current = false\s*if \(confirmed\) returnToList\(\)/)
    // Saving / submitting and an open confirmation still block a second request.
    expect(source).toContain('if (submitting || discardConfirmingRef.current) return')
    expect(source).toContain('if (saving || discardConfirmingRef.current) return')
  })
})
