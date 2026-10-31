import { afterEach, expect, test } from 'bun:test'
import { GlobalRegistrator } from '@happy-dom/global-registrator'
if (!GlobalRegistrator.isRegistered) GlobalRegistrator.register()
;(globalThis as any).IS_REACT_ACT_ENVIRONMENT = true
const React = await import('react')
const { act, cleanup, fireEvent, render, waitFor } = await import('@testing-library/react')
const { StreamingText } = await import('../src/features/chat/components/StreamingText')
const { Models } = await import('../src/components/assistant-ui/elements/composer')
const { FeedbackDialog } = await import('../src/components/assistant-ui/elements/feedback-dialog')
const { useFoxRuntime } = await import('../src/features/chat/thread/use-fox-runtime')
const { AssistantRuntimeProvider } = await import('@assistant-ui/react')
const { default: FoxThread } = await import('../src/features/chat/thread/fox-thread')
const groupHelpers = await import('../src/features/chat/runtime-process-groups')
// MarkdownResponse renders through React.lazy(import('message-response')), which pulls in
// Streamdown, KaTeX and Mermaid. A cold import of that graph takes several seconds under bun,
// longer than a test's 5 s budget, so the Suspense fallback (.fox-markdown-loading) was still
// on screen when the StreamingText assertions ran. Warm the module here, at file evaluation
// time, so the lazy import resolves from the module cache inside the tests.
await import('../src/components/ai-elements/message-response')
afterEach(cleanup)

test('输入流光仅新建页挂载，已有页即使centered、聚焦或运行也没有beam节点', async () => {
  const { MemoizedComposer } = await import('../src/features/chat/composer/fox-composer')
  const { TooltipProvider } = await import('../src/components/ui/tooltip')
  function Harness({ newConversation, running=false }: { newConversation: boolean; running?: boolean }) {
    const [draft,setDraft]=React.useState('')
    return <TooltipProvider><MemoizedComposer resetKey={0} draft={draft} setDraft={setDraft} chatState={running ? 'running' : 'idle'} centered newConversation={newConversation} runtimeControlled /></TooltipProvider>
  }
  const view=render(<Harness newConversation={false}/>)
  try {
    fireEvent.focusIn(view.getByRole('textbox'))
    expect(view.container.querySelector('[data-beam]')).toBeNull()
    expect(view.queryByLabelText('当前 Agent：Fox')).toBeNull()
    expect(view.queryByRole('button', { name: /选择 Agent/ })).toBeNull()
    await act(async()=>view.rerender(<Harness newConversation={false} running/>))
    expect(view.container.querySelector('[data-beam]')).toBeNull()
    await act(async()=>view.rerender(<Harness newConversation/>))
    fireEvent.focusIn(view.getByRole('textbox'))
    expect(view.container.querySelectorAll('[data-beam]').length).toBe(1)
    // Plan C1: a new conversation offers the Agent menu (Fox / Pi / Codex / DeepSeek).
    expect(view.getByRole('button', { name: '选择 Agent，当前 Fox' }).textContent).toBe('Fox')
  } finally {
    await act(async()=>view.unmount())
  }
})

