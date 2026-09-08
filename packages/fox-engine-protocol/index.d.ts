// Generated from fox-engine-protocol Rust DTOs. Do not edit; run node scripts/generate-engine-protocol.mjs.
export type HostResponse = { "conversationId"?: (string | null); "id": string; "kind": string; "payload": unknown; "protocol": string; "requestId": string; "runId"?: (string | null); "runtimeSessionId"?: (string | null); "timestamp": string; "type": string; "version": number; }
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
