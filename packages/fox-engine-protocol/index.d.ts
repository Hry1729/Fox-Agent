// Generated from fox-engine-protocol Rust DTOs. Do not edit; run node scripts/generate-engine-protocol.mjs.
export type HostResponse = { "conversationId"?: (string | null); "id": string; "kind": string; "payload": unknown; "protocol": string; "requestId": string; "runId"?: (string | null); "runtimeSessionId"?: (string | null); "timestamp": string; "type": string; "version": number; }
export type KernelSettledToolResult = { "canonicalInput": unknown; "result": unknown; "sourceOrder": number; "state": KernelSettledToolState; "tool": string; "toolCallId": string; }
export type KernelSettledToolState = "completed" | "failed"
export type KernelBatchResumeFrame = { "assistantMessage": unknown; "batchId": string; "checkpointSeq": number; "history": Array<unknown>; "idempotencyKey": string; "schemaVersion": number; "tools": Array<KernelSettledToolResult>; "turnId": string; }
export type KernelCompactionRequest = { "compactionId": string; "inputHash": string; "maxSummaryBytes": number; "messages": Array<unknown>; "runId": string; "schemaVersion": number; "turnId": string; }
export type KernelCompactionResponse = { "compactionId": string; "inputHash": string; "runId": string; "schemaVersion": number; "summary": string; "turnId": string; "usage": unknown; }
export type KernelEngineBatchCheckpoint = { "assistantMessage": unknown; "batchId": string; "history": Array<unknown>; "schemaVersion": number; }
export type KernelInitialModelInput = { "messages": Array<unknown>; "promptConfigHash": string; "runId": string; "schemaVersion": number; "turnId": string; }
export type KernelInitialModelFrame = { "checkpointSeq": number; "idempotencyKey": string; "input": KernelInitialModelInput; "schemaVersion": number; }
export type KernelInitialModelResponse = { "assistantMessage": unknown; "checkpointSeq": number; "runId": string; "schemaVersion": number; "turnId": string; }
export type KernelModelFailure = { "category": string; "checkpointSeq": number; "httpStatus"?: (number | null); "retryAfterMs"?: (number | null); "runId": string; "schemaVersion": number; "turnId": string; }
export type KernelModelPreview = { "checkpointSeq": number; "conversationId": string; "revision": number; "runId": string; "schemaVersion": number; "text": string; "turnId": string; }
export type KernelModelResponse = { "assistantMessage": unknown; "batchId": string; "checkpointSeq": number; "runId": string; "schemaVersion": number; "turnId": string; }
export type KernelToolSnapshot = { "approvalState"?: (string | null); "batchId": string; "sourceOrder": number; "state": string; "tool": string; "toolCallId": string; }
export type KernelRunSnapshot = { "approvalDeadlineWallMs"?: (number | null); "compactions": number; "engineId": string; "lastEventSeq": string; "providerAttempts": number; "retryDueWallMs"?: (number | null); "runId": string; "runningElapsedMs": number; "schemaVersion": number; "state": string; "terminalWritten": boolean; "tools": Array<KernelToolSnapshot>; "turnAttempts": number; "turnId": string; }
export type KernelStateInvalidation = { "schemaVersion": number; }
export type ExecutionAuthority = "legacy" | "authoritative"
export type FrozenPermission = { "grants": Array<PermissionGrant>; "mode": PermissionMode; "projectRoot"?: (string | null); }
export type PermissionGrant = { "scope": string; "tool": string; }
export type PermissionMode = "ask" | "read_only" | "allow"
export type ResourceExecutor = "runtime" | "rust"
export type TimeBudgets = { "approvalWaitMs": number; "modelRequestMs": number; "runExecutionMs": number; "toolExecutionMs": number; }
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
