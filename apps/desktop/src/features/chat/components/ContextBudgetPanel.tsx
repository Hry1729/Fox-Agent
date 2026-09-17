import { useState } from 'react'
import { Button } from '@/components/ui/button'
import type { ContextBudgetDto } from './context-budget-presentation'
import {
  calibrationNote,
  compactTokenCount,
  contextBudgetRows,
  estimateSourceLabel,
  estimateSourceMeasured,
  pressurePercent,
  triggerReasonLabel,
  usageOccupancyNote,
} from './context-budget-presentation'
import '../context-budget.css'

/**
 * Explainable context-occupancy panel (#11/#12): itemized budget rows, the
 * estimation origin (measured vs estimated), and the compaction trigger
 * reason. Purely presentational — window A supplies the DTO and the mount.
 */
export function ContextBudgetPanel({ budget }: { budget: ContextBudgetDto }) {
  const [expanded, setExpanded] = useState(false)
  const percent = pressurePercent(budget)
  const measured = estimateSourceMeasured(budget)
  const rows = contextBudgetRows(budget)
  const usageNote = usageOccupancyNote(budget)
  const calibration = calibrationNote(budget)
  const panelId = 'fox-context-budget-body'
  return <section className="fox-context-budget" aria-label="上下文占用说明">
    <div className="fox-context-budget-heading">
      <div className="fox-context-budget-summary">
        <strong>上下文 {compactTokenCount(budget.modelWindowTokens - budget.availableTokens)} / {compactTokenCount(budget.modelWindowTokens)}</strong>
        <span className={measured ? 'fox-context-budget-badge is-measured' : 'fox-context-budget-badge'}>
          {estimateSourceLabel(budget)}
        </span>
      </div>
      <Button variant="ghost" size="sm" onClick={() => setExpanded(value => !value)} aria-expanded={expanded} aria-controls={panelId}>
        {expanded ? '收起' : '占用明细'}
      </Button>
    </div>
    <div className="fox-context-budget-bar" role="meter" aria-valuemin={0} aria-valuemax={100} aria-valuenow={percent}
      aria-label={`上下文压力 ${percent}%`}>
      <div className={percent >= 90 ? 'fox-context-budget-bar-fill is-high' : 'fox-context-budget-bar-fill'} style={{ width: `${percent}%` }} />
    </div>
    <p className="fox-context-budget-reason">{triggerReasonLabel(budget)}</p>
    {expanded && <div id={panelId} className="fox-context-budget-body">
      <dl className="fox-context-budget-rows">
        {rows.map(row => <div className="fox-context-budget-row" key={row.key}>
          <dt>{row.label}</dt>
          <dd>
            <b>{compactTokenCount(row.tokens)}</b>
            <span className={row.exact ? 'fox-context-budget-tag is-exact' : 'fox-context-budget-tag'}>
              {row.exact ? '精确' : row.note}
            </span>
          </dd>
        </div>)}
        <div className="fox-context-budget-row is-total">
          <dt>剩余可用</dt>
          <dd><b>{compactTokenCount(budget.availableTokens)}</b></dd>
        </div>
      </dl>
      {budget.usageInputTokens !== undefined && <p className="fox-context-budget-note">
        实测输入占用 <b>{compactTokenCount(budget.usageInputTokens)}</b>
        {usageNote ? <span>（{usageNote}）</span> : null}
      </p>}
      {calibration && <p className="fox-context-budget-note">{calibration}</p>}
      <p className="fox-context-budget-note">估算值用于提前预警；真实上限以模型供应商为准。压缩不会改写原始历史，也不重放工具。</p>
    </div>}
  </section>
}
