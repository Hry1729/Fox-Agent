import { useRef, useState } from 'react'
import type { RuntimeQuestion, RuntimeQuestionHistory } from '@/features/conversations/model/pending-interactions'
import { Input } from '@/components/ui/input'
import { Button } from '@/components/ui/button'
import { ContactChoice, ContactDecision } from './chat-decision'

export function ContactQuestionCard({ request, onSubmit, disabled = false }: {
  request: RuntimeQuestionHistory
  onSubmit?: (text: string, answers: Record<string, string | string[]>) => Promise<boolean>
  disabled?: boolean
}) {
  const [selections, setSelections] = useState<Record<string, string[]>>({})
  const [other, setOther] = useState<Record<string, string>>({})
  const [submitting, setSubmitting] = useState(false)
  const [error, setError] = useState('')
  const pending = useRef(false)
  const editable = request.status === 'pending'
  const select = (question: RuntimeQuestion, value: string) => {
    setOther(current => ({ ...current, [question.questionId]: '' }))
    setSelections(current => {
      const values = current[question.questionId] ?? []
      return { ...current, [question.questionId]: question.multiSelect
        ? values.includes(value) ? values.filter(item => item !== value) : [...values, value] : [value] }
    })
  }
  const submit = async () => {
    if (!onSubmit || pending.current || disabled) return
    const answers: Record<string, string | string[]> = {}
    const lines: string[] = []
    for (const question of request.questions) {
      const selected = selections[question.questionId] ?? []
      const text = other[question.questionId]?.trim()
      if (question.allowOther && text) {
        answers[question.questionId] = text
        lines.push(`${question.question}：${text}`)
      } else if (selected.length) {
        answers[question.questionId] = question.multiSelect ? selected : selected[0]
        lines.push(`${question.question}：${selected.map(value => question.options.find(option => option.value === value)?.label ?? value).join('、')}`)
      } else {
        setError('请回答每个问题，也可以填写自己的答案。')
        return
      }
    }
    pending.current = true
    setSubmitting(true)
    setError('')
    try {
      if (await onSubmit(lines.join('\n'), answers)) return
      setError('回答未发送成功，请重试。')
    } catch {
      setError('回答未发送成功，请重试。')
    }
    pending.current = false
    setSubmitting(false)
  }
  return <ContactDecision title={request.questions.length === 1 ? request.questions[0].question : '有几件事想和你确认'}
    description={editable ? '选好后发送，回答会留在这段对话里。' : request.status === 'answered' ? '已收到你的回答' : '这次提问已结束'}>
    {request.questions.map((question, index) => <div className="fox-contact-question" key={question.questionId}>
      {request.questions.length > 1 && <strong>{index + 1}. {question.question}</strong>}
      {question.multiSelect && <small>可多选</small>}
      <div className="fox-contact-choices">{question.options.map((option, optionIndex) => {
        const savedAnswer = request.answerText.split('\n').find(line => line.startsWith(`${question.question}：`) || line.startsWith(`${question.question}:`))?.slice(question.question.length + 1)
        const selected = editable ? (selections[question.questionId] ?? []).includes(option.value)
          : savedAnswer?.split('、').includes(option.label) ?? false
        return <ContactChoice key={option.value} index={optionIndex} selected={selected} disabled={!editable || disabled || submitting}
          onClick={() => select(question, option.value)}>{option.label}</ContactChoice>
      })}</div>
      {editable && question.allowOther && <Input aria-label={`${question.question}：自由回答`} value={other[question.questionId] ?? ''}
        disabled={disabled || submitting} placeholder="也可以输入自己的答案"
        onChange={event => {
          setOther(current => ({ ...current, [question.questionId]: event.target.value }))
          setSelections(current => ({ ...current, [question.questionId]: [] }))
        }} />}
    </div>)}
    {editable ? <Button size="sm" disabled={disabled || submitting || !onSubmit} onClick={() => void submit()}>{submitting ? '正在发送…' : '发送回答'}</Button>
      : request.answerText && <p className="fox-contact-answer-receipt">{request.answerText}</p>}
    {error && <p role="alert" className="fox-contact-decision-error">{error}</p>}
  </ContactDecision>
}
