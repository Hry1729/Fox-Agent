import type { ReactNode } from 'react'
import { ArrowRight } from 'lucide-react'
import './detail-prompt-cards.css'

export function DetailPromptCards({ label, name, mark, prompts, disabled, onSelect }: {
  label: string
  name: string
  mark: ReactNode
  prompts: string[]
  disabled?: boolean
  onSelect: (prompt: string) => void
}) {
  return <section className="fox-detail-prompt-cards" aria-label={label}>
    {prompts.map((prompt, index) => <button type="button" className="fox-detail-prompt-card" key={index}
      aria-label={prompt} disabled={disabled} onClick={() => onSelect(prompt)}>
      <span>
        <span className="fox-detail-prompt-mention" aria-hidden="true">{mark}{name}</span>
        {prompt}
      </span>
      <span className="fox-detail-prompt-arrow"><ArrowRight size={14} aria-hidden /></span>
    </button>)}
  </section>
}