test('中文增量直接显示正文，既有段落保留DOM，结束不丢Markdown和光标状态', async () => {
  const view = render(<StreamingText text={'**旧中文**\n\n正在追加'} streaming />)
  await waitFor(() => expect(view.container.querySelector('[data-streamdown="strong"]')).not.toBeNull(), { timeout: 5000 })
  const title = view.container.querySelector('[data-streamdown="strong"]')!
  const oldParagraph = [...view.container.querySelectorAll('p')].find(p => p.textContent === '正在追加')!
  expect(oldParagraph.childElementCount).toBe(0)
  expect(view.container.querySelector('[data-sd-animate]')).toBeNull()
  expect(view.container.querySelectorAll('[data-slot="streaming-text"][data-streaming="true"] > [aria-hidden]')).toHaveLength(1)
  await act(async () => view.rerender(<StreamingText text={'**旧中文**\n\n正在追加新文字'} streaming />))
  await waitFor(() => expect(view.container.textContent).toContain('正在追加新文字'))
  expect(view.container.querySelector('[data-streamdown="strong"]')).toBe(title)
  expect([...view.container.querySelectorAll('p')].find(p => p.textContent === '正在追加新文字')).toBe(oldParagraph)
  expect(oldParagraph.childElementCount).toBe(0)
  expect(view.container.querySelector('[data-sd-animate]')).toBeNull()
  expect(view.container.querySelector('.aui-streaming-fresh')).toBeNull()
  await act(async () => view.rerender(<StreamingText text={'**旧中文**\n\n正在追加新文字\n\n```typescript\nconst a = "中文";\n```'} streaming />))
  await waitFor(() => expect(view.container.querySelector('pre')?.textContent).toContain('const a = "中文";'))
  expect(view.container.querySelector('[data-streamdown="strong"]')).toBe(title)
  const paragraph = [...view.container.querySelectorAll('p')].find(p => p.textContent === '正在追加新文字')!
  await act(async () => view.rerender(<StreamingText text={'**旧中文**\n\n正在追加新文字\n\n```typescript\nconst a = "中文";\n```'} streaming={false} />))
  await waitFor(() => expect(view.container.querySelector('[data-slot="streaming-text"]')?.getAttribute('data-streaming')).toBe('false'))
  expect([...view.container.querySelectorAll('p')].find(p => p.textContent === '正在追加新文字')).toBe(paragraph)
  expect(view.container.querySelector('pre')?.textContent).toContain('const a = "中文";')
  expect(view.container.querySelector('[data-slot="streaming-text"] > [aria-hidden]')).toBeNull()
})

test('已完成历史初次加载没有流式动画节点', async () => {
  const view = render(<StreamingText text={'中文历史\n\n| 项目 | 状态 |\n| --- | --- |\n| A | 完成 |'} streaming={false} />)
  await waitFor(() => expect(view.container.querySelector('table')).not.toBeNull())
  expect(view.container.querySelector('[data-sd-animate]')).toBeNull()
})

test('Models打开后键盘选择关闭弹窗并将焦点归还触发器', async () => {
  function Harness() {
    const [value, setValue] = React.useState('fox')
    return <Models value={value} onValueChange={setValue} options={[{ id: 'fox', name: 'Fox' }, { id: 'colleague', name: '数字同事' }]} />
  }
  const view = render(<Harness />)
  const trigger = view.getByRole('button', { name: '选择模型' })
  fireEvent.click(trigger)
  await waitFor(() => expect(document.activeElement?.getAttribute('aria-label')).toBe('模型列表'))
  fireEvent.keyDown(document.activeElement!, { key: 'ArrowDown' })
  fireEvent.keyDown(document.activeElement!, { key: 'Enter' })
  await waitFor(() => expect(trigger.textContent).toContain('数字同事'))
  await waitFor(() => expect(view.queryByRole('dialog')).toBeNull())
  expect(document.activeElement).toBe(trigger)
})

test('反馈单选支持箭头，保存字段保持单分类与备注', async () => {
  let saved: unknown
  function Harness() {
    const [category, setCategory] = React.useState<'irrelevant' | 'code_error' | 'misunderstanding' | 'other'>('irrelevant')
    const [note, setNote] = React.useState('')
    return <FeedbackDialog open onOpenChange={() => {}} category={category} onCategoryChange={setCategory} note={note} onNoteChange={setNote} saving={false} onSubmit={() => { saved = { category, note } }} />
  }
  const view = render(<Harness />)
  fireEvent.keyDown(view.getByRole('radio', { name: '答非所问' }), { key: 'ArrowRight' })
  expect(view.getByRole('radio', { name: '代码错误' }).getAttribute('aria-checked')).toBe('true')
  fireEvent.change(view.getByRole('textbox', { name: '补充说明' }), { target: { value: '代码未运行' } })
  fireEvent.click(view.getByRole('button', { name: '提交反馈' }))
  expect(saved).toEqual({ category: 'code_error', note: '代码未运行' })
})

