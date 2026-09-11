import { expect, test } from 'bun:test'
import { renderToStaticMarkup } from 'react-dom/server'
import { ReasoningText } from '../src/features/chat/components/ReasoningText'

test('unfinished reasoning markup stays visible and long histories remain bounded', () => {
  const html = renderToStaticMarkup(<ReasoningText text={'先核对 [尚未完成的链接](\n\n' + '检查工作表、单位与缺失值。\n\n'.repeat(2000)} />)
  expect(html).toContain('先核对 [尚未完成的链接](')
  expect(html).toContain('下一页')
  expect(html).not.toContain('<a ')
  expect(html).not.toContain('<p>')
  expect(html.length).toBeLessThan(6000)
})
