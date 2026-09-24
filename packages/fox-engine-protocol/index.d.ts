// Generated from fox-engine-protocol Rust DTOs. Do not edit; run node scripts/generate-engine-protocol.mjs.
export type ActionClass = ("read" | "write" | "execute" | "destructive" | "sensitive_egress" | "manage")
export type BackendRequirement = { "evidenceDigest"?: (string | null); "required": string; }
export type HostObservation = { "coveredWholeFile"?: boolean; "observedByToolCallId": string; "rangeEnd"?: (number | null); "rangeStart"?: (number | null); "targetIdentity": string; "totalUnits"?: (number | null); "truncated"?: boolean; "version": string; "viewKind"?: ObservationView; }
export type ObservationView = ("full_file" | "line_range" | "unit_window" | "office_extract" | "legacy_unknown" | "missing")
export type RealtimeRequirement = ("policy_version" | "parent_revocation" | "resource_grant" | "cancellation" | "backend_evidence" | "host_observation")
export type ExecutionCredential = { "actionClass": ActionClass; "backendRequirement": BackendRequirement; "budgetCeilingMs": number; "conversationId": string; "credentialDigest": string; "dispatchId": string; "fileBaseline"?: (HostObservation | null); "intentDigest": string; "parentRevocationGeneration"?: (number | null); "policySnapshotId": string; "policyVersion"?: (number | null); "realtimeRequirements": Array<RealtimeRequirement>; "replaceCandidateDigest"?: (string | null); "replaceRequestDigest"?: (string | null); "requiresReplaceGrant"?: boolean; "resolvedProfile": string; "runId": string; }
export type ExecutionEvidence = ("not_started" | "started" | "unknown")
export type ControlPlaneState = "none" | "prepared" | "applying" | "committed" | "not_applied" | "uncertain" | "unknown"
export type ExecutionKind = "process" | "file"
export type ExecutionStage = "admitted" | "job_created" | "launch_confirmed" | "interrupted" | "file_prepared" | "file_applying" | "file_committed" | "file_not_applied" | "file_recovery_required" | "file_indeterminate"
export type SideEffectState = "none" | "prepared" | "applying" | "committed" | "not_applied" | "uncertain" | "unknown"
export type TriState = "true" | "false" | "unknown"
export type ExecutionReceipt = { "allowReplay": boolean; "codeAlias"?: (string | null); "controlPlane": ControlPlaneState; "dispatchId": string; "executionStarted": TriState; "externalEffect": SideEffectState; "kind": ExecutionKind; "reasonCode"?: (string | null); "resumable": boolean; "stage": ExecutionStage; }
export type HostResponse = { "conversationId"?: (string | null); "id": string; "kind": string; "payload": unknown; "protocol": string; "requestId": string; "runId"?: (string | null); "runtimeSessionId"?: (string | null); "timestamp": string; "type": string; "version": number; }
export type HostJobNotice = { "attempt": number; "conversationId": string; "dataRootId": string; "errorCode"?: (string | null); "finishedAt": number; "jobId": string; "resultBytes"?: (number | null); "resultRef"?: (string | null); "resultSha256"?: (string | null); "runId": string; "source": string; "terminalState": string; }
export type KernelSettledToolResult = { "canonicalInput": unknown; "result": unknown; "sourceOrder": number; "state": KernelSettledToolState; "storage"?: unknown; "tool": string; "toolCallId": string; }
export type KernelSettledToolState = "completed" | "failed"
export type KernelSteeringNotice = { "content": string; "messageId": string; "receivedAt"?: (number | null); }
export type KernelBatchResumeFrame = { "assistantMessage": unknown; "batchId": string; "checkpointSeq": number; "history": Array<unknown>; "hostJobNotices"?: Array<HostJobNotice>; "idempotencyKey": string; "schemaVersion": number; "steering"?: Array<KernelSteeringNotice>; "tools": Array<KernelSettledToolResult>; "turnId": string; }
export type KernelCompactionRequest = { "compactionId": string; "inputHash": string; "maxSummaryBytes": number; "messages": Array<unknown>; "runId": string; "schemaVersion": number; "turnId": string; }
export type KernelCompactionResponse = { "compactionId": string; "inputHash": string; "runId": string; "schemaVersion": number; "summary": string; "turnId": string; "usage": unknown; }
export type KernelEngineBatchCheckpoint = { "assistantMessage": unknown; "batchId": string; "history": Array<unknown>; "schemaVersion": number; }
export type KernelInitialModelInput = { "messages": Array<unknown>; "promptConfigHash": string; "runId": string; "schemaVersion": number; "turnId": string; }
export type KernelInitialModelFrame = { "checkpointSeq": number; "continuationKey"?: (string | null); "hostJobNotices"?: Array<HostJobNotice>; "idempotencyKey": string; "input": KernelInitialModelInput; "schemaVersion": number; }
export type KernelInitialModelResponse = { "assistantMessage": unknown; "checkpointSeq": number; "runId": string; "schemaVersion": number; "turnId": string; }
export type KernelModelTelemetry = { "elapsedMs": number; "firstResponseMs"?: (number | null); "idleElapsedMs": number; "reasoningBytes": number; "textBytes": number; "toolParamBytes": number; }
export type KernelModelFailure = { "category": string; "checkpointSeq": number; "httpStatus"?: (number | null); "retryAfterMs"?: (number | null); "runId": string; "schemaVersion": number; "telemetry"?: (KernelModelTelemetry | null); "turnId": string; }
export type KernelModelPreview = { "checkpointSeq": number; "conversationId": string; "progressBytes"?: (number | null); "reasoning"?: string; "revision": number; "runId": string; "schemaVersion": number; "text": string; "turnId": string; }
export type KernelModelResponse = { "assistantMessage": unknown; "batchId": string; "checkpointSeq": number; "runId": string; "schemaVersion": number; "turnId": string; }
export type KernelRoundDirectiveKind = "batch" | "continuation" | "final"
export type KernelRoundDirective = { "batchId"?: (string | null); "checkpointSeq"?: (number | null); "hostJobNotices"?: Array<HostJobNotice>; "kind": KernelRoundDirectiveKind; "previewSeq"?: (number | null); "prompt"?: (string | null); "schemaVersion": number; "steering"?: Array<KernelSteeringNotice>; "tools"?: Array<KernelSettledToolResult>; }
export type KernelRoundOutputFrame = { "assistantMessage": unknown; "schemaVersion": number; "turnId": string; }
export type KernelToolSnapshot = { "approvalState"?: (string | null); "batchId": string; "sourceOrder": number; "state": string; "tool": string; "toolCallId": string; }
export type KernelRunSnapshot = { "approvalDeadlineWallMs"?: (number | null); "compactions": number; "engineId": string; "lastEventSeq": string; "providerAttempts": number; "retryDueWallMs"?: (number | null); "runId": string; "runningElapsedMs": number; "schemaVersion": number; "state": string; "terminalWritten": boolean; "tools": Array<KernelToolSnapshot>; "turnAttempts": number; "turnId": string; }
export type KernelStateInvalidation = { "schemaVersion": number; }
export type ExecutionAuthority = "legacy" | "authoritative"
export type FrozenPermission = { "approvalEpoch"?: (number | null); "grants": Array<PermissionGrant>; "mode": PermissionMode; "projectRoot"?: (string | null); }
export type GrantKind = ("resource" | "approval_reuse")
export type PermissionGrant = { "kind"?: GrantKind; "scope": string; "tool": string; }
export type PermissionMode = "ask" | "read_only" | "allow"
export type ResourceExecutor = "runtime" | "rust"
export type TimeBudgets = { "approvalWaitMs": number; "modelFirstResponseMs"?: number; "modelIdleMs"?: number; "modelRequestMs": number; "runExecutionLimited"?: boolean; "runExecutionMs": number; "toolExecutionMs": number; }
export type RunControlBinding = { "authority": ExecutionAuthority; "budgets": TimeBudgets; "conversationId": string; "engineId": string; "executionProfileId": string; "permission": FrozenPermission; "permissionSnapshotId": string; "readOnlyExecutor": ResourceExecutor; "runId": string; "schemaVersion": number; }
export type RuntimeToolCapability = { "approval": string; "category": string; "execution": string; "name": string; }
export type RuntimeCapabilityManifest = { "cancellation": boolean; "contextCompaction": boolean; "dynamicModelSwitch": boolean; "imageInput": boolean; "manifestVersion": number; "reasoning": boolean; "sessionResume": boolean; "steering": boolean; "streamingText": boolean; "toolApproval": boolean; "tools": Array<RuntimeToolCapability>; "workLoop"?: boolean; }
export type RuntimeEnvelope = { "conversationId"?: (string | null); "id": string; "kind": string; "payload"?: unknown; "protocol": string; "requestId"?: (string | null); "runId"?: (string | null); "runtimeSessionId"?: (string | null); "seq"?: (number | null); "timestamp": string; "type": string; "version": number; }
export type RuntimeRequest = { "conversationId"?: (string | null); "id": string; "kind": string; "payload"?: unknown; "protocol": string; "runId"?: (string | null); "runtimeSessionId"?: (string | null); "timestamp": string; "type": string; "version": number; }

export declare const PROTOCOL_NAME: "fox-runtime-jsonl"
export declare const PROTOCOL_VERSION: 1
export declare const CAPABILITY_MANIFEST_VERSION: 2
export declare const RUNTIME_TOOL_CATALOG: ReadonlyArray<Readonly<RuntimeToolCapability>>
export declare function validateWireValue(schemaName: string, value: unknown): string[]
