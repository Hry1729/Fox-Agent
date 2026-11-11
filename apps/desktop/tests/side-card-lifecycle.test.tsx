// SPEC item 4 缺口 A 的生命周期合同，跑真代码而不是只看正则：
// 1) 联系人卡是「聊天工作台」的卡片 —— 离开 chat（设置 / 共享记忆 / 智能体管理）必须
//    当场关掉、并且把打开状态清掉，回到 chat 时不能自己又开一次（那会再次声明原生宽度）；
// 2) 原生增长被拒时，DOM 只回滚「要不到宽度的那张卡」：已经落地的同级卡必须原地不动，
//    只是变宽的那张恢复原宽度而不是被关掉。
//
// 两边都调用真实代码：守卫走 renderHook + act（React 19），回滚策略直接调用提取出来的纯函数
// `sideCardGrowthRollback`。workbench 有没有真的接上它们，见文件末尾的接线断言，以及
// sidebar-window-growth-hook.test.tsx（假原生窗口 + 真 hook + 真拒绝回报）。
import { afterEach, describe, expect, test } from 'bun:test'
import { readFileSync } from 'node:fs'
import { useState } from 'react'
import { act, cleanup, renderHook } from '@testing-library/react'
import { rightPanelResizeBounds, sideCardGrowthRollback, useChatWorkspaceSideCard } from '../src/features/chat/workbench-side-cards'
import type { SidebarGrowthEntry, SidebarGrowthRefusal } from '../src/features/workspace/use-sidebar-window-growth'
import type { WorkspaceView } from '../src/features/workspace/types'

afterEach(cleanup)

const CONTACT_ID = 'conv-1'
const CONTACT_CARD: SidebarGrowthEntry = { id: 'contact-card', width: 320 }
const RIGHT_PANEL: SidebarGrowthEntry = { id: 'right-panel', width: 300 }

test('右栏拖动以实际可见宽度为起点，并给聊天区留空间', () => {
  expect(rightPanelResizeBounds(760, 1000, 480, 300)).toEqual({ width: 480, min: 280, max: 480 })
  expect(rightPanelResizeBounds(400, 1000, 400, 320)).toEqual({ width: 400, min: 280, max: 440 })
  expect(rightPanelResizeBounds(400, 1000, 400, 260)).toEqual({ width: 400, min: 280, max: 400 })
})

interface ProbeProps { initialView: WorkspaceView; cleared: WorkspaceView[] }

/** The workbench's own wiring, in miniature: the view and the card id live in the SAME
 *  component (so navigation and opening can batch, like `openContact` does), the gate is the
 *  extracted hook, and `entries` is what would be declared to the shared growth controller. */
function probe({ initialView, cleared }: ProbeProps) {
  const [view, setView] = useState(initialView)
  const [contactCardConversationId, setContactCardConversationId] = useState<string | null>(CONTACT_ID)
  const insideChat = useChatWorkspaceSideCard(view, () => {
    cleared.push(view)
    setContactCardConversationId(null)
  })
  const contactCardOpen = insideChat && contactCardConversationId === CONTACT_ID
  const entries: SidebarGrowthEntry[] = []
  if (insideChat) entries.push(RIGHT_PANEL)
  if (contactCardOpen) entries.push(CONTACT_CARD)
  return {
    view,
    entries,
    contactCardOpen,
    contactCardConversationId,
    navigate: (next: WorkspaceView) => setView(next),
    // workbench.openContact：先 handleOpenConversation（navigate('chat')），再写入卡片 id。
    openContact: (next: WorkspaceView) => { setView(next); setContactCardConversationId(CONTACT_ID) },
  }
}

const ids = (entries: SidebarGrowthEntry[]) => entries.map((entry) => entry.id)

function renderProbe(initialView: WorkspaceView) {
  const cleared: WorkspaceView[] = []
  const { result } = renderHook((props: ProbeProps) => probe(props), { initialProps: { initialView, cleared } })
  return {
    cleared,
    result,
    navigate: (view: WorkspaceView) => { act(() => { result.current.navigate(view) }) },
    openContact: (view: WorkspaceView) => { act(() => { result.current.openContact(view) }) },
  }
}