test('真实Composer恢复Fox原附件结构，移除保留项目资料库和原File身份', async () => {
  const { MemoizedComposer }=await import('../src/features/chat/composer/fox-composer')
  const { PromptInputProvider,usePromptInputController }=await import('../src/components/ai-elements/prompt-input')
  const { TooltipProvider }=await import('../src/components/ui/tooltip')
  const { ComposerRuntimeContext }=await import('../src/features/chat/fox-shared')
  const sample=new File(['附件正文'],'输入附件示例.txt',{type:'text/plain'})
  const createURL=URL.createObjectURL, revokeURL=URL.revokeObjectURL
  const revoked:string[]=[]
  URL.createObjectURL=()=> 'blob:attachment-preview'
  URL.revokeObjectURL=url=>{revoked.push(url)}
  let controller: ReturnType<typeof usePromptInputController>
  // The project chip moved out of the attachment row into ComposerProjectBar (59c31ff); it is
  // covered by its own test below, so this test keeps only the attachment and knowledge steps.
  let knowledgeClicks=0
  function Seed() {
    controller=usePromptInputController()
    const seeded=React.useRef(false)
    React.useEffect(()=>{if(seeded.current)return;seeded.current=true;controller.attachments.add([sample])},[])
    return null
  }
  function Harness() {
    const [draft,setDraft]=React.useState('')
    return <TooltipProvider><PromptInputProvider><Seed/><ComposerRuntimeContext.Provider value={{knowledgeBindings:[],knowledgeReferences:[{source:'local',id:'kb'}],localKnowledgeBases:[{id:'kb',name:'项目资料库'} as any]}}><MemoizedComposer resetKey={0} draft={draft} setDraft={setDraft} chatState="idle" runtimeControlled projectRoot="D:/projects/aipath-main" onKnowledge={()=>knowledgeClicks++}/></ComposerRuntimeContext.Provider></PromptInputProvider></TooltipProvider>
  }
  const view=render(<Harness />)
  try {
    await waitFor(()=>expect(view.queryByText('输入附件示例.txt')).not.toBeNull())
    expect(view.container.querySelector('[data-slot="composer-attachment"]')).toBeNull()
    expect(controller!.attachments.files[0].file).toBe(sample)
    const id=controller!.attachments.files[0].id
    fireEvent.click(view.getByRole('button',{name:'项目资料库'}))
    expect(knowledgeClicks).toBe(1)
    expect(controller!.attachments.files[0].id).toBe(id)
    fireEvent.click(view.getByRole('button',{name:'移除附件'}))
    expect(controller!.attachments.files).toHaveLength(0)
    expect(view.queryByText('输入附件示例.txt')).toBeNull()
    expect(view.getByRole('button',{name:'项目资料库'})).toBeDefined()
    expect(revoked).toContain('blob:attachment-preview')
  } finally {
    await act(async()=>view.unmount())
    URL.createObjectURL=createURL
    URL.revokeObjectURL=revokeURL
  }
})

test('ComposerProjectBar shows the current project folder name and offers adding a project', async () => {
  const { ComposerProjectBar }=await import('../src/features/chat/composer/composer-project-bar')
  let added=0
  const view=render(<ComposerProjectBar projectRoot="D:/projects/aipath-main" projects={[]} onSelect={()=>{}} onAdd={()=>added++} />)
  fireEvent.click(view.getByRole('button',{name:'aipath-main'}))
  await waitFor(()=>expect(view.getByRole('button',{name:'添加项目'})).toBeDefined())
  fireEvent.click(view.getByRole('button',{name:'添加项目'}))
  expect(added).toBe(1)
})

