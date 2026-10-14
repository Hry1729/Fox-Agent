import { FOX_ASSISTANT_AVATAR, RunStatusText } from '../components/FoxAssistantAvatar'
import { ErrorState } from '@/components/assistant-ui/elements/error-state'
import { expertErrorPresentation } from '../components/expert-binding-ui'
import { ChainOfThought, ChainOfThoughtContent, ChainOfThoughtHeader, ChainOfThoughtSearchResult, ChainOfThoughtSearchResults, ChainOfThoughtStep } from '@/components/ai-elements/chain-of-thought'
import { Confirmation, ConfirmationAction, ConfirmationActions, ConfirmationRejected, ConfirmationRequest, ConfirmationTitle } from '@/components/ai-elements/confirmation'
import { InlineCitation, InlineCitationCard, InlineCitationCardBody, InlineCitationCardTrigger, InlineCitationCarousel, InlineCitationCarouselContent, InlineCitationCarouselHeader, InlineCitationCarouselIndex, InlineCitationCarouselItem, InlineCitationCarouselNext, InlineCitationCarouselPrev, InlineCitationQuote, InlineCitationSource, InlineCitationText } from '@/components/ai-elements/inline-citation'
import { Button } from '@/components/ui/button'
import { type DesktopErrorDetails } from '@/features/conversations/model/types'
import { Brain, ChevronRight, FileText, MessageCircleMore, RotateCcw, Search, ShieldCheck } from 'lucide-react'
import { useState } from 'react'

const citationUrls = [
  'https://github.com/xingyuv/kun/blob/develop/LICENSE',
  'https://github.com/xingyuv/kun/blob/develop/CLA.md'
]

export const licenseAnswerMarkdown = `### 企业内部使用

营利性企业将 Kun 用作内部效率工具、业务系统或其他商业运营用途，通常属于商业使用，**需要向版权持有者申请单独的书面商业授权**。

| 使用场景 | 是否允许 |
| --- | --- |
| 个人学习与非商业二次开发 | 允许 |
| 教育、公益研究 | 允许 |
| 营利企业内部业务使用 | 需要商业授权 |
| SaaS、商业产品集成或转售 | 需要商业授权 |`

export function AssistantProcess({ running = false }: { running?: boolean }) {
  const [open, setOpen] = useState(false)

  return (
    <ChainOfThought open={open} onOpenChange={setOpen} className="fox-chain-of-thought">
      <ChainOfThoughtHeader className="fox-chain-of-thought-header">
        {running ? <RunStatusText text="正在处理" /> : <span>处理过程（7 步）</span>}
        {!running && <small>· 思考 10 秒</small>}
      </ChainOfThoughtHeader>
      <ChainOfThoughtContent className="fox-chain-of-thought-content">
        <ChainOfThoughtStep icon={Search} label="查找 LICENSE*" status="complete">
          <ChainOfThoughtSearchResults>
            <ChainOfThoughtSearchResult>Kun/LICENSE</ChainOfThoughtSearchResult>
          </ChainOfThoughtSearchResults>
        </ChainOfThoughtStep>
        <ChainOfThoughtStep icon={Search} label="搜索 license、License、LICENSE" status="complete">
          <ChainOfThoughtSearchResults>
            <ChainOfThoughtSearchResult>Kun/CLA.md</ChainOfThoughtSearchResult>
            <ChainOfThoughtSearchResult>Kun/README.md</ChainOfThoughtSearchResult>
          </ChainOfThoughtSearchResults>
        </ChainOfThoughtStep>
        <ChainOfThoughtStep
          icon={Brain}
          label="分析许可证问题"
          description="需要依据仓库中的 LICENSE 与 CLA 判断二次开发和企业内部使用范围。"
          status="complete"
        />
        <ChainOfThoughtStep
          icon={MessageCircleMore}
          label="好的，我来查一下 Fox 项目（Kun）的许可证信息。"
          status="complete"
        />
        <ChainOfThoughtStep icon={FileText} label="读取 Kun/LICENSE" status="complete">
          <div className="fox-chain-detail">确认项目采用 PolyForm Noncommercial License 1.0.0。</div>
        </ChainOfThoughtStep>
        <ChainOfThoughtStep
          icon={Brain}
          label="核对许可范围"
          description="非商业学习、研究和实验允许使用；商业用途需要单独书面授权。"
          status="complete"
        />
        <ChainOfThoughtStep
          icon={ShieldCheck}
          label={running ? '正在判断企业内部使用场景…' : '判断企业内部使用场景'}
          description="营利性企业用于内部业务通常属于商业用途，需要向版权持有者申请授权。"
          status={running ? 'active' : 'complete'}
        />
      </ChainOfThoughtContent>
    </ChainOfThought>
  )
}

export function ApprovalPrompt({ onApprove, onDeny }: { onApprove: () => void; onDeny: () => void }) {
  return (
    <Confirmation approval={{ id: 'fox-write-file' }} state="approval-requested" className="fox-confirmation">
      <ConfirmationRequest>
        <div className="fox-confirmation-body">
          <span className="fox-prompt-figure"><img src={FOX_ASSISTANT_AVATAR} alt="" /></span>
          <div><ConfirmationTitle>允许 Fox 修改本机文件？</ConfirmationTitle><p><code>apps/desktop/src/styles/workbench.css</code></p><small>Fox 将写入此工作区中的一个文件。你可以在 Changes 面板检查修改。</small></div>
        </div>
        <ConfirmationActions className="fox-confirmation-actions">
          <ConfirmationAction variant="ghost" onClick={onDeny}>拒绝</ConfirmationAction>
          <ConfirmationAction onClick={onApprove}>允许一次</ConfirmationAction>
        </ConfirmationActions>
      </ConfirmationRequest>
    </Confirmation>
  )
}

