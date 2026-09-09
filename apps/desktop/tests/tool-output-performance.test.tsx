import { expect, test } from 'bun:test'
import { renderToStaticMarkup } from 'react-dom/server'
import { ToolInput, ToolOutput } from '../src/components/ai-elements/tool'

test('large spreadsheet results render a bounded plain text page without token spans', () => {
  const text = 'AGV等待时间 A1: 123\n'.repeat(100000)
  const html = renderToStaticMarkup(<ToolOutput output={{ content: [{ type: 'text', text }], details: { text } }} errorText={undefined} />)
  expect(html).toContain('分页查看')
  expect(html).toContain('下一页')
  expect(html.length).toBeLessThan(12000)
  expect(html).not.toContain('shiki')
})

test('large tool inputs are bounded as well', () => {
  const html = renderToStaticMarkup(<ToolInput input={{ content: 'x'.repeat(1000000) }} />)
  expect(html.length).toBeLessThan(12000)
  expect(html).toContain('分页查看')
})
