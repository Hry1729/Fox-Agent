#!/usr/bin/env python3
"""Read-only projection of one or more Fox runs from a database copy.

Prints exactly the facts the diagnostics compare: which authority ran the run,
which model service the Host froze/handed over, what the provider request body
carried (usage.request), and which credential *name* each path resolves.

The database is opened read-only (`mode=ro`) and never written. No credential
value is read: only column names and the base URL that names the keyring entry.

Usage:
    python inspect-run-facts.py <path-to-fox.db> <runId> [<runId> ...]

Notes:
    * Copy `fox.db` *with* `fox.db-wal` next to it if the app is (or was) running;
      `-wal` holds recent rows.
    * sqlite3 is a Python standard-library module: nothing to install.
"""
import json
import sqlite3
import sys


def connect(path):
    uri = "file:" + path.replace("\\", "/").replace(" ", "%20") + "?mode=ro"
    connection = sqlite3.connect(uri, uri=True)
    connection.row_factory = sqlite3.Row
    return connection


def rows(connection, sql, params=()):
    return [dict(row) for row in connection.execute(sql, params)]


def print_json(label, value):
    print(f"{label}: {json.dumps(value, ensure_ascii=False, indent=1)}")


def describe(connection, run_id):
    print(f"\n{'=' * 78}\nrun {run_id}\n{'=' * 78}")
    run = rows(connection, "select id, status, model, error_code, error_message, runtime_session_id, run_kind from runs where id = ?", (run_id,))
    print_json("runs row", run[0] if run else None)

    binding = rows(connection, "select authority, engine_id, binding_json from run_control_bindings where run_id = ?", (run_id,))
    if binding:
        parsed = binding[0]
        try:
            parsed = json.loads(binding[0]["binding_json"])
            parsed = {k: parsed.get(k) for k in ("authority", "engineId", "executionProfileId", "readOnlyExecutor")}
        except Exception:
            pass
        print_json("frozen control binding", parsed)

    frozen = rows(connection, "select config_json from kernel_model_configs where run_id = ?", (run_id,))
    if frozen:
        config = json.loads(frozen[0]["config_json"])
        print_json("frozen Kernel modelService (authoritative path only)", config.get("modelService"))

    snapshot = rows(connection, "select event_json from run_events where run_id = ? and event_type = 'run.request_snapshot'", (run_id,))
    if snapshot:
        event = json.loads(snapshot[0]["event_json"])
        print_json("sidecar run.request_snapshot", {
            key: event.get(key) for key in ("model", "apiType", "provider", "baseUrl", "maxOutputTokens", "modelProfile")
        })

    usage = rows(connection, "select event_json from run_events where run_id = ? and event_type = 'usage.request'", (run_id,))
    for index, event in enumerate(usage):
        record = json.loads(event["event_json"]).get("record", {})
        print_json(f"usage.request[{index}] (the body actually serialized)", {
            "provider": record.get("provider"),
            "modelId": record.get("modelId"),
            "api": record.get("api"),
            "outcome": record.get("outcome"),
            "transportAttempts": record.get("transportAttempts"),
            "config": record.get("config"),
            "usage": record.get("usage"),
        })

    phases = rows(connection, "select seq, event_type, event_json from run_events where run_id = ? and event_type in ('run.phase', 'run.failed', 'run.completed') order by seq", (run_id,))
    for phase in phases:
        print_json(f"event {phase['seq']} {phase['event_type']}", json.loads(phase["event_json"]))


def main(argv):
    if len(argv) < 3:
        print(__doc__)
        return 2
    connection = connect(argv[1])
    try:
        for run_id in argv[2:]:
            describe(connection, run_id)
        print("\ncredential *name* resolved by each path = the model base URL above;"
              "\nthe keyring service is `com.fox.agent.model` (apps/desktop/src-tauri/src/model_service.rs:9,335)."
              "\nNo credential value is read by this script.")
        return 0
    finally:
        connection.close()


if __name__ == "__main__":
    sys.exit(main(sys.argv))
