import { ChevronRight, FileEdit, LoaderCircle, ShieldCheck, Terminal } from 'lucide-react'
import { useEffect, useId, useRef, useState } from 'react'
import { FoxDisclosure } from '@/components/fox-ui/fox-disclosure'
import { SideCard, SideSection } from '@/components/fox-ui/side-section'
import { ScrollArea } from '@/components/ui/scroll-area'
import type { ApprovalRecord, ConversationDetail, TaskEvidenceRecord, ToolCallRecord } from '@/features/conversations/model/types'
import { commandOutput, isChangeOrCommandTool, isCommandTool, toolExecutionStatus, toolStatusLabel } from '../composer/fox-composer'
import { useDeveloperUi } from '../developer-ui'
import { formatDurationMs } from '../sidebar-model'
import { toolRecordPresentation, toolTarget } from '../tool-record-presentation'
import { RightPanelEmpty } from './right-panel-empty'

export function ChangesPanel({ detail, focusedEvidence, onOpenEvidence }: { detail: ConversationDetail | null; focusedEvidence: TaskEvidenceRecord | null; onOpenEvidence?: (evidence: TaskEvidenceRecord) => void }) {
  const developerUi = useDeveloperUi()
  const changes = (detail?.toolCalls ?? []).filter(item => isChangeOrCommandTool(item.toolName)).sort((a, b) => b.startedAt - a.startedAt)
  if (!changes.length) return <RightPanelEmpty icon={FileEdit}>这段对话还没有修改或命令记录。</RightPanelEmpty>
  return <div className="fox-change-panel">
    <SideSection title="更改" count={changes.length}>
      <SideCard>
        {changes.map(tool => <ChangeRow key={tool.id} tool={tool} approvals={detail?.approvals ?? []} evidence={detail?.evidence ?? []}
          focusedEvidence={focusedEvidence && evidenceMatchesTool(focusedEvidence, tool) ? focusedEvidence : null}
          developerUi={developerUi} onOpenEvidence={onOpenEvidence} />)}
      </SideCard>
    </SideSection>
  </div>
}

function evidenceMatchesTool(evidence: TaskEvidenceRecord, tool: ToolCallRecord) {
  return evidence.refKind === 'tool_call' && (evidence.refId === tool.id || evidence.refId === tool.runtimeToolCallId)
}

function ChangeRow({ tool, approvals, evidence, focusedEvidence, developerUi, onOpenEvidence }: {
  tool: ToolCallRecord; approvals: ApprovalRecord[]; evidence: TaskEvidenceRecord[];
  focusedEvidence: TaskEvidenceRecord | null; developerUi: boolean; onOpenEvidence?: (evidence: TaskEvidenceRecord) => void
}) {
  const [open, setOpen] = useState(Boolean(focusedEvidence))
  const id = useId()
  const row = useRef<HTMLDivElement>(null)
  useEffect(() => {
    if (!focusedEvidence) return
    setOpen(true)
    row.current?.scrollIntoView?.({ block: 'nearest' })
  }, [focusedEvidence])

  const command = isCommandTool(tool.toolName)
  const Icon = command ? Terminal : FileEdit
  const target = toolTarget(tool) || toolRecordPresentation(tool).action
  const time = new Date(tool.startedAt).toLocaleTimeString('zh-CN', { hour: '2-digit', minute: '2-digit' })
  const status = toolStatusLabel(tool)
  const linkedEvidence = open ? evidence.filter(item => evidenceMatchesTool(item, tool)) : []
  return <div className="fox-change-entry" ref={row}>
    <button type="button" className="fox-change-trigger" aria-expanded={open} aria-controls={id} onClick={() => setOpen(value => !value)}>
      <ChevronRight className="fox-change-chevron" aria-hidden="true" />
      <Icon aria-hidden="true" />
      <span className="fox-change-target" title={target}>{target}</span>
      <time className="fox-change-time">{time}</time>
      <small className={`fox-side-status is-${tool.status}`} title={status}>{tool.status === 'running' && <LoaderCircle className="motion-safe:animate-spin" aria-hidden="true" />}{status}</small>
    </button>
    <div id={id} className="fox-change-expanded" hidden={!open}>
      {open && <>
        {developerUi && <p className="fox-side-note">{tool.toolName}</p>}
        {command ? <CommandDetails tool={tool} target={target} /> : <FileChangeDetails tool={tool} target={target} approvals={approvals} />}
        {linkedEvidence.length > 0 && <FoxDisclosure summary={`${linkedEvidence.length} 条证据`} defaultOpen={Boolean(focusedEvidence)}>
          <div className="fox-linked-evidence">{linkedEvidence.map(item => <button type="button" key={item.id} disabled={!onOpenEvidence} onClick={() => onOpenEvidence?.(item)}><ShieldCheck size={12} aria-hidden="true" /><span>{item.summary}</span><small>{item.validityStatus}</small></button>)}</div>
        </FoxDisclosure>}
      </>}
    </div>
  </div>
}

