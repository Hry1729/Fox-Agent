import { useEffect, useId, useRef, useState, type ReactNode } from 'react'
import { CircleHelp } from 'lucide-react'
import { Button } from '@/components/ui/button'
import { Input } from '@/components/ui/input'
import type { RuntimeQuestion } from '@/features/conversations/model/pending-interactions'
import { DecisionPagination } from './decision-pagination'

function QuestionAnswerRows({ question, navigation, selected = [], disabled, onSelect }: {
  question: RuntimeQuestion
  navigation?: ReactNode
  selected?: readonly string[]
  disabled: boolean
  onSelect: (value: string) => void
}) {
  const headingId = useId()
  return <section className="fox-question-field" aria-labelledby={headingId}>
    <header className="fox-question-field-heading">
      <span aria-hidden="true"><CircleHelp /></span>
      <div><h3 id={headingId}>{question.question}</h3>{question.multiSelect && <small>可多选</small>}</div>
      {navigation}
    </header>
    <ol className="fox-question-answer-rows">
      {question.options.map((option, index) => {
        const pressed = selected.includes(option.value)
        return <li key={option.value}>
          <Button type="button" size="sm" variant={pressed ? 'default' : 'ghost'} aria-pressed={pressed} disabled={disabled} onClick={() => onSelect(option.value)}>
            <span className="fox-question-option-number" aria-hidden="true">{index + 1}</span><span>{option.label}</span>
          </Button>
        </li>
      })}
    </ol>
  </section>
}

export function QuestionPages({ questions, selections, otherAnswers, submitting, onSelect, onOtherChange, onSubmit }: {
  questions: readonly RuntimeQuestion[]
  selections: Readonly<Record<string, string[]>>
  otherAnswers: Readonly<Record<string, string>>
  submitting: boolean
  onSelect: (question: RuntimeQuestion, value: string) => void
  onOtherChange: (question: RuntimeQuestion, value: string) => void
  onSubmit: () => void
}) {
  const [page, setPage] = useState(0)
  const list = useRef<HTMLDivElement>(null)
  const current = Math.min(page, questions.length - 1)
  const question = questions[current]
  useEffect(() => { if (list.current) list.current.scrollTop = 0 }, [question?.questionId])
  if (!question) return null
  const last = current === questions.length - 1
  const advance = () => {
    if (!last) { setPage(current + 1); return }
    if (questions.length > 1) {
      const missing = questions.findIndex(item => !selections[item.questionId]?.length && !(item.allowOther && otherAnswers[item.questionId]?.trim()))
      if (missing >= 0) setPage(missing)
    }
    onSubmit()
  }
  return <div className="fox-question-decision-content">
    <div className="fox-question-decision-list" ref={list}>
      <QuestionAnswerRows question={question} selected={selections[question.questionId]} disabled={submitting}
        navigation={<DecisionPagination page={current} total={questions.length} item="题" disabled={submitting} onPage={setPage} />}
        onSelect={value => onSelect(question, value)} />
    </div>
    <div className="fox-question-decision-actions">
      {question.allowOther && <label className="fox-question-other-row">
        <span className="fox-question-option-number" aria-hidden="true">{question.options.length + 1}</span>
        <Input aria-label={`${question.question} 自定义答案`} placeholder="请输入其他选择" disabled={submitting}
          value={otherAnswers[question.questionId] ?? ''} onChange={event => onOtherChange(question, event.currentTarget.value)} />
      </label>}
      <Button type="button" disabled={submitting} onClick={advance}>{submitting ? '正在提交…' : last ? '确认并继续' : '下一题'}</Button>
    </div>
  </div>
}