describe('离开 chat 时关闭联系人卡（缺口 A）', () => {
  test('切到管理页：当场隐藏并清空打开状态，回到 chat 不再自己出现', () => {
    const hook = renderProbe('chat')
    expect(hook.result.current.contactCardOpen).toBe(true)
    expect(ids(hook.result.current.entries)).toEqual(['right-panel', 'contact-card'])

    // 同一趟渲染里这张卡就不存在了：predicate 先挡住（没有一帧泄漏），effect 再把状态清掉。
    hook.navigate('settings')
    expect(hook.result.current.contactCardOpen).toBe(false)
    expect(ids(hook.result.current.entries)).toEqual([])
    expect(hook.result.current.contactCardConversationId).toBeNull()
    expect(hook.cleared).toEqual(['settings'])

    // 管理页不动选中的会话，所以回到 chat 时不能靠 id 相等把卡「复活」。
    hook.navigate('chat')
    expect(hook.result.current.contactCardOpen).toBe(false)
    expect(ids(hook.result.current.entries)).toEqual(['right-panel'])

    // 「共享记忆」那类跳转同样算离开（它停在 agent-detail 上）。
    hook.navigate('agent-detail')
    expect(hook.result.current.contactCardOpen).toBe(false)
    expect(hook.cleared).toEqual(['settings', 'agent-detail'])
  })

  test('挂载就不在 chat：状态立即清空，声明里也没有联系人卡', () => {
    const hook = renderProbe('agents')
    expect(hook.result.current.contactCardOpen).toBe(false)
    expect(ids(hook.result.current.entries)).toEqual([])
    expect(hook.result.current.contactCardConversationId).toBeNull()
    expect(hook.cleared).toEqual(['agents'])
  })

  test('在管理页打开联系人：导航与打开同批提交，卡片照常出现', () => {
    const hook = renderProbe('agents')
    expect(hook.cleared).toEqual(['agents'])
    // 守卫不能把这趟正当的打开当成「离开 chat」顺手清掉：两次更新合并成一次渲染，
    // 落地时视图已经是 chat，清空 effect 不会为中间状态运行。
    hook.openContact('chat')
    expect(hook.result.current.contactCardOpen).toBe(true)
    expect(ids(hook.result.current.entries)).toEqual(['right-panel', 'contact-card'])
    expect(hook.cleared).toEqual(['agents'])
  })

  test('离开一次就够：管理页之间来回切不重复清空，也不再声明卡片', () => {
    const hook = renderProbe('chat')
    hook.navigate('settings')
    hook.navigate('model-service')
    hook.navigate('plugins')
    // 只在 chat → 非 chat 的那一次跳转上清空：`insideChat` 没再变，effect 不会重跑。
    expect(hook.cleared).toEqual(['settings'])
    expect(ids(hook.result.current.entries)).toEqual([])
    hook.navigate('chat')
    expect(ids(hook.result.current.entries)).toEqual(['right-panel'])
    expect(hook.cleared).toEqual(['settings'])
  })
})

describe('被拒绝的增长只回滚请求者（缺口 A 的 DOM 侧）', () => {
  test('两种开关顺序都只怪罪新增加的那张卡', () => {
    expect(sideCardGrowthRollback([{ id: 'right-panel', width: 300, accepted: null }])).toEqual({
      closeContactCard: false, closeRightPanel: true, restoreRightPanelWidth: null,
    })
    expect(sideCardGrowthRollback([{ id: 'contact-card', width: 320, accepted: null }])).toEqual({
      closeContactCard: true, closeRightPanel: false, restoreRightPanelWidth: null,
    })
  })

  test('变宽失败：恢复真正落地的宽度，而不是关掉这张卡', () => {
    expect(sideCardGrowthRollback([{ id: 'right-panel', width: 500, accepted: 300 }])).toEqual({
      closeContactCard: false, closeRightPanel: false, restoreRightPanelWidth: 300,
    })
    // 一次拒绝里同时来了两张卡（面板变宽 + 新开的联系人卡），各按各的处置。
    expect(sideCardGrowthRollback([
      { id: 'right-panel', width: 500, accepted: 300 },
      { id: 'contact-card', width: 320, accepted: null },
    ])).toEqual({ closeContactCard: true, closeRightPanel: false, restoreRightPanelWidth: 300 })
  })

  test('迟到的同级卡成功后，面板 340 的拒绝精确恢复 300，两张卡都不关闭', () => {
    expect(sideCardGrowthRollback([{ id: 'right-panel', width: 340, accepted: 300 }])).toEqual({
      closeContactCard: false, closeRightPanel: false, restoreRightPanelWidth: 300,
    })
  })

  test('名单里没有的卡片一律不动：未知 id、空名单都不会误伤另一张卡', () => {
    const refusals: SidebarGrowthRefusal[] = [{ id: 'future-drawer', width: 400, accepted: null }]
    expect(sideCardGrowthRollback(refusals)).toEqual({ closeContactCard: false, closeRightPanel: false, restoreRightPanelWidth: null })
    expect(sideCardGrowthRollback([])).toEqual({ closeContactCard: false, closeRightPanel: false, restoreRightPanelWidth: null })
  })
})

describe('workbench 真的接上了这两个提取出来的部件', () => {
  const workbench = readFileSync(new URL('../src/features/chat/workbench.tsx', import.meta.url), 'utf8')

  test('联系人卡的开关先看 activeView；回报进来的是一组 refusal 而不是整份声明', () => {
    expect(workbench).toContain('useChatWorkspaceSideCard(activeView, closeContactCard)')
    expect(workbench).toContain('const contactCardOpen = insideChatWorkspace && Boolean(')
    expect(workbench).toContain('const handleSidebarGrowthRefused = useStableCallback((refusals: SidebarGrowthRefusal[])')
    expect(workbench).toContain('sideCardGrowthRollback(refusals)')
    // 旧的「按渲染顺序关掉最后一张」已经没有了。
    expect(workbench.includes('entries[entries.length - 1]')).toBe(false)
  })
})
