// Generated from fox-engine-protocol Rust DTOs. Do not edit; run node scripts/generate-engine-protocol.mjs.
import { validateSchema } from './validate.mjs'
export const SCHEMA_BUNDLE = {
  "capabilityManifestVersion": 2,
  "protocolName": "fox-runtime-jsonl",
  "protocolVersion": 1,
  "schemaVersion": 1,
  "schemas": {
    "ExecutionCredential": {
      "$defs": {
        "ActionClass": {
          "description": "Action classification of one admitted operation.",
          "oneOf": [
            {
              "enum": [
                "read",
                "write",
                "execute",
                "destructive",
                "sensitive_egress"
              ],
              "type": "string"
            },
            {
              "const": "manage",
              "description": "Safe management surface (status/output/cancel of an already-admitted\njob). It never starts an external process, so it does not require a\nverified execution backend; scope checks still apply.",
              "type": "string"
            }
          ]
        },
        "BackendRequirement": {
          "additionalProperties": false,
          "description": "Backend requirement bound into a credential. `evidence_digest` is `None`\nuntil a backend has actually been verified; it is never synthesized.",
          "properties": {
            "evidenceDigest": {
              "type": [
                "string",
                "null"
              ]
            },
            "required": {
              "type": "string"
            }
          },
          "required": [
            "required"
          ],
          "type": "object"
        },
        "HostObservation": {
          "additionalProperties": false,
          "description": "An authoritative Host observation of a file target: which durable Host read\nrecord saw which version of which target. A model-declared\n`expectedVersion` is a *precondition claim*, never an observation.\n\nREV-05: the record also carries **what was actually delivered**. Reading one\nline establishes a line-range observation of that version, not a whole-file\none, and an extracted Office view is never the document's source text. The\nfields default to the most conservative values so an older persisted\ncredential (which predates them) can never gain whole-file replacement\neligibility it did not have.",
          "properties": {
            "coveredWholeFile": {
              "default": false,
              "description": "True only when the Host delivered the entire content of `version`.",
              "type": "boolean"
            },
            "observedByToolCallId": {
              "description": "The durable Host tool-call record that produced this observation.",
              "type": "string"
            },
            "rangeEnd": {
              "default": null,
              "format": "uint64",
              "minimum": 0,
              "type": [
                "integer",
                "null"
              ]
            },
            "rangeStart": {
              "default": null,
              "description": "Delivered range in UTF-16 code units, the single coordinate system the\nread tool's `offset`/`limit`/`nextOffset` already use. `None` for views\nwith no text range (Office extract, missing file).",
              "format": "uint64",
              "minimum": 0,
              "type": [
                "integer",
                "null"
              ]
            },
            "targetIdentity": {
              "type": "string"
            },
            "totalUnits": {
              "default": null,
              "description": "Size of the whole text of `version` in UTF-16 code units.",
              "format": "uint64",
              "minimum": 0,
              "type": [
                "integer",
                "null"
              ]
            },
            "truncated": {
              "default": false,
              "description": "The delivered page was itself cut short (output or read budget).",
              "type": "boolean"
            },
            "version": {
              "type": "string"
            },
            "viewKind": {
              "$ref": "#/$defs/ObservationView",
              "default": "legacy_unknown",
              "description": "Classification label only. It never expresses coverage strength; the\nconcrete `range_start`/`range_end` and `covered_whole_file` do."
            }
          },
          "required": [
            "targetIdentity",
            "version",
            "observedByToolCallId"
          ],
          "type": "object"
        },
        "ObservationView": {
          "description": "What kind of view produced an observation. A label, not a coverage claim.",
          "oneOf": [
            {
              "const": "full_file",
              "description": "The Host delivered the whole file content for this version.",
              "type": "string"
            },
            {
              "const": "line_range",
              "description": "A line range (`startLine`/`lineCount`).",
              "type": "string"
            },
            {
              "const": "unit_window",
              "description": "A UTF-16 code-unit window (`offset`/`limit`).",
              "type": "string"
            },
            {
              "const": "office_extract",
              "description": "Text extracted from a binary/Office container; not the source text.",
              "type": "string"
            },
            {
              "const": "legacy_unknown",
              "description": "A row written before coverage tracking existed. Fail-closed.",
              "type": "string"
            },
            {
              "const": "missing",
              "description": "A verified absent target.",
              "type": "string"
            }
          ]
        },
        "RealtimeRequirement": {
          "description": "Real-time requirements the executor must re-verify before any side effect.\nA requirement whose authoritative source does not exist yet is *unsatisfied*\n(fail-closed); it is never silently treated as met.",
          "oneOf": [
            {
              "const": "policy_version",
              "description": "Live session policy version (source pending in a later batch).",
              "type": "string"
            },
            {
              "const": "parent_revocation",
              "description": "The parent's current revocation generation.",
              "type": "string"
            },
            {
              "const": "resource_grant",
              "description": "The scope ResourceGrant is still valid.",
              "type": "string"
            },
            {
              "const": "cancellation",
              "description": "Run/tool cancellation state.",
              "type": "string"
            },
            {
              "const": "backend_evidence",
              "description": "A verified execution backend.",
              "type": "string"
            },
            {
              "const": "host_observation",
              "description": "An authoritative Host observation of the file target/version.",
              "type": "string"
            }
          ]
        }
      },
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "description": "The immutable execution credential issued once per dispatch inside the\nfinal admission transaction. The model never constructs it: the Host\nderives every field from durable facts.",
      "properties": {
        "actionClass": {
          "$ref": "#/$defs/ActionClass"
        },
        "backendRequirement": {
          "$ref": "#/$defs/BackendRequirement"
        },
        "budgetCeilingMs": {
          "description": "Budget bound at issue time (milliseconds). The executor may only\ntighten it, never widen it.",
          "format": "int64",
          "type": "integer"
        },
        "conversationId": {
          "type": "string"
        },
        "credentialDigest": {
          "type": "string"
        },
        "dispatchId": {
          "type": "string"
        },
        "fileBaseline": {
          "anyOf": [
            {
              "$ref": "#/$defs/HostObservation"
            },
            {
              "type": "null"
            }
          ],
          "description": "Authoritative Host observation of the file target. `None` means no\nobservation existed at issue time — a write-class action must then be\nrefused, never executed against a model-declared version."
        },
        "intentDigest": {
          "description": "Digest over the frozen tool + canonical input (what was admitted).",
          "type": "string"
        },
        "parentRevocationGeneration": {
          "description": "The parent's revocation generation observed at issue time. `None` = no\nparent fact (root run); a child must re-read the parent's *current*\ngeneration at execution.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "policySnapshotId": {
          "description": "Durable policy identity bound at issue time (the frozen permission\nsnapshot id).",
          "type": "string"
        },
        "policyVersion": {
          "description": "Live session policy version. `None` = no such durable fact exists yet;\nthe live-version re-verification requirement stays listed and\nunsatisfied rather than being silently skipped.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "realtimeRequirements": {
          "description": "What the executor must re-verify in real time before any side effect.",
          "items": {
            "$ref": "#/$defs/RealtimeRequirement"
          },
          "type": "array"
        },
        "replaceCandidateDigest": {
          "default": null,
          "type": [
            "string",
            "null"
          ]
        },
        "replaceRequestDigest": {
          "default": null,
          "type": [
            "string",
            "null"
          ]
        },
        "requiresReplaceGrant": {
          "default": false,
          "type": "boolean"
        },
        "resolvedProfile": {
          "type": "string"
        },
        "runId": {
          "type": "string"
        }
      },
      "required": [
        "dispatchId",
        "runId",
        "conversationId",
        "intentDigest",
        "actionClass",
        "resolvedProfile",
        "policySnapshotId",
        "budgetCeilingMs",
        "realtimeRequirements",
        "backendRequirement",
        "credentialDigest"
      ],
      "title": "ExecutionCredential",
      "type": "object"
    },
    "ExecutionEvidence": {
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "description": "What the executor reports about the target operation it just ran. This is\nthe Host-internal trusted evidence channel: it is produced by the code that\nactually performed (or refused) the operation, NEVER inferred from the shape\nof a tool result.\n\nA Rust `Ok(..)` says nothing here: Fox tools return `Ok(json)` with\n`isError=true` for business failures, and a successful read/query is not the\nstart of an external process.",
      "oneOf": [
        {
          "const": "not_started",
          "description": "The executor refused or completed the call WITHOUT starting the target\noperation (pre-execution refusal, validation failure, a read/query that\nfinished). The attempt is terminal and no start fact exists.",
          "type": "string"
        },
        {
          "const": "started",
          "description": "The executor has positive evidence that the target operation started\n(a recorded applying journal, a spawned process identity, ...). Only the\nexecutor can assert this.",
          "type": "string"
        },
        {
          "const": "unknown",
          "description": "The outcome of an already-started operation is unknown (crash,\ninterruption, lost transport). Never replayed.",
          "type": "string"
        }
      ],
      "title": "ExecutionEvidence"
    },
    "ExecutionReceipt": {
      "$defs": {
        "ControlPlaneState": {
          "description": "Control-plane persistence state, enumerated per stage.",
          "enum": [
            "none",
            "prepared",
            "applying",
            "committed",
            "not_applied",
            "uncertain",
            "unknown"
          ],
          "type": "string"
        },
        "ExecutionKind": {
          "description": "Execution family: file and process stages are expressed separately.",
          "enum": [
            "process",
            "file"
          ],
          "type": "string"
        },
        "ExecutionStage": {
          "description": "Stage enumeration. Process and file stages never mix.",
          "enum": [
            "admitted",
            "job_created",
            "launch_confirmed",
            "interrupted",
            "file_prepared",
            "file_applying",
            "file_committed",
            "file_not_applied",
            "file_recovery_required",
            "file_indeterminate"
          ],
          "type": "string"
        },
        "SideEffectState": {
          "description": "External (target) side-effect state, kept separate from the control plane.",
          "enum": [
            "none",
            "prepared",
            "applying",
            "committed",
            "not_applied",
            "uncertain",
            "unknown"
          ],
          "type": "string"
        },
        "TriState": {
          "description": "Three-valued fact: unknown is never collapsed into true or false.",
          "enum": [
            "true",
            "false",
            "unknown"
          ],
          "type": "string"
        }
      },
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "description": "One stage receipt. Illegal kind/stage or state combinations are rejected at\nconstruction, so a receipt can never claim a fact its stage cannot hold.",
      "properties": {
        "allowReplay": {
          "description": "Always false in the first version; kept explicit so wiring cannot\n\"helpfully\" replay a side-effecting attempt.",
          "type": "boolean"
        },
        "codeAlias": {
          "description": "Same-fact alias of an uncertain outcome (`job.interrupted_unknown`).",
          "type": [
            "string",
            "null"
          ]
        },
        "controlPlane": {
          "$ref": "#/$defs/ControlPlaneState"
        },
        "dispatchId": {
          "type": "string"
        },
        "executionStarted": {
          "$ref": "#/$defs/TriState"
        },
        "externalEffect": {
          "$ref": "#/$defs/SideEffectState"
        },
        "kind": {
          "$ref": "#/$defs/ExecutionKind"
        },
        "reasonCode": {
          "type": [
            "string",
            "null"
          ]
        },
        "resumable": {
          "type": "boolean"
        },
        "stage": {
          "$ref": "#/$defs/ExecutionStage"
        }
      },
      "required": [
        "dispatchId",
        "kind",
        "stage",
        "executionStarted",
        "controlPlane",
        "externalEffect",
        "resumable",
        "allowReplay"
      ],
      "title": "ExecutionReceipt",
      "type": "object"
    },
    "HostResponse": {
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "properties": {
        "conversationId": {
          "type": [
            "string",
            "null"
          ]
        },
        "id": {
          "type": "string"
        },
        "kind": {
          "type": "string"
        },
        "payload": true,
        "protocol": {
          "type": "string"
        },
        "requestId": {
          "type": "string"
        },
        "runId": {
          "type": [
            "string",
            "null"
          ]
        },
        "runtimeSessionId": {
          "type": [
            "string",
            "null"
          ]
        },
        "timestamp": {
          "type": "string"
        },
        "type": {
          "type": "string"
        },
        "version": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "protocol",
        "version",
        "kind",
        "id",
        "requestId",
        "timestamp",
        "type",
        "payload"
      ],
      "title": "HostResponse",
      "type": "object"
    },
    "KernelBatchResumeFrame": {
      "$defs": {
        "KernelSettledToolResult": {
          "additionalProperties": false,
          "properties": {
            "canonicalInput": true,
            "result": true,
            "sourceOrder": {
              "format": "uint32",
              "minimum": 0,
              "type": "integer"
            },
            "state": {
              "$ref": "#/$defs/KernelSettledToolState"
            },
            "storage": {
              "description": "Storage facts supplied by the Host, never by the executed tool."
            },
            "tool": {
              "type": "string"
            },
            "toolCallId": {
              "type": "string"
            }
          },
          "required": [
            "toolCallId",
            "tool",
            "canonicalInput",
            "sourceOrder",
            "state",
            "result"
          ],
          "type": "object"
        },
        "KernelSettledToolState": {
          "enum": [
            "completed",
            "failed"
          ],
          "type": "string"
        },
        "KernelSteeringNotice": {
          "additionalProperties": false,
          "description": "One additional user request attached to a round directive.",
          "properties": {
            "content": {
              "type": "string"
            },
            "messageId": {
              "description": "Stable idempotency id of the durable steering row.",
              "type": "string"
            }
          },
          "required": [
            "messageId",
            "content"
          ],
          "type": "object"
        }
      },
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "properties": {
        "assistantMessage": {
          "description": "Original proposal, including provider metadata needed by the engine adapter."
        },
        "batchId": {
          "type": "string"
        },
        "checkpointSeq": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "history": {
          "description": "Complete prior history. Engines must reject rather than synthesize missing results.",
          "items": true,
          "type": "array"
        },
        "idempotencyKey": {
          "type": "string"
        },
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "steering": {
          "description": "Mid-run user requests bound to this (possibly retried) dispatch. The\nengine appends them as ordinary user messages *after* the settled tool\nresults, in seq order. Like the rest of this frame they carry no\nauthority, tools, or grants.",
          "items": {
            "$ref": "#/$defs/KernelSteeringNotice"
          },
          "type": "array"
        },
        "tools": {
          "items": {
            "$ref": "#/$defs/KernelSettledToolResult"
          },
          "type": "array"
        },
        "turnId": {
          "type": "string"
        }
      },
      "required": [
        "schemaVersion",
        "turnId",
        "batchId",
        "idempotencyKey",
        "checkpointSeq",
        "history",
        "assistantMessage",
        "tools"
      ],
      "title": "KernelBatchResumeFrame",
      "type": "object"
    },
    "KernelCompactionRequest": {
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "properties": {
        "compactionId": {
          "type": "string"
        },
        "inputHash": {
          "type": "string"
        },
        "maxSummaryBytes": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "messages": {
          "description": "Only old, plain conversational prose; never tool calls/results or images.",
          "items": true,
          "type": "array"
        },
        "runId": {
          "type": "string"
        },
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "turnId": {
          "type": "string"
        }
      },
      "required": [
        "schemaVersion",
        "runId",
        "turnId",
        "compactionId",
        "inputHash",
        "messages",
        "maxSummaryBytes"
      ],
      "title": "KernelCompactionRequest",
      "type": "object"
    },
    "KernelCompactionResponse": {
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "properties": {
        "compactionId": {
          "type": "string"
        },
        "inputHash": {
          "type": "string"
        },
        "runId": {
          "type": "string"
        },
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "summary": {
          "type": "string"
        },
        "turnId": {
          "type": "string"
        },
        "usage": {
          "description": "Bounded provider usage, accounted separately from visible chat messages."
        }
      },
      "required": [
        "schemaVersion",
        "runId",
        "turnId",
        "compactionId",
        "inputHash",
        "summary",
        "usage"
      ],
      "title": "KernelCompactionResponse",
      "type": "object"
    },
    "KernelEngineBatchCheckpoint": {
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "properties": {
        "assistantMessage": true,
        "batchId": {
          "type": "string"
        },
        "history": {
          "items": true,
          "type": "array"
        },
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "schemaVersion",
        "batchId",
        "history",
        "assistantMessage"
      ],
      "title": "KernelEngineBatchCheckpoint",
      "type": "object"
    },
    "KernelInitialModelFrame": {
      "$defs": {
        "KernelInitialModelInput": {
          "additionalProperties": false,
          "description": "Host-owned initial prompt snapshot, not a dispatch request or permission.\nA separate durable dispatch intent is required before invoking an engine.",
          "properties": {
            "messages": {
              "items": true,
              "type": "array"
            },
            "promptConfigHash": {
              "type": "string"
            },
            "runId": {
              "type": "string"
            },
            "schemaVersion": {
              "format": "uint32",
              "minimum": 0,
              "type": "integer"
            },
            "turnId": {
              "type": "string"
            }
          },
          "required": [
            "schemaVersion",
            "runId",
            "turnId",
            "promptConfigHash",
            "messages"
          ],
          "type": "object"
        }
      },
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "properties": {
        "checkpointSeq": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "continuationKey": {
          "description": "Present when a fresh worker restores a durably leased continuation input.",
          "type": [
            "string",
            "null"
          ]
        },
        "idempotencyKey": {
          "type": "string"
        },
        "input": {
          "$ref": "#/$defs/KernelInitialModelInput"
        },
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "schemaVersion",
        "input",
        "idempotencyKey",
        "checkpointSeq"
      ],
      "title": "KernelInitialModelFrame",
      "type": "object"
    },
    "KernelInitialModelInput": {
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "description": "Host-owned initial prompt snapshot, not a dispatch request or permission.\nA separate durable dispatch intent is required before invoking an engine.",
      "properties": {
        "messages": {
          "items": true,
          "type": "array"
        },
        "promptConfigHash": {
          "type": "string"
        },
        "runId": {
          "type": "string"
        },
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "turnId": {
          "type": "string"
        }
      },
      "required": [
        "schemaVersion",
        "runId",
        "turnId",
        "promptConfigHash",
        "messages"
      ],
      "title": "KernelInitialModelInput",
      "type": "object"
    },
    "KernelInitialModelResponse": {
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "properties": {
        "assistantMessage": true,
        "checkpointSeq": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "runId": {
          "type": "string"
        },
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "turnId": {
          "type": "string"
        }
      },
      "required": [
        "schemaVersion",
        "runId",
        "turnId",
        "checkpointSeq",
        "assistantMessage"
      ],
      "title": "KernelInitialModelResponse",
      "type": "object"
    },
    "KernelModelFailure": {
      "$defs": {
        "KernelModelTelemetry": {
          "additionalProperties": false,
          "description": "Byte/time counters for one failed model round. All values are worker-side\nobservations for diagnosis; Host timeout anchors remain authoritative.",
          "properties": {
            "elapsedMs": {
              "format": "int64",
              "type": "integer"
            },
            "firstResponseMs": {
              "format": "int64",
              "type": [
                "integer",
                "null"
              ]
            },
            "idleElapsedMs": {
              "format": "int64",
              "type": "integer"
            },
            "reasoningBytes": {
              "format": "uint64",
              "minimum": 0,
              "type": "integer"
            },
            "textBytes": {
              "format": "uint64",
              "minimum": 0,
              "type": "integer"
            },
            "toolParamBytes": {
              "format": "uint64",
              "minimum": 0,
              "type": "integer"
            }
          },
          "required": [
            "elapsedMs",
            "idleElapsedMs",
            "textBytes",
            "reasoningBytes",
            "toolParamBytes"
          ],
          "type": "object"
        }
      },
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "description": "Sanitized evidence of a settled model failure, never a permission to retry.\nHost still owns retry admission, counters, delay, lease and cancellation.",
      "properties": {
        "category": {
          "type": "string"
        },
        "checkpointSeq": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "httpStatus": {
          "format": "uint16",
          "maximum": 65535,
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "retryAfterMs": {
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "runId": {
          "type": "string"
        },
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "telemetry": {
          "anyOf": [
            {
              "$ref": "#/$defs/KernelModelTelemetry"
            },
            {
              "type": "null"
            }
          ],
          "description": "Low-sensitivity progress telemetry captured by the worker when the\nround failed. Never contains credentials, model input, or complete\ntool arguments: only elapsed times and output byte counts, so Host can\ndistinguish slow generation from a stalled stream. Absent for old\nworkers and for non-timeout categories."
        },
        "turnId": {
          "type": "string"
        }
      },
      "required": [
        "schemaVersion",
        "runId",
        "turnId",
        "checkpointSeq",
        "category"
      ],
      "title": "KernelModelFailure",
      "type": "object"
    },
    "KernelModelPreview": {
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "description": "Display content only. Host may persist partial text; never a decision or replay input.",
      "properties": {
        "checkpointSeq": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "conversationId": {
          "type": "string"
        },
        "progressBytes": {
          "description": "Cumulative output bytes observed by the worker this round, including\ntool-parameter bytes that never appear in `text`. Lets Host tell slow\ngeneration apart from a stalled stream without receiving the content.\nAbsent for old workers; display code must treat absence as unknown.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "reasoning": {
          "type": "string"
        },
        "revision": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "runId": {
          "type": "string"
        },
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "text": {
          "type": "string"
        },
        "turnId": {
          "type": "string"
        }
      },
      "required": [
        "schemaVersion",
        "runId",
        "conversationId",
        "turnId",
        "checkpointSeq",
        "revision",
        "text"
      ],
      "title": "KernelModelPreview",
      "type": "object"
    },
    "KernelModelResponse": {
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "description": "One model response to an identified, durable batch delivery. History and\nthe next batch identity remain Host-owned and are never supplied by Node.",
      "properties": {
        "assistantMessage": true,
        "batchId": {
          "type": "string"
        },
        "checkpointSeq": {
          "format": "uint64",
          "minimum": 0,
          "type": "integer"
        },
        "runId": {
          "type": "string"
        },
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "turnId": {
          "type": "string"
        }
      },
      "required": [
        "schemaVersion",
        "runId",
        "turnId",
        "batchId",
        "checkpointSeq",
        "assistantMessage"
      ],
      "title": "KernelModelResponse",
      "type": "object"
    },
    "KernelRoundDirective": {
      "$defs": {
        "KernelRoundDirectiveKind": {
          "enum": [
            "batch",
            "continuation",
            "final"
          ],
          "type": "string"
        },
        "KernelSettledToolResult": {
          "additionalProperties": false,
          "properties": {
            "canonicalInput": true,
            "result": true,
            "sourceOrder": {
              "format": "uint32",
              "minimum": 0,
              "type": "integer"
            },
            "state": {
              "$ref": "#/$defs/KernelSettledToolState"
            },
            "storage": {
              "description": "Storage facts supplied by the Host, never by the executed tool."
            },
            "tool": {
              "type": "string"
            },
            "toolCallId": {
              "type": "string"
            }
          },
          "required": [
            "toolCallId",
            "tool",
            "canonicalInput",
            "sourceOrder",
            "state",
            "result"
          ],
          "type": "object"
        },
        "KernelSettledToolState": {
          "enum": [
            "completed",
            "failed"
          ],
          "type": "string"
        },
        "KernelSteeringNotice": {
          "additionalProperties": false,
          "description": "One additional user request attached to a round directive.",
          "properties": {
            "content": {
              "type": "string"
            },
            "messageId": {
              "description": "Stable idempotency id of the durable steering row.",
              "type": "string"
            }
          },
          "required": [
            "messageId",
            "content"
          ],
          "type": "object"
        }
      },
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "description": "Host decision after committing one engine round output. `Batch` hands the\nsettled durable results of the proposed batch back to the live session;\n`Continuation` injects a bounded Host-authored review prompt; `Final` ends\nthe loop. The engine never derives any of these itself.",
      "properties": {
        "batchId": {
          "type": [
            "string",
            "null"
          ]
        },
        "checkpointSeq": {
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "kind": {
          "$ref": "#/$defs/KernelRoundDirectiveKind"
        },
        "previewSeq": {
          "description": "Preview-attribution cursor for a continuation round: the message id the\nengine must tag streamed previews with while answering the review.",
          "format": "uint64",
          "minimum": 0,
          "type": [
            "integer",
            "null"
          ]
        },
        "prompt": {
          "type": [
            "string",
            "null"
          ]
        },
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "steering": {
          "description": "Mid-run user requests durably received before this round boundary.\nThe engine appends them as ordinary user messages after the settled\ntool results (batch) or review prompt (continuation); they never carry\nauthority, tools, or grants. Final directives never carry steering.",
          "items": {
            "$ref": "#/$defs/KernelSteeringNotice"
          },
          "type": "array"
        },
        "tools": {
          "items": {
            "$ref": "#/$defs/KernelSettledToolResult"
          },
          "type": "array"
        }
      },
      "required": [
        "schemaVersion",
        "kind"
      ],
      "title": "KernelRoundDirective",
      "type": "object"
    },
    "KernelRoundOutputFrame": {
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "description": "One engine round output from a live loop session: either a tool proposal or\na completed answer. History, batch identity and the checkpoint cursor remain\nHost-owned and are never supplied or echoed by Node.",
      "properties": {
        "assistantMessage": true,
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "turnId": {
          "type": "string"
        }
      },
      "required": [
        "schemaVersion",
        "turnId",
        "assistantMessage"
      ],
      "title": "KernelRoundOutputFrame",
      "type": "object"
    },
    "KernelRunSnapshot": {
      "$defs": {
        "KernelToolSnapshot": {
          "additionalProperties": false,
          "properties": {
            "approvalState": {
              "type": [
                "string",
                "null"
              ]
            },
            "batchId": {
              "type": "string"
            },
            "sourceOrder": {
              "format": "uint",
              "minimum": 0,
              "type": "integer"
            },
            "state": {
              "type": "string"
            },
            "tool": {
              "type": "string"
            },
            "toolCallId": {
              "type": "string"
            }
          },
          "required": [
            "toolCallId",
            "batchId",
            "tool",
            "sourceOrder",
            "state"
          ],
          "type": "object"
        }
      },
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "properties": {
        "approvalDeadlineWallMs": {
          "format": "int64",
          "type": [
            "integer",
            "null"
          ]
        },
        "compactions": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "engineId": {
          "type": "string"
        },
        "lastEventSeq": {
          "description": "Decimal string: SQLite sequences must not lose precision in JavaScript.",
          "type": "string"
        },
        "providerAttempts": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "retryDueWallMs": {
          "format": "int64",
          "type": [
            "integer",
            "null"
          ]
        },
        "runId": {
          "type": "string"
        },
        "runningElapsedMs": {
          "format": "int64",
          "type": "integer"
        },
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "state": {
          "type": "string"
        },
        "terminalWritten": {
          "type": "boolean"
        },
        "tools": {
          "items": {
            "$ref": "#/$defs/KernelToolSnapshot"
          },
          "type": "array"
        },
        "turnAttempts": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "turnId": {
          "type": "string"
        }
      },
      "required": [
        "schemaVersion",
        "runId",
        "turnId",
        "engineId",
        "state",
        "lastEventSeq",
        "runningElapsedMs",
        "terminalWritten",
        "tools",
        "providerAttempts",
        "turnAttempts",
        "compactions"
      ],
      "title": "KernelRunSnapshot",
      "type": "object"
    },
    "KernelStateInvalidation": {
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "description": "A coalescible global invalidation, NOT a state transition or authority claim.",
      "properties": {
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "schemaVersion"
      ],
      "title": "KernelStateInvalidation",
      "type": "object"
    },
    "RunControlBinding": {
      "$defs": {
        "ExecutionAuthority": {
          "enum": [
            "legacy",
            "authoritative"
          ],
          "type": "string"
        },
        "FrozenPermission": {
          "additionalProperties": false,
          "properties": {
            "approvalEpoch": {
              "default": null,
              "description": "The approval generation this Run was frozen at.\n\nA reusable approval is bound to the generation that issued it: any\nrevocation advances the generation, so a Run frozen before it can never\nkeep using that approval — not even if the same (tool, scope) pair is\ngranted again afterwards. `None` means the binding predates the field\nand falls back to a per-grant liveness check only.",
              "format": "uint64",
              "minimum": 0,
              "type": [
                "integer",
                "null"
              ]
            },
            "grants": {
              "items": {
                "$ref": "#/$defs/PermissionGrant"
              },
              "type": "array"
            },
            "mode": {
              "$ref": "#/$defs/PermissionMode"
            },
            "projectRoot": {
              "type": [
                "string",
                "null"
              ]
            }
          },
          "required": [
            "mode",
            "grants"
          ],
          "type": "object"
        },
        "GrantKind": {
          "description": "What kind of authorization a frozen grant carries. A frozen Run must never\nbecome a way to keep using an approval the user has since withdrawn, so an\napproval-reuse grant is re-checked live at every use (REV-04).",
          "oneOf": [
            {
              "const": "resource",
              "description": "A resource grant (project root, read scope) frozen with the Run.",
              "type": "string"
            },
            {
              "const": "approval_reuse",
              "description": "A reusable approval (`allow_conversation`). Subject to live revocation.",
              "type": "string"
            }
          ]
        },
        "PermissionGrant": {
          "additionalProperties": false,
          "properties": {
            "kind": {
              "$ref": "#/$defs/GrantKind",
              "default": "approval_reuse",
              "description": "Absent in bindings frozen before revocation tracking existed; see\n[`GrantKind::default`]."
            },
            "scope": {
              "type": "string"
            },
            "tool": {
              "type": "string"
            }
          },
          "required": [
            "tool",
            "scope"
          ],
          "type": "object"
        },
        "PermissionMode": {
          "enum": [
            "ask",
            "read_only",
            "allow"
          ],
          "type": "string"
        },
        "ResourceExecutor": {
          "enum": [
            "runtime",
            "rust"
          ],
          "type": "string"
        },
        "TimeBudgets": {
          "additionalProperties": false,
          "properties": {
            "approvalWaitMs": {
              "format": "int64",
              "type": "integer"
            },
            "modelFirstResponseMs": {
              "default": 60000,
              "description": "No output at all within this long after dispatch fails the round.\nOld bindings predate this field and deserialize to the default.",
              "format": "int64",
              "type": "integer"
            },
            "modelIdleMs": {
              "default": 120000,
              "description": "No text/thinking/tool-parameter progress within this long fails the\nround, even if the whole-round bound has not been reached. Old bindings\npredate this field and deserialize to the default.",
              "format": "int64",
              "type": "integer"
            },
            "modelRequestMs": {
              "description": "Whole-round backstop for one model request, including slow-but-progressing\ngeneration. Stall detection is owned by the first-response/idle bounds\nbelow; this bound only caps total spend per round.",
              "format": "int64",
              "type": "integer"
            },
            "runExecutionLimited": {
              "description": "Old frozen runs retain their explicit limit. Ordinary new conversations\nopt out of a whole-run deadline; model/tool stall limits still apply.",
              "type": "boolean"
            },
            "runExecutionMs": {
              "format": "int64",
              "type": "integer"
            },
            "toolExecutionMs": {
              "format": "int64",
              "type": "integer"
            }
          },
          "required": [
            "modelRequestMs",
            "toolExecutionMs",
            "runExecutionMs",
            "approvalWaitMs"
          ],
          "type": "object"
        }
      },
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "additionalProperties": false,
      "properties": {
        "authority": {
          "$ref": "#/$defs/ExecutionAuthority"
        },
        "budgets": {
          "$ref": "#/$defs/TimeBudgets"
        },
        "conversationId": {
          "type": "string"
        },
        "engineId": {
          "type": "string"
        },
        "executionProfileId": {
          "type": "string"
        },
        "permission": {
          "$ref": "#/$defs/FrozenPermission"
        },
        "permissionSnapshotId": {
          "type": "string"
        },
        "readOnlyExecutor": {
          "$ref": "#/$defs/ResourceExecutor"
        },
        "runId": {
          "type": "string"
        },
        "schemaVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "schemaVersion",
        "runId",
        "conversationId",
        "engineId",
        "executionProfileId",
        "authority",
        "readOnlyExecutor",
        "permissionSnapshotId",
        "permission",
        "budgets"
      ],
      "title": "RunControlBinding",
      "type": "object"
    },
    "RuntimeCapabilityManifest": {
      "$defs": {
        "RuntimeToolCapability": {
          "properties": {
            "approval": {
              "type": "string"
            },
            "category": {
              "type": "string"
            },
            "execution": {
              "type": "string"
            },
            "name": {
              "type": "string"
            }
          },
          "required": [
            "name",
            "category",
            "execution",
            "approval"
          ],
          "type": "object"
        }
      },
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "properties": {
        "cancellation": {
          "type": "boolean"
        },
        "contextCompaction": {
          "type": "boolean"
        },
        "dynamicModelSwitch": {
          "type": "boolean"
        },
        "imageInput": {
          "type": "boolean"
        },
        "manifestVersion": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        },
        "reasoning": {
          "type": "boolean"
        },
        "sessionResume": {
          "type": "boolean"
        },
        "steering": {
          "type": "boolean"
        },
        "streamingText": {
          "type": "boolean"
        },
        "toolApproval": {
          "type": "boolean"
        },
        "tools": {
          "items": {
            "$ref": "#/$defs/RuntimeToolCapability"
          },
          "type": "array"
        },
        "workLoop": {
          "default": false,
          "type": "boolean"
        }
      },
      "required": [
        "manifestVersion",
        "streamingText",
        "cancellation",
        "reasoning",
        "sessionResume",
        "toolApproval",
        "imageInput",
        "steering",
        "contextCompaction",
        "dynamicModelSwitch",
        "tools"
      ],
      "title": "RuntimeCapabilityManifest",
      "type": "object"
    },
    "RuntimeEnvelope": {
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "properties": {
        "conversationId": {
          "type": [
            "string",
            "null"
          ]
        },
        "id": {
          "type": "string"
        },
        "kind": {
          "type": "string"
        },
        "payload": true,
        "protocol": {
          "type": "string"
        },
        "requestId": {
          "type": [
            "string",
            "null"
          ]
        },
        "runId": {
          "type": [
            "string",
            "null"
          ]
        },
        "runtimeSessionId": {
          "type": [
            "string",
            "null"
          ]
        },
        "seq": {
          "format": "int64",
          "type": [
            "integer",
            "null"
          ]
        },
        "timestamp": {
          "type": "string"
        },
        "type": {
          "type": "string"
        },
        "version": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "protocol",
        "version",
        "kind",
        "id",
        "type",
        "timestamp"
      ],
      "title": "RuntimeEnvelope",
      "type": "object"
    },
    "RuntimeRequest": {
      "$schema": "https://json-schema.org/draft/2020-12/schema",
      "properties": {
        "conversationId": {
          "type": [
            "string",
            "null"
          ]
        },
        "id": {
          "type": "string"
        },
        "kind": {
          "type": "string"
        },
        "payload": true,
        "protocol": {
          "type": "string"
        },
        "runId": {
          "type": [
            "string",
            "null"
          ]
        },
        "runtimeSessionId": {
          "type": [
            "string",
            "null"
          ]
        },
        "timestamp": {
          "type": "string"
        },
        "type": {
          "type": "string"
        },
        "version": {
          "format": "uint32",
          "minimum": 0,
          "type": "integer"
        }
      },
      "required": [
        "protocol",
        "version",
        "kind",
        "id",
        "timestamp",
        "type"
      ],
      "title": "RuntimeRequest",
      "type": "object"
    }
  },
  "toolContracts": [
    {
      "approval": "preflight",
      "category": "project-read",
      "execution": "runtime",
      "name": "read"
    },
    {
      "approval": "preflight",
      "category": "project-read",
      "execution": "runtime",
      "name": "ls"
    },
    {
      "approval": "preflight",
      "category": "project-read",
      "execution": "runtime",
      "name": "find"
    },
    {
      "approval": "preflight",
      "category": "project-read",
      "execution": "runtime",
      "name": "grep"
    },
    {
      "approval": "none",
      "category": "project-read",
      "execution": "runtime",
      "name": "graph_readonly_run"
    },
    {
      "approval": "none",
      "category": "attachment",
      "execution": "host",
      "name": "read_attachment"
    },
    {
      "approval": "none",
      "category": "attachment",
      "execution": "host",
      "name": "attachment_compute"
    },
    {
      "approval": "none",
      "category": "attachment",
      "execution": "host",
      "name": "compute_job_start"
    },
    {
      "approval": "none",
      "category": "attachment",
      "execution": "host",
      "name": "compute_job_status"
    },
    {
      "approval": "none",
      "category": "attachment",
      "execution": "host",
      "name": "compute_job_cancel"
    },
    {
      "approval": "none",
      "category": "attachment",
      "execution": "host",
      "name": "compute_job_result"
    },
    {
      "approval": "none",
      "category": "attachment",
      "execution": "host",
      "name": "read_tool_result"
    },
    {
      "approval": "none",
      "category": "skill",
      "execution": "host",
      "name": "skill_load"
    },
    {
      "approval": "policy",
      "category": "project-write",
      "execution": "host",
      "name": "write_file"
    },
    {
      "approval": "policy",
      "category": "project-write",
      "execution": "host",
      "name": "edit_file"
    },
    {
      "approval": "always",
      "category": "process",
      "execution": "host",
      "name": "run_command"
    },
    {
      "approval": "always",
      "category": "skill",
      "execution": "host",
      "name": "web_search"
    },
    {
      "approval": "always",
      "category": "skill",
      "execution": "host",
      "name": "web_read"
    },
    {
      "approval": "always",
      "category": "skill",
      "execution": "host",
      "name": "http_request"
    },
    {
      "approval": "always",
      "category": "skill",
      "execution": "host",
      "name": "system_info"
    },
    {
      "approval": "always",
      "category": "project-read",
      "execution": "host",
      "name": "sqlite_read"
    },
    {
      "approval": "none",
      "category": "skill",
      "execution": "host",
      "name": "structured_data"
    },
    {
      "approval": "none",
      "category": "project-read",
      "execution": "host",
      "name": "git_read"
    },
    {
      "approval": "always",
      "category": "process",
      "execution": "host",
      "name": "test_run"
    },
    {
      "approval": "always",
      "category": "process",
      "execution": "host",
      "name": "code_check"
    },
    {
      "approval": "always",
      "category": "project-write",
      "execution": "host",
      "name": "format_code"
    },
    {
      "approval": "none",
      "category": "project-read",
      "execution": "host",
      "name": "tabular_data"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "work_snapshot_get"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "graph_readonly_activate"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "graph_readonly_snapshot_get"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "graph_readonly_node_start"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "graph_readonly_node_review"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "graph_readonly_node_finish"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "graph_readonly_node_cancel"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "graph_readonly_accept"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "workflow_snapshot_get"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "workflow_start"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "workflow_stage_start"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "workflow_stage_complete"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "workflow_stage_fail"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "workflow_cancel"
    },
    {
      "approval": "none",
      "category": "delegation",
      "execution": "host",
      "name": "team_snapshot_get"
    },
    {
      "approval": "none",
      "category": "delegation",
      "execution": "host",
      "name": "team_start"
    },
    {
      "approval": "none",
      "category": "delegation",
      "execution": "host",
      "name": "team_member_start"
    },
    {
      "approval": "none",
      "category": "delegation",
      "execution": "host",
      "name": "team_collect"
    },
    {
      "approval": "none",
      "category": "delegation",
      "execution": "host",
      "name": "team_cancel"
    },
    {
      "approval": "none",
      "category": "delegation",
      "execution": "host",
      "name": "child_agent_list"
    },
    {
      "approval": "none",
      "category": "delegation",
      "execution": "host",
      "name": "child_run_start"
    },
    {
      "approval": "none",
      "category": "delegation",
      "execution": "host",
      "name": "child_run_collect"
    },
    {
      "approval": "none",
      "category": "delegation",
      "execution": "host",
      "name": "child_run_cancel"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "goal_propose"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "goal_complete"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "task_create_many"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "task_update"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "task_attempt_start"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "task_repair_start"
    },
    {
      "approval": "always",
      "category": "work",
      "execution": "host",
      "name": "task_repair_escalate_start"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "task_attempt_finish"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "task_evidence_add"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "task_evidence_validate"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "plan_revision_create"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "review_finding_add"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "review_finding_resolve"
    },
    {
      "approval": "none",
      "category": "work",
      "execution": "host",
      "name": "acceptance_submit"
    },
    {
      "approval": "none",
      "category": "memory",
      "execution": "host",
      "name": "memory_search"
    },
    {
      "approval": "none",
      "category": "memory",
      "execution": "host",
      "name": "memory_propose"
    },
    {
      "approval": "none",
      "category": "knowledge",
      "execution": "host",
      "name": "list_knowledge_bases"
    },
    {
      "approval": "none",
      "category": "knowledge",
      "execution": "host",
      "name": "search_knowledge"
    },
    {
      "approval": "none",
      "category": "knowledge",
      "execution": "host",
      "name": "read_knowledge_document"
    },
    {
      "approval": "none",
      "category": "knowledge",
      "execution": "host",
      "name": "query_knowledge_graph"
    },
    {
      "approval": "none",
      "category": "mcp",
      "execution": "host",
      "name": "list_mcp_tools"
    },
    {
      "approval": "always",
      "category": "mcp",
      "execution": "host",
      "name": "call_mcp_tool"
    }
  ]
}
export const PROTOCOL_NAME = SCHEMA_BUNDLE.protocolName
export const PROTOCOL_VERSION = SCHEMA_BUNDLE.protocolVersion
export const CAPABILITY_MANIFEST_VERSION = SCHEMA_BUNDLE.capabilityManifestVersion
export const RUNTIME_TOOL_CATALOG = Object.freeze(SCHEMA_BUNDLE.toolContracts.map(Object.freeze))
export function validateWireValue(schemaName, value) {
  const schema = SCHEMA_BUNDLE.schemas[schemaName]
  return schema ? validateSchema(schema, value) : ['unknown wire schema: ' + schemaName]
}
