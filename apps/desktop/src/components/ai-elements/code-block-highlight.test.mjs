import { expect, spyOn, test } from 'bun:test'
import { setTimeout, clearTimeout } from 'node:timers'
import { createElement } from 'react'
import { act, render, waitFor } from '@testing-library/react'
import { CodeBlock, CodeBlockCopyButton, highlightCode } from './code-block'

function highlight(code, language) {
  return new Promise((resolve, reject) => {
    const timeout = setTimeout(() => reject(new Error(`Highlighting did not finish: ${language}`)), 5000)
    const finish = result => { clearTimeout(timeout); resolve(result) }
    const cached = highlightCode(code, language, finish)
    if (cached) finish(cached)
  })
}

test('real Shiki grammars retain code and emit distinct light and dark syntax colors', async () => {
  const samples = [
    ['typescript', 'const message: string = "小狐";'],
    ['json', '{"name":"小狐","ready":true}'],
    ['python', 'def hello():\n    return "小狐"'],
    ['rust', 'fn main() { println!("小狐"); }'],
  ]
  for (const [language, code] of samples) {
    const result = await highlight(code, language)
    expect(result.tokens.map(line => line.map(token => token.content).join('')).join('\n')).toBe(code)
    const tokens = result.tokens.flat()
    expect(new Set(tokens.map(token => token.htmlStyle?.color ?? token.color)).size).toBeGreaterThan(1)
    expect(tokens.some(token => token.htmlStyle?.['--shiki-dark'])).toBe(true)
  }
}, 10000)

test('updating a code block preserves its source and copies the latest text', async () => {
  const first = 'const message = "旧内容";'
  const latest = 'const message = "中文流式结果";\nconsole.log(message);'
  const write = spyOn(navigator.clipboard, 'writeText').mockResolvedValue(undefined)
  const block = code => createElement(CodeBlock, { code, language: 'typescript' }, createElement(CodeBlockCopyButton, { 'aria-label': '复制代码' }))
  const view = render(block(first))
  try {
    await waitFor(() => expect(view.container.querySelectorAll('span[style*="color"]').length).toBeGreaterThan(3))
    await act(async () => view.rerender(block(latest)))
    await waitFor(() => expect(view.container.textContent).toContain('中文流式结果'))
    await act(async () => view.getByRole('button', { name: '复制代码' }).click())
    expect(write).toHaveBeenCalledWith(latest)
  } finally { view.unmount(); write.mockRestore() }
})

test('language aliases and unsupported languages preserve complete source text', async () => {
  for (const language of ['ts', 'text', 'unsupported-language']) {
    const code = `const text = "${language} 中文";`
    const result = await highlight(code, language)
    expect(result.tokens.flat().map(token => token.content).join('')).toBe(code)
  }
}, 10000)