export function DeniedPrompt() {
  return (
    <Confirmation approval={{ id: 'fox-write-file', approved: false }} state="approval-responded" className="fox-denied-confirmation">
      <ConfirmationRejected>
        <div className="fox-denied-prompt">
          <img className="fox-inline-mascot" src={FOX_ASSISTANT_AVATAR} alt="" />
          <span><strong>操作已取消</strong><small>Fox 没有修改任何本机文件。</small></span>
        </div>
      </ConfirmationRejected>
    </Confirmation>
  )
}

function errorPresentation(error?: string | null, details?: DesktopErrorDetails | null) {
  const detail = error?.trim() ?? ''
  const expertError = expertErrorPresentation(details ?? error)
  if (expertError) {
    return {
      title: expertError.title,
      description: expertError.description,
      detail: '',
      retryable: expertError.retryable,
      retryLabel: expertError.retryLabel,
    }
  }
  if (/Pi Runtime dependencies|Fox Runtime|runtime (?:script|executable)|Node\.js is required/i.test(detail)) {
    return {
      title: 'Fox 未能启动，请重启应用后重试',
      description: '本机专家运行环境未能启动。请确认 Fox 依赖完整，并在重启应用后重试。',
      detail,
    }
  }
  if (/knowledge|知识库|yuxi|ECONNREFUSED.*5010/i.test(detail)) {
    return {
      title: '资料库服务连接失败，请检查设置',
      description: '资料库服务暂时没有响应。请确认服务已启动，或稍后重试。',
      detail,
    }
  }
  return {
    title: '处理失败，可重试或查看问题诊断',
    description: '运行过程中遇到了问题。你可以重试，或在设置中查看运行诊断。',
    detail,
  }
}

export function ErrorPrompt({ onRetry, error, errorDetails }: { onRetry?: () => void; error?: string | null; errorDetails?: DesktopErrorDetails | null }) {
  const presentation = errorPresentation(error, errorDetails)
  return <ErrorState title={presentation.title} detail={presentation.description} diagnostic={presentation.detail} onRetry={presentation.retryable === false ? undefined : onRetry} retryLabel={presentation.retryLabel} />
}

const clarificationOptions = [
  { id: 'analyze', title: '只看不改', detail: '先阅读相关文件并给出建议，不写入工作区' },
  { id: 'edit', title: '修改文件', detail: '按推荐方案修改文件，写入前仍会请求批准' },
  { id: 'backup', title: '先备份再改', detail: '保留原文件副本后再执行修改' }
] as const

export function QuestionPrompt({ onAnswer }: { onAnswer: (answer: string) => void }) {
  return (
    <div className="fox-question-prompt">
      <div className="fox-question-heading">
        <span className="fox-prompt-figure"><img src={FOX_ASSISTANT_AVATAR} alt="" /></span>
        <div><strong>你希望我怎样处理这些文件？</strong><p>确认执行方式后，Fox 会继续当前任务。</p></div>
      </div>
      <div className="fox-question-options">
        {clarificationOptions.map((option, index) => (
          <button type="button" key={option.id} onClick={() => onAnswer(option.title)}>
            <kbd>{index + 1}</kbd><span><strong>{option.title}</strong><small>{option.detail}</small></span><ChevronRight size={14} />
          </button>
        ))}
      </div>
    </div>
  )
}

export function LicenseInlineCitation() {
  return (
    <InlineCitation className="fox-inline-citation">
      <InlineCitationText>Kun 使用 PolyForm Noncommercial License 1.0.0</InlineCitationText>
      <InlineCitationCard>
        <InlineCitationCardTrigger sources={citationUrls} className="fox-inline-citation-trigger">2 个依据</InlineCitationCardTrigger>
        <InlineCitationCardBody className="fox-inline-citation-card">
          <InlineCitationCarousel>
            <InlineCitationCarouselHeader className="fox-inline-citation-head">
              <InlineCitationCarouselPrev />
              <InlineCitationCarouselIndex />
              <InlineCitationCarouselNext />
            </InlineCitationCarouselHeader>
            <InlineCitationCarouselContent>
              <InlineCitationCarouselItem>
                <InlineCitationSource title="PolyForm Noncommercial License 1.0.0" url="Kun/LICENSE" description="项目当前使用的非商业许可协议。" />
                <InlineCitationQuote>许可范围涵盖个人、教育、研究及其他非商业用途。</InlineCitationQuote>
              </InlineCitationCarouselItem>
              <InlineCitationCarouselItem>
                <InlineCitationSource title="Contributor License Agreement" url="Kun/CLA.md" description="确认项目授权与商业许可安排。" />
                <InlineCitationQuote>商业使用需要获得版权持有者单独的书面授权。</InlineCitationQuote>
              </InlineCitationCarouselItem>
            </InlineCitationCarouselContent>
          </InlineCitationCarousel>
        </InlineCitationCardBody>
      </InlineCitationCard>
    </InlineCitation>
  )
}
