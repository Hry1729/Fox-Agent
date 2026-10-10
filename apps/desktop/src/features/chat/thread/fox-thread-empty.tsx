import { ThreadPrimitive } from '@assistant-ui/react'
import { NewConversationMascot } from '../components/NewConversationMascot'

const DEFAULT_SUGGESTIONS = [
  '整理这个项目中的文档',
  '从知识库查找相关资料',
  '读取一个文件并帮我分析',
  '帮我规划并完成一个任务',
]

/**
 * The empty-thread state (migration step 5C): mascot, one heading line and the
 * expert's opening suggestions (or the default four). A suggestion sends its
 * prompt through the runtime's `onNew` — the same optimistic path as typing.
 */
export function FoxThreadEmpty({ suggestions }: { suggestions?: string[] }) {
  const visible = (suggestions?.map((item) => item.trim()).filter(Boolean) ?? []).slice(0, 6)
  const prompts = visible.length ? visible : DEFAULT_SUGGESTIONS
  return (
    <div className="fox-empty-conversation">
      <div className="fox-empty-heading">
        <NewConversationMascot />
        <div className="fox-empty-heading-copy"><h2>今天想一起做点什么？</h2><p>Fox 可以和你对话、处理本地文件，也可以从知识库中检索资料。</p></div>
      </div>
      <div className="fox-starter-suggestions">
        {prompts.map((prompt) => (
          <ThreadPrimitive.Suggestion key={prompt} className="fox-starter-suggestion" prompt={prompt} send>
            {prompt}
          </ThreadPrimitive.Suggestion>
        ))}
      </div>
    </div>
  )
}