test('真实AUI适配器保留Kernel中途回复与最终回复，并只显示一份工具调用', async () => {
  const message = (id: string, content: string, role = 'assistant') => ({ id, role, content, runId: 'run', conversationId: 'c', kind: 'text', status: 'completed', createdAt: 1000, updatedAt: 6000, ordinal: 1 })
  const event = (seq: number, type: string, data = {}) => ({ runId: 'run', seq, eventType: type, event: { type, ...data }, createdAt: 1000 + seq * 1000 })
  const events = [event(1, 'run.started'), event(2, 'reasoning.delta', { delta: '先分析', source: 'kernel-model:1' }), event(3, 'tool.started', { toolCallId: 'read1', tool: 'read', input: { path: 'README.md' }, kernelCheckpointSeq: '1' }), event(4, 'tool.completed', { toolCallId: 'read1', tool: 'read', result: { content: 'ok' }, kernelCheckpointSeq: '1' }), event(5, 'run.completed')]
  const detail = { conversation: { id: 'c' }, messages: [message('u', '问题', 'user'), message('kernel-message:run:1', '中途回复'), message('kernel-message:run:2', '最终回复')], runtimeEvents: events, attachments: [], artifacts: [], approvals: [], toolCalls: [], knowledgeBindings: [], runs: [], lastRun: { id: 'run', status: 'completed', startedAt: 1000, finishedAt: 6000 } }
  function Harness() {
    const runtime = useFoxRuntime({ enabled: true, detail, streamingText: '', cancel: async () => true, loadEarlierMessages: async () => {} } as any, { onSubmit: () => {}, onReload: () => {} })
    return <AssistantRuntimeProvider runtime={runtime}><FoxThread fox={{ events: events as any, eventsByRun: new Map([['run', events as any]]), attachmentNames: new Map(), attachmentsByMessage: new Map(), knowledgeBindings: [], groupHelpers }} /></AssistantRuntimeProvider>
  }
  const view = render(<Harness />)
  await waitFor(() => expect(view.container.querySelector('.fox-answer-segment.is-final')?.textContent).toBe('最终回复'))
  expect(view.container.querySelectorAll('.fox-thread-msg.is-assistant')).toHaveLength(1)
  expect(view.container.querySelectorAll('[data-activity-kind="commentary"]')).toHaveLength(1)
  expect(view.container.querySelector('[data-activity-kind="commentary"]')?.closest('[hidden]')).not.toBeNull()
  fireEvent.click(view.container.querySelector('.fox-process-turn-toggle')!)
  for (const trigger of view.container.querySelectorAll('.fox-chain-of-thought-header')) fireEvent.click(trigger)
  await waitFor(() => expect(view.container.querySelectorAll('[data-slot="tool-call"]')).toHaveLength(1))
  expect(view.container.querySelector('[data-activity-kind="commentary"]')?.textContent).toBe('中途回复')
  expect(view.container.querySelector('.fox-answer-segment.is-final')?.textContent).toBe('最终回复')
})

test('AUI运行中隐藏当前助手footer，历史保留，终态整体显示；用户编辑仍受运行约束', async () => {
  const messages = [['u1', 'user', '第一问', 'r1'], ['a1', 'assistant', '第一答', 'r1'], ['u2', 'user', '第二问', 'r2'], ['a2', 'assistant', '第二答', 'r2']].map(([id, role, content, runId], i) => ({ id, role, content, runId, conversationId: 'c', kind: 'text', status: 'completed', ordinal: i + 1, createdAt: 1000 + i, updatedAt: 2000 + i }))
  function Harness({ running = false }) {
    const detail = { conversation: { id: 'c' }, messages, runtimeEvents: [], attachments: [], artifacts: [], approvals: [], toolCalls: [], knowledgeBindings: [], runs: [], lastRun: { id: 'r2', status: running ? 'running' : 'completed', startedAt: 1000, finishedAt: running ? null : 6000 } }
    const runtime = useFoxRuntime({ enabled: true, detail, streamingText: '', cancel: async () => true, loadEarlierMessages: async () => {} } as any, { onSubmit: () => {}, onEdit: async () => {}, onReload: () => {} })
    return <AssistantRuntimeProvider runtime={runtime}><FoxThread fox={{ events: [], eventsByRun: new Map(), attachmentNames: new Map(), attachmentsByMessage: new Map(), knowledgeBindings: [], groupHelpers }} /></AssistantRuntimeProvider>
  }
  const view = render(<Harness />)
  await waitFor(() => expect(view.getAllByRole('button', { name: '这条回答有帮助' })).toHaveLength(2))
  expect(view.getAllByRole('button', { name: '复制消息' })).toHaveLength(2)
  const edit = view.getByRole('button', { name: '编辑并重新发送' })
  expect(edit.closest('[data-turn-message-id]')?.getAttribute('data-turn-message-id')).toBe('u2')
  expect((edit as HTMLButtonElement).disabled).toBe(false)
  fireEvent.click(edit)
  await waitFor(() => expect(view.getByRole('textbox', { name: '编辑消息' })).toBeDefined())
  fireEvent.click(view.getByRole('button', { name: '取消' }))
  await waitFor(() => expect(view.queryByRole('textbox', { name: '编辑消息' })).toBeNull())
  await act(async () => view.rerender(<Harness running />))
  expect((view.getByRole('button', { name: '编辑并重新发送' }) as HTMLButtonElement).disabled).toBe(true)
  expect(view.container.querySelector('[data-turn-message-id="a2"] .fox-message-footer')).toBeNull()
  expect(view.container.querySelector('[data-turn-message-id="a1"] .fox-message-footer')).not.toBeNull()
  expect(view.getAllByRole('button', { name: '这条回答有帮助' })).toHaveLength(1)
  expect((view.getByRole('button', { name: '这条回答有帮助' }) as HTMLButtonElement).disabled).toBe(false)
  await act(async () => view.rerender(<Harness />))
  expect(view.getAllByRole('button', { name: '这条回答有帮助' })).toHaveLength(2)
  expect(view.container.querySelector('[data-turn-message-id="a2"] .fox-message-meta')).not.toBeNull()
})

