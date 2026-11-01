// 侧栏行展示：时间不占行内，tooltip 用 24 小时制；日期分组不再带对话图标。
import { describe, expect, test } from 'bun:test'
import { act } from 'react'
import { createRoot, type Root } from 'react-dom/client'
import { formatClockTime } from '../src/lib/time-format'
import { Sidebar } from '../src/features/chat/shell/fox-sidebar'
import { TooltipProvider } from '../src/components/ui/tooltip'
import type { ConversationSummary, ProjectRecord } from '../src/features/conversations/model/types'

if (!window.matchMedia) {
  window.matchMedia = (() => ({ matches: false, addEventListener() {}, removeEventListener() {} })) as typeof window.matchMedia
}

const noop = () => {}
const at = Date.parse('2026-11-01T06:20:00Z')

const conversation = (id: string, title: string, projectRoot = ''): ConversationSummary => ({
  id,
  agentId: 'fox-general',
  agentName: 'Fox',
  title,
  projectId: null,
  projectRoot,
  createdAt: at,
  updatedAt: at,
  lastMessageAt: at,
  permissionMode: 'ask',
  status: 'active',
})

const project: ProjectRecord = {
  id: 'proj',
  name: '示例项目',
  rootPath: '/work/demo',
  createdAt: at,
  pinned: true,
  permissionMode: 'ask',
  status: 'active',
  updatedAt: at,
  lastOpenedAt: null,
}

function mount(node: HTMLElement) {
  const root = createRoot(node)
  act(() => {
    root.render(
      <TooltipProvider>
        <div className="fox-shell">
          <Sidebar
            collapsed={false}
            assistantMode="assistant"
            onModeChange={noop}
            onNewChat={noop}
            onNewProjectChat={noop}
            onAddProject={noop}
            onOpenConversation={noop}
            onRenameConversation={noop}
            onPinConversation={noop}
            onArchiveConversation={noop}
            onUnarchiveConversation={noop}
            onTrashConversation={noop}
            onRestoreConversation={noop}
            onPurgeConversation={noop}
            onDeleteProject={noop}
            onNavigate={noop}
            onExitManagement={noop}
            onExpand={noop}
            onTheme={noop}
            onSettings={noop}
            newChatActive={false}
            activeView="chat"
            runtimeConversations={[
              conversation('loose', '未归项目的对话'),
              conversation('in-project', '项目里的对话', '/work/demo'),
            ]}
            projects={[project]}
            runIndicators={new Map([['loose', 'approval']])}
          />
        </div>
      </TooltipProvider>,
    )
  })
  return root
}

describe('sidebar row presentation', () => {
  let host: HTMLDivElement
  let root: Root

  test('时间只出现在标题 tooltip，日期分组是纯文字，徽标仍是「等你处理」', () => {
    host = document.createElement('div')
    document.body.appendChild(host)
    root = mount(host)
    const clock = formatClockTime(at)
    expect(clock).not.toMatch(/AM|PM|上午|下午/)
    expect(host.querySelector('.fox-sidebar-row-time')).toBeNull()
    const titles = [...host.querySelectorAll<HTMLButtonElement>('.fox-sidebar-tree-main')]
    expect(titles.length).toBeGreaterThanOrEqual(2)
    for (const button of titles) {
      expect(button.title).toContain(`最后消息 ${clock}`)
      expect(button.title).not.toMatch(/AM|PM/)
    }
    const headings = [...host.querySelectorAll('.fox-sidebar-date-heading')]
    expect(headings.length).toBeGreaterThan(0)
    for (const heading of headings) {
      expect(heading.querySelector('svg')).toBeNull()
      expect(heading.textContent).not.toBe('')
    }
    const badge = host.querySelector('.fox-sidebar-attention')
    expect(badge?.textContent).toBe('等你处理')
    expect(badge?.getAttribute('data-attention')).toBe('approval')
    const pinned = host.querySelector('.fox-project-row svg[aria-label="已置顶项目"]')
    expect(pinned).not.toBeNull()
    act(() => { root.unmount() })
    host.remove()
  })

  test('区域样式把动作浮出文档流，并用 token 描述渐隐和徽标', async () => {
    const css = await Bun.file(new URL('../src/styles/areas/sidebar.css', import.meta.url)).text()
    expect(css).toContain('position: absolute')
    expect(css).toContain('--fox-sidebar-actions-fade')
    expect(css).toContain('--fox-sidebar-badge-height')
    expect(css).toContain('--fox-warning-text')
    expect(css).toContain('prefers-reduced-motion')
    const added = css.split('\n').filter((line) => line.includes(':') && !line.trim().startsWith('/*') && !line.trim().startsWith('*'))
    const bare = added.filter((line) => /#[0-9a-fA-F]{3,8}\b/.test(line) || /(?<!var\([^)]*)\b\d+ms\b/.test(line) || /(?<![-\d])\d+px\b/.test(line))
    expect(bare).toEqual([])
  })
})
