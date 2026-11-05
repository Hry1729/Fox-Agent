import { Button } from '@/components/ui/button'
import type { VerificationReport } from './agent-config-types'
const LAYERS = { config_saved: '配置已保存', model_request: '模型请求通过', agent_output: 'Agent 有输出', tool_call: '工具调用通过' } as const
export function VerificationLadder({ report, running, developer, onStop }: { report: VerificationReport | null; running: boolean; developer: boolean; onStop: () => void }) {
  return <section className="fox-agent-inline fox-verification" aria-label="分层验证结果">
    <div className="fox-model-actions"><b>{running ? '正在验证…' : '验证结果'}</b><span className="is-grow" />{running && <Button type="button" variant="outline" size="sm" onClick={onStop}>停止</Button>}</div>
    <ol aria-live="polite">{Object.entries(LAYERS).map(([layer, label]) => {
      const result = report?.layers.find(r => r.layer === layer)
      return <li key={layer} data-status={result?.status ?? 'skipped'}><b>{label}</b><span>{result?.message ?? (running ? '等待本次验证结果' : '未进行')}{result?.latencyMs != null ? ` · ${result.latencyMs} ms` : ''}</span>{developer && result?.code && <code>{result.code}</code>}{developer && result?.detail && <details><summary>诊断详情</summary><pre>{result.detail}</pre></details>}</li>
    })}</ol>
    {report && !running && <small>上次验证：{new Date(report.checkedAt * (report.checkedAt < 1e12 ? 1000 : 1)).toLocaleString()}</small>}
  </section>
}