test('AUI重新加载只有用户行的失败Run后保留错误卡片和连续重试入口', async () => {
  let retries = 0
  function Harness({ revision = 0 }) {
    const failed = { id: 'run', status: 'failed', startedAt: 1000, finishedAt: 2000, errorCode: 'runtime.start_failed', errorMessage: 'session.native_corrupt' }
    const detail = { conversation: { id: 'c', updatedAt: revision }, messages: [{ id: 'u', role: 'user', content: '测试', runId: 'run', conversationId: 'c', kind: 'text', status: 'completed', ordinal: 1, createdAt: 1000, updatedAt: 1000 }], runtimeEvents: [], attachments: [], artifacts: [], approvals: [], toolCalls: [], knowledgeBindings: [], runs: [failed], lastRun: failed }
    const runtime = useFoxRuntime({ enabled: true, detail, streamingText: '', cancel: async () => true, loadEarlierMessages: async () => {} } as any, { onSubmit: () => {}, onReload: () => { retries += 1 } })
    return <AssistantRuntimeProvider runtime={runtime}><FoxThread fox={{ events: [], eventsByRun: new Map(), attachmentNames: new Map(), attachmentsByMessage: new Map(), knowledgeBindings: [], groupHelpers }} /></AssistantRuntimeProvider>
  }
  const view = render(<Harness />)
  for (let attempt = 1; attempt <= 2; attempt += 1) {
    const retry = await view.findByRole('button', { name: '重试', exact: true })
    expect(retry.closest('[data-turn-message-id]')?.getAttribute('data-turn-message-id')).toBe('assistant-run')
    expect((retry as HTMLButtonElement).disabled).toBe(false)
    await act(async () => { fireEvent.click(retry) })
    expect(retries).toBe(attempt)
    await act(async () => view.rerender(<Harness revision={attempt} />))
    expect(view.container.querySelectorAll('.fox-thread-msg.is-assistant')).toHaveLength(1)
  }
})

test('经典当前助手footer运行期整体隐藏，终态显示并沿用重试回调', async () => {
  const { MemoizedRuntimeAssistantMessage } = await import('../src/features/chat/timeline/fox-runtime-assistant-message')
  let retries = 0
  const props = { message: { id: 'answer', role: 'assistant', content: '已完成回复', status: 'completed', runId: null, conversationId: 'c', kind: 'text', ordinal: 1, createdAt: 1000, updatedAt: 2000 } as any, processEvents: [], groupHelpers: null, singleRunMessage: false, running: false, artifacts: [], assistantName: 'Fox', onRetry: () => { retries += 1 } }
  const view = render(<MemoizedRuntimeAssistantMessage {...props} />)
  fireEvent.click(view.getByRole('button', { name: '重新生成回复' }))
  expect(retries).toBe(1)
  await act(async () => view.rerender(<MemoizedRuntimeAssistantMessage {...props} running />))
  expect(view.container.querySelector('.fox-message-footer')).toBeNull()
  expect(view.queryByRole('button', { name: '重新生成回复' })).toBeNull()
  await act(async () => view.rerender(<MemoizedRuntimeAssistantMessage {...props} runInProgress />))
  expect(view.container.querySelector('.fox-message-footer')).toBeNull()
  await act(async () => view.rerender(<MemoizedRuntimeAssistantMessage {...props} />))
  expect(view.container.querySelector('.fox-message-footer .fox-message-meta')).not.toBeNull()
  fireEvent.click(view.getByRole('button', { name: '重新生成回复' }))
  expect(retries).toBe(2)
  await act(async () => view.rerender(<MemoizedRuntimeAssistantMessage {...props} onRetry={undefined} />))
  expect(view.queryByRole('button', { name: '重新生成回复' })).toBeNull()
})
