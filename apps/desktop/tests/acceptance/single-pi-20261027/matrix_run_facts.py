"""Capture one run's database linkage and bounded native-session facts."""
import argparse
import hashlib
import json
import sqlite3
from pathlib import Path


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--run', required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--needle', action='append', default=[])
    args = parser.parse_args()
    root = args.data.resolve()
    db = sqlite3.connect((root / 'fox.db').as_uri() + '?mode=ro', uri=True)
    db.row_factory = sqlite3.Row

    def rows(query, params):
        return [dict(item) for item in db.execute(query, params)]

    runs = rows('''SELECT r.id, r.conversation_id, r.runtime_session_id, r.status, r.model,
        r.error_code, r.error_message, r.started_at, r.finished_at, r.created_at,
        b.authority, b.engine_id FROM runs r LEFT JOIN run_control_bindings b ON b.run_id=r.id
        WHERE r.id=?''', (args.run,))
    if len(runs) != 1:
        raise SystemExit(f'expected one run, found {len(runs)}')
    run = runs[0]
    sessions = rows('SELECT * FROM runtime_sessions WHERE conversation_id=?', (run['conversation_id'],))
    result = {'run': run, 'current_sessions': sessions,
        'messages': rows('SELECT id, role, kind, content, status, ordinal FROM messages WHERE run_id=? ORDER BY ordinal', (args.run,)),
        'tools': rows('SELECT id, tool_name, status, input_json, result_json, error_message FROM tool_calls WHERE run_id=? ORDER BY started_at, id', (args.run,)),
        'event_counts': rows('SELECT event_type, COUNT(*) AS count FROM run_events WHERE run_id=? GROUP BY event_type', (args.run,))}
    native = []
    for session in sessions:
        text = session.get('native_session_path')
        if not text:
            continue
        if text.startswith('\\\\?\\'):
            text = text[4:]
        file = Path(text).resolve()
        managed = (root / 'runtime-sessions').resolve()
        if managed not in file.parents:
            raise SystemExit(f'native path outside managed directory: {file}')
        entry = {'path': str(file), 'exists': file.is_file()}
        if file.is_file():
            raw = file.read_bytes()
            entry['sha256'] = hashlib.sha256(raw).hexdigest()
            entry['size'] = len(raw)
            records = []
            for line in raw.splitlines():
                try:
                    item = json.loads(line)
                except (UnicodeDecodeError, json.JSONDecodeError):
                    records.append({'invalid': True})
                    continue
                message = item.get('message') or {}
                body = json.dumps(message.get('content', ''), ensure_ascii=False, sort_keys=True)
                records.append({'type': item.get('type'), 'id': item.get('id'), 'parentId': item.get('parentId'),
                    'role': message.get('role'), 'content_sha256': hashlib.sha256(body.encode('utf-8')).hexdigest(),
                    'needles': [needle for needle in args.needle if needle in body]})
            entry['records'] = records
        native.append(entry)
    result['native_files'] = native
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
    print(json.dumps({'out': str(args.out), 'run': run['id'], 'status': run['status'],
        'runtime_session_id': run['runtime_session_id'], 'authority': run['authority'], 'engine_id': run['engine_id'],
        'tools': [item['tool_name'] for item in result['tools']]}, ensure_ascii=False))
    db.close()


if __name__ == '__main__':
    main()
