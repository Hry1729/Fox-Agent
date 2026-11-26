import { Button } from '@/components/ui/button'
import { useId, useState } from 'react'
import type { VerificationReport } from './agent-config-types'
const LAYERS = { config_saved: '配置', model_request: '模型请求', agent_output: 'Agent 输出', tool_call: '工具调用' } as const
export function VerificationLadder({ report, running, developer, onStop }: { report: VerificationReport | null; running: boolean; developer: boolean; onStop: () => void }) {
  const [expanded, setExpanded] = useState(false)
  const detailsId = useId()
  const label = report?.layers.some(layer => layer.status === 'failed') ? '验证未通过'
    : report?.layers.find(layer => layer.layer === 'agent_output')?.status === 'passed' ? 'Agent 已通过验证'
    : report ? '已验证部分项目' : '暂无验证结果'
  return <section className="fox-verification" aria-label="分层验证结果">
    <div className="fox-model-actions"><b>{running ? '正在验证…' : label}</b><span className="is-grow" />{running && <Button type="button" variant="outline" size="sm" onClick={onStop}>停止</Button>}<Button type="button" variant="ghost" size="sm" aria-expanded={expanded} aria-controls={detailsId} onClick={() => setExpanded(!expanded)}>{expanded ? '收起详情' : '查看详情'}</Button></div>
    <ol id={detailsId} hidden={!expanded} aria-live="polite">{Object.entries(LAYERS).map(([layer, label]) => {
      const result = report?.layers.find(r => r.layer === layer)
      return <li key={layer} data-status={result?.status ?? 'skipped'}><b>{label}</b><span>{result?.message ?? (running ? '等待本次验证结果' : '未进行')}{result?.latencyMs != null ? ` · ${result.latencyMs} ms` : ''}</span>{developer && result?.code && <code>{result.code}</code>}{developer && result?.detail && <details><summary>诊断详情</summary><pre>{result.detail}</pre></details>}</li>
    })}</ol>
    {report && !running && <small>上次验证：{new Date(report.checkedAt * (report.checkedAt < 1e12 ? 1000 : 1)).toLocaleString()}</small>}
  </section>
}
