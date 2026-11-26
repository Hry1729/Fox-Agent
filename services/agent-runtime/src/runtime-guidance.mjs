const DATA_GUIDANCE = `Preview the sheets and columns, then use attachment_compute for full-table totals and rankings. Use attachmentIds for uploads and projectPaths for files in the authorized folder. If the synchronous limit is too small, use compute_job_start with chunked processing and wait for its result. When a request fails, use the reported error to correct the input or choose another authorized method. If neither works, state which sheets or rows you covered, what stopped the work and what you need to continue. Do not estimate totals from a preview.
Call ask_user alone when the calculation depends on a choice. Wait for the reply before making those tool calls.
For structured CSV, use the source workbook's actual epoch. dateKey preserves an ISO calendar day without a timezone and uses UTC for explicit offsets or Date values. Do not reinterpret calendar text in local time or turn bare serials or arbitrary objects into dates. Use saveFile for ordinary quoted text.
For source-backed metric,key,value CSV include all requested totals and dates. Do not invent a denominator or claim unsupported cross-file verification. An overall pie must cite the authorized input, sheet and denominator column; the Host checks the source and renders the remainder. Export metadata alone cannot certify it.
Keep intermediate files in the private compute workspace. Publish each requested artifact at its Host-resolved target with write_file, so the saved version and source snapshot can be verified.`

const WORK_GUIDANCE = `- Fox injects the current persisted Goal, Task, and Evidence snapshot before every turn. Use work_snapshot_get without arguments only when you need to refresh it after a work-tool call.
- Never create Tasks under a proposed or blocked Goal. If goal_propose returns a proposed Goal, stop work and wait for Host confirmation. When Fox resumes the Run with an active Goal, create the ordered Tasks then continue.
- Persist the initial execution plan and every material change with plan_revision_create. Each revision contains the complete ordered plan and starts as proposed. Stop state-changing work after proposing it; only the user-facing Fox Host can approve or reject a plan.
- Before final acceptance, refresh Work State, confirm every Task is completed or skipped, validate at least one Evidence record for every completed Task, and conduct a separate review pass in a different Run. Bind every review_finding_add call to the approved planRevisionId; resolve addressed findings with review_finding_resolve.
- Independent review and acceptance are A1 capabilities. Reviewer identity is the actual Run ID supplied by Fox Host, never a name supplied by the model. Persist ReviewFinding and Acceptance records rather than asserting review in assistant prose.
- Prefer acceptance_submit for A1 completion. Submit explicit checks with criterion, method, status, and evidenceIds. Fox Host requires the current approved PlanRevision, a review made by this independent Run, no open critical/high/medium finding, terminal Tasks, valid Evidence covering every completed Task, and the current Goal version. Use goal_complete only for legacy A0 conversations without an A1 plan.
- Completion and acceptance tools are requests, not self-issued verdicts. Fox Host remains authoritative. Never claim completion unless Fox returns the persisted Goal with status completed.
- Treat gate decisions and tool troubleshooting as internal control state. Do not narrate attempted activation, gate retries, status speculation, or tool inventories in the assistant reply.
- Only claim that a Goal or Task exists after the corresponding tool returned the persisted record. Always use the returned UUID and optimistic version; never invent numeric task IDs.
- Coding Agent extension state is not a second source of truth. Goal, Task, Evidence, approval, and project authorization state always comes from Fox Host.`

const WORKFLOW_GUIDANCE = `- Resume a persisted Workflow from workflow_snapshot_get, including its current Stage and gate. Start a new one with workflow_start only when the selected expert declares it and the user request matches; supply schema-valid input.
- Workflow Stages are durable checkpoints mapped one-to-one to the Host Goal Tasks. Never create duplicate Goal or Task records for a Workflow and never skip ahead of currentStageIndex.
- If a Stage is awaiting_gate, stop and wait for the user-facing Host to approve or reject it. The model cannot approve its own Gate.
- Call workflow_stage_start before doing Stage work. Add and validate Task Evidence through the ordinary work tools, then call workflow_stage_complete with schema-conforming stageOutput. The final Stage also supplies workflowOutput.
- Call workflow_stage_fail only for a real failed attempt. Fox owns retry counts and the terminal failure transition; do not loop after the retry budget is exhausted.
- A persisted Workflow checkpoint, Goal, Task, Evidence, Gate, or output is authoritative only after the corresponding Host tool returns it. Workflow permissions remain bounded by normal Host authorization.`

const TEAM_GUIDANCE = `- Resume an existing Team from team_snapshot_get. Only the Supervisor/Lead of a declared Team may call team_start once, dispatch declared members one at a time with team_member_start, collect each terminal result before dispatching the next, and use team_collect for the final Host-owned aggregate. Members never see the parent transcript unless you pass bounded context explicitly.`

const TABULAR_FILE = /\.(?:xls[xmb]?|xlam|ods|csv|tsv)(?!\w|\.\w)/iu

export function scopedRuntimeGuidance(context = {}, turn = {}) {
  const sections = []
  if (context.companionContext?.role === 'owner' && turn.reportDeliveryAvailable) {
    sections.push(['companion_report_delivery', 'If the result needs more than a few short chat paragraphs, put the full result in a file before sending the final reply. Use the requested format and location. When unspecified, write a descriptive Markdown (.md) report in the authorized output folder. Use write_file to save Markdown and present_files to attach the saved result. Once the file is declared, give the conclusion and essential findings in one to three short paragraphs, without repeating the report. If saving or declaring the file fails, report that accurately and retain the findings.'])
  }
  const run = context.runContext
  const manifest = context.expertPackage?.packageManifest
  const workflow = Boolean(manifest?.workflow || run?.activeExpertWorkflow)
  const team = Boolean(manifest?.team || run?.activeExpertTeam)
  const snapshot = context.workSnapshot
  const goal = snapshot?.goal
  const dataTask = run?.tabularInput || TABULAR_FILE.test(turn.text ?? '')
    || turn.attachmentExtensions?.some(extension => TABULAR_FILE.test(`.${extension}`))
    || TABULAR_FILE.test(context.companionCoordination?.incoming?.message ?? '')
    || context.filePlacement?.deliveryTargets?.targets?.some(target =>
      TABULAR_FILE.test(target.path ?? '') || target.itemKey?.startsWith('slot:csv:'))
  if (dataTask && turn.computeAvailable === false && context.companionContext?.role === 'owner') {
    sections.push(['data_preview_only', 'Use an authorized full-table method if one is available. If you can only preview the spreadsheet, say which sheets and rows you inspected and that the full-table result is unavailable. Never present sample totals or rankings as workbook-wide results.'])
  } else if (turn.computeAvailable !== false) {
    if (dataTask) sections.push(['data_guidance', DATA_GUIDANCE])
    else if (turn.priorComputation) {
      sections.push(['data_continuation', 'If continuing the preceding calculation, reuse its verified inputs or returned artifactIds. Check the requested denominator against the source.'])
    }
  }
  const ongoingWork = goal ? !['completed', 'cancelled'].includes(goal.status)
    : snapshot?.tasks?.length || snapshot?.evidence?.length
  if (ongoingWork || run?.activeExpertWorkflow) {
    sections.push(['work_guidance', WORK_GUIDANCE])
  }
  if (workflow) sections.push(['workflow_guidance', WORKFLOW_GUIDANCE])
  if (team) sections.push(['team_guidance', TEAM_GUIDANCE])
  return sections
}
