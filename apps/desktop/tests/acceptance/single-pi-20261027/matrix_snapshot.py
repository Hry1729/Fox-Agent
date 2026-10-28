"""Capture bounded SQLite and file facts for the single-Pi desktop matrix."""
import argparse
import hashlib
import json
import sqlite3
from pathlib import Path


def digest(value):
    return hashlib.sha256(value).hexdigest()


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--backup', type=Path)
    parser.add_argument('--conversation', action='append', required=True)
    args = parser.parse_args()
    source = (args.data / 'fox.db').resolve()
    db = sqlite3.connect(source.as_uri() + '?mode=ro', uri=True)
    db.row_factory = sqlite3.Row
    if args.backup:
        args.backup.parent.mkdir(parents=True, exist_ok=True)
        if args.backup.exists():
            raise SystemExit(f'refusing to overwrite backup: {args.backup}')
        with sqlite3.connect(args.backup) as copy:
            db.backup(copy)

    def rows(query, params=()):
        return [dict(row) for row in db.execute(query, params)]

    result = {'database': str(source), 'conversations': {}, 'session_files': []}
    for conversation_id in args.conversation:
        data = {}
        data['conversation'] = rows('SELECT * FROM conversations WHERE id=?', (conversation_id,))
        data['expert'] = rows('SELECT id, conversation_id, expert_id, state, activation_source, expert_version, package_hash FROM conversation_expert_bindings WHERE conversation_id=? ORDER BY id', (conversation_id,))
        data['knowledge'] = rows('SELECT * FROM knowledge_bindings_v2 WHERE conversation_id=? ORDER BY id', (conversation_id,))
        data['current'] = rows('SELECT * FROM runtime_sessions WHERE conversation_id=? ORDER BY runtime_type', (conversation_id,))
        data['retired'] = rows('SELECT * FROM runtime_session_generations WHERE conversation_id=? ORDER BY runtime_type, context_generation', (conversation_id,))
        data['runs'] = rows('''SELECT r.id, r.conversation_id, r.runtime_session_id, r.status, r.model,
            r.error_code, r.error_message, r.created_at, b.authority, b.engine_id
            FROM runs r LEFT JOIN run_control_bindings b ON b.run_id=r.id
            WHERE r.conversation_id=? ORDER BY r.created_at, r.id''', (conversation_id,))
        messages = rows('SELECT id, run_id, role, kind, content, status, ordinal, runtime_message_id, metadata_json FROM messages WHERE conversation_id=? ORDER BY ordinal, id', (conversation_id,))
        data['messages'] = {'count': len(messages), 'sha256': digest(json.dumps(messages, ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode('utf-8')), 'ids': [row['id'] for row in messages]}
        expert_ids = [r['expert_id'] for r in data['expert']]
        memories = rows('SELECT * FROM memory_entities WHERE source_conversation_id=? OR scope_key IN (%s) ORDER BY id' % (','.join('?' for _ in expert_ids) or "''"), [conversation_id, *expert_ids])
        data['memories'] = {'count': len(memories), 'sha256': digest(json.dumps(memories, ensure_ascii=False, sort_keys=True, separators=(',', ':')).encode('utf-8')), 'ids': [row['id'] for row in memories]}
        result['conversations'][conversation_id] = data
    for file in sorted((args.data / 'runtime-sessions').glob('*.jsonl')):
        result['session_files'].append({'name': file.name, 'size': file.stat().st_size, 'sha256': digest(file.read_bytes())})
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(json.dumps(result, ensure_ascii=False, indent=2), encoding='utf-8')
    db.close()
    print(json.dumps({'output': str(args.out), 'backup': str(args.backup) if args.backup else None, 'session_files': len(result['session_files']), 'conversations': list(result['conversations'])}, ensure_ascii=False))


if __name__ == '__main__':
    main()