function CommandDetails({ tool, target }: { tool: ToolCallRecord; target: string }) {
  const command = commandOutput(tool)
  return <div className="fox-command-detail">
    <dl className="fox-detail-facts">
      <dt>命令</dt><dd>{target}</dd>
      <dt>结果</dt><dd className={tool.status === 'failed' || command.exitCode !== null && command.exitCode !== 0 ? 'is-error' : undefined}>
        {toolStatusLabel(tool, toolExecutionStatus(tool))}{command.exitCode !== null && ` · 退出码 ${command.exitCode}`}
        {command.execution?.requiresAttention && ` · ${command.execution.uncertain ? '结果未知，未自动重试' : '已拒绝'}`}
      </dd>
      <dt>耗时</dt><dd>{command.duration === null ? tool.status === 'running' ? '进行中' : '—' : formatDurationMs(command.duration)}</dd>
      <dt>目录</dt><dd>{command.cwd || '—'}</dd>
    </dl>
    <ScrollArea className="fox-command-output"><pre>{command.output || tool.errorMessage || (tool.status === 'running' ? '等待命令输出…' : '命令没有返回输出。')}</pre></ScrollArea>
  </div>
}

function FileChangeDetails({ tool, target, approvals }: { tool: ToolCallRecord; target: string; approvals: ApprovalRecord[] }) {
  const lines = toolDiff(tool, approvals).split('\n')
  const additions = lines.filter(line => line.startsWith('+') && !line.startsWith('+++ ')).length
  const removals = lines.filter(line => line.startsWith('-') && !line.startsWith('--- ')).length
  return <>
    <dl className="fox-detail-facts"><dt>文件</dt><dd>{target}</dd><dt>改动</dt><dd>新增 {additions} 行 · 删除 {removals} 行</dd></dl>
    <ScrollArea className="fox-diff-scroll"><code>{lines.map((line, index) => {
      const tone = line.startsWith('@@') || line.startsWith('+++ ') || line.startsWith('--- ') ? 'meta'
        : line.startsWith('+') ? 'added' : line.startsWith('-') ? 'removed' : 'context'
      return <span key={index} className={`is-${tone}`}><i>{index + 1}</i><b>{line || ' '}</b></span>
    })}</code></ScrollArea>
  </>
}

function toolDiff(tool: ToolCallRecord, approvals: ApprovalRecord[]) {
  const approval = approvals.find(item => item.toolCallId === tool.id)
  if (approval?.request.diff) return approval.request.diff
  const input = tool.input && typeof tool.input === 'object' && !Array.isArray(tool.input)
    ? tool.input as Record<string, unknown>
    : {}
  if (typeof input.content === 'string') return input.content.split('\n').map(line => `+ ${line}`).join('\n')
  if (typeof input.oldText === 'string' || typeof input.newText === 'string') {
    return [
      ...String(input.oldText ?? '').split('\n').map(line => `- ${line}`),
      ...String(input.newText ?? '').split('\n').map(line => `+ ${line}`),
    ].join('\n')
  }
  return JSON.stringify(tool.input, null, 2) ?? ''
}
