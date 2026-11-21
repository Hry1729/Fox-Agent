import { useId, type ComponentProps, type ReactNode } from 'react'
import { Check, Info } from 'lucide-react'
import type { ApprovalCard } from '@/components/assistant-ui/elements/approval-card'
import './contact-chat.css'

export function ContactDecisionExplanation({ children }: { children: ReactNode }) {
  return <div className="fox-contact-message"><div className="fox-contact-bubble">{children}</div></div>
}

export function ContactDecision({ title, description, children, resolved = false }: { title: string; description?: string; children: ReactNode; resolved?: boolean }) {
  const titleId = useId()
  return <section className={`fox-contact-decision${resolved ? ' is-resolved' : ''}`} aria-labelledby={titleId}>
    <header><strong id={titleId}>{title}</strong>{description && <p>{description}</p>}</header>
    {children}
  </section>
}

export function ContactChoice({ index, selected = false, children, ...props }: ComponentProps<'button'> & { index: number; selected?: boolean }) {
  return <button type="button" {...props} className="fox-contact-choice" aria-pressed={selected}>
    <span className="fox-contact-choice-key" aria-hidden="true">{String.fromCharCode(65 + index)}</span>
    <span>{children}</span>
    {selected && <Check size={15} aria-hidden="true" />}
  </button>
}

export function ContactAnswerChoice({ index, children }: { index: number; children: ReactNode }) {
  return <p className="fox-contact-answer-choice">
    <span className="fox-contact-choice-key" aria-hidden="true">{String.fromCharCode(65 + index)}</span><span>{children}</span>
  </p>
}

export function ContactApprovalCard({ state, title, command, description, onAllowOnce, onAllowConversation, onDeny, detailsContent, busy, statusLabel, invalidated = false }: ComponentProps<typeof ApprovalCard>) {
  const choices = [
    { label: '仅允许这一次', action: onAllowOnce },
    { label: '本次对话内允许', action: onAllowConversation },
    { label: '拒绝', action: onDeny },
  ].filter(choice => choice.action)
  if (state !== 'request') return <ContactDecision title={invalidated ? '本次操作未执行' : `可以${title}吗？`} resolved>
    <p className="fox-contact-receipt" role="status">{invalidated ? <Info size={15} aria-hidden="true" /> : <Check size={15} aria-hidden="true" />}{statusLabel}</p>
    {(command || description || detailsContent) && <details className="fox-contact-receipt-details"><summary>查看操作详情</summary>
      {command && <pre className="fox-contact-operation" tabIndex={0}>{command}</pre>}
      {description && <p>{description}</p>}{detailsContent}
    </details>}
  </ContactDecision>
  return <ContactDecision title={invalidated ? '本次操作未执行' : `可以${title}吗？`}>
    {command && <pre className="fox-contact-operation" tabIndex={0}>{command}</pre>}
    {description && <p className="fox-contact-decision-note">{description}</p>}
    <div className="fox-contact-choices">{choices.map((choice, index) =>
      <ContactChoice key={choice.label} index={index} disabled={busy} onClick={choice.action}>{choice.label}</ContactChoice>)}</div>
    {detailsContent}
  </ContactDecision>
}
