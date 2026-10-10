import { AssistantRuntimeProvider, ComposerPrimitive, MessagePrimitive, ThreadPrimitive, type AssistantRuntime } from '@assistant-ui/react'
import { Button } from '@/components/ui/button'
import './fox-thread.css'

/**
 * Minimal assistant-ui-driven thread (migration step 5B acceptance surface).
 *
 * This is not the final chat UI — 5C/5D replace it with the full message list
 * and composer. It exists to prove the runtime contract end to end: sending,
 * streaming and cancelling through the real conversation pipeline, with the
 * switch (`fox.ui.chatEngine`) gating it against the legacy implementation.
 */

function MinimalUserMessage() {
  return (
    <div className="fox-thread-msg is-user">
      <MessagePrimitive.Root>
        <MessagePrimitive.Parts />
      </MessagePrimitive.Root>
    </div>
  )
}

function MinimalAssistantMessage() {
  return (
    <div className="fox-thread-msg is-assistant">
      <MessagePrimitive.Root>
        <MessagePrimitive.Parts />
      </MessagePrimitive.Root>
    </div>
  )
}

export default function FoxThreadMinimal({ runtime }: { runtime: AssistantRuntime }) {
  return (
    <AssistantRuntimeProvider runtime={runtime}>
      <div className="fox-thread-minimal">
        <ThreadPrimitive.Root className="fox-thread-root">
          <ThreadPrimitive.Viewport className="fox-thread-viewport" autoScroll>
            <ThreadPrimitive.Empty>
              <div className="fox-thread-empty"> Fox 已就绪，输入内容开始对话 </div>
            </ThreadPrimitive.Empty>
            <ThreadPrimitive.Messages components={{ UserMessage: MinimalUserMessage, AssistantMessage: MinimalAssistantMessage }} />
            <ThreadPrimitive.LoadEarlier className="fox-thread-load-earlier">加载更早消息</ThreadPrimitive.LoadEarlier>
          </ThreadPrimitive.Viewport>
          <ThreadPrimitive.If running={false}>
            <ComposerPrimitive.Root className="fox-thread-composer">
              <ComposerPrimitive.Input className="fox-thread-composer-input" placeholder="发送消息…" rows={1} autoFocus />
              <ComposerPrimitive.Send className="fox-thread-composer-send">发送</ComposerPrimitive.Send>
            </ComposerPrimitive.Root>
          </ThreadPrimitive.If>
          <ThreadPrimitive.If running>
            <ComposerPrimitive.Root className="fox-thread-composer is-running">
              <ComposerPrimitive.Input className="fox-thread-composer-input" placeholder="Fox 正在回复…" rows={1} />
              <ComposerPrimitive.Cancel className="fox-thread-composer-cancel">停止</ComposerPrimitive.Cancel>
            </ComposerPrimitive.Root>
          </ThreadPrimitive.If>
        </ThreadPrimitive.Root>
      </div>
    </AssistantRuntimeProvider>
  )
}

/** Temporary engine switch for the acceptance round: a small ghost button is
 * enough during development and is removed with the minimal surface in 5C. */
export function ChatEngineSwitch({ engine, onChange }: { engine: 'legacy' | 'assistant-ui'; onChange: (engine: 'legacy' | 'assistant-ui') => void }) {
  return (
    <Button variant="ghost" size="sm" className="fox-thread-engine-switch" onClick={() => onChange(engine === 'legacy' ? 'assistant-ui' : 'legacy')}>
      {engine === 'legacy' ? '试用 assistant-ui 界面' : '返回经典界面'}
    </Button>
  )
}
