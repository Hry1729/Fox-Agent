import { expect, test } from 'bun:test'
import { approvalPresentation, displayPath } from '../src/features/conversations/model/approval-presentation'

test('13.37 审批卡展示时去掉 verbatim 前缀，审批请求本身不变', () => {
  expect(displayPath('\\\\?\\D:\\fox-test\\测试项目\\p0.txt')).toBe('D:\\fox-test\\测试项目\\p0.txt')
  expect(displayPath('\\\\?\\UNC\\server\\share\\a.txt')).toBe('\\\\server\\share\\a.txt')
  expect(displayPath('D:\\plain\\a.txt')).toBe('D:\\plain\\a.txt')
  expect(displayPath('写入 \\\\?\\D:\\a.txt 和 \\\\?\\D:\\b.txt')).toBe('写入 D:\\a.txt 和 D:\\b.txt')

  const request = { input: { path: '\\\\?\\D:\\fox-test\\测试项目\\p0.txt', content: 'hello' }, summary: '新建 \\\\?\\D:\\fox-test\\测试项目\\p0.txt' }
  const presentation = approvalPresentation({ request, toolName: 'write_file', requestedAction: '写入文件' } as never)
  expect(presentation.target).toBe('D:\\fox-test\\测试项目\\p0.txt')
  expect(presentation.summary).toBe('新建 D:\\fox-test\\测试项目\\p0.txt')
  expect(request.input.path).toBe('\\\\?\\D:\\fox-test\\测试项目\\p0.txt')
})
