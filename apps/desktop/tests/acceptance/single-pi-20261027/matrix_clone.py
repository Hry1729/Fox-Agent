"""Make a disposable data copy with paths confined to the copy."""
import argparse
import hashlib
import json
import shutil
import sqlite3
from pathlib import Path


def backup(source, destination):
    destination.parent.mkdir(parents=True, exist_ok=True)
    with sqlite3.connect(source.resolve().as_uri() + '?mode=ro', uri=True) as origin:
        with sqlite3.connect(destination) as copy:
            origin.backup(copy)
            if copy.execute('PRAGMA quick_check').fetchone()[0] != 'ok':
                raise RuntimeError(f'bad SQLite backup: {destination}')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--snapshot', type=Path, required=True)
    parser.add_argument('--dest', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    args = parser.parse_args()
    source, dest = args.source.resolve(), args.dest.resolve()
    if dest.exists() or dest == source or source in dest.parents or dest in source.parents:
        raise SystemExit('destination must be a new, separate directory')
    if not (source / '.fox-single-pi-acceptance').is_file():
        raise SystemExit('source is missing the acceptance marker')
    dest.mkdir(parents=True)
    shutil.copy2(source / '.fox-single-pi-acceptance', dest)
    for directory in ('runtime-sessions', 'skills', 'attachments'):
        if (source / directory).exists():
            shutil.copytree(source / directory, dest / directory)
    source_knowledge = source / 'local-knowledge'
    shutil.copytree(source_knowledge, dest / 'local-knowledge', ignore=shutil.ignore_patterns('knowledge.db', 'knowledge.db-wal', 'knowledge.db-shm'))
    backup(args.snapshot, dest / 'fox.db')
    backup(source_knowledge / 'knowledge.db', dest / 'local-knowledge' / 'knowledge.db')
    old, new = str(source), str(dest)
    changed = {}
    with sqlite3.connect(dest / 'fox.db') as db:
        for table in ('runtime_sessions', 'runtime_session_generations'):
            for column in ('native_session_path', 'session_path'):
                rows = db.execute(f'SELECT rowid, "{column}" FROM "{table}" WHERE "{column}" IS NOT NULL').fetchall()
                count = 0
                for rowid, path in rows:
                    if old not in path:
                        raise RuntimeError(f'{table}.{column} is outside source data: {path}')
                    db.execute(f'UPDATE "{table}" SET "{column}"=? WHERE rowid=?', (path.replace(old, new), rowid))
                    count += 1
                changed[f'{table}.{column}'] = count
    with sqlite3.connect(dest / 'local-knowledge' / 'knowledge.db') as db:
        rows = db.execute('SELECT rowid, knowledge_base_id, relative_path, source_path, content_hash FROM local_kb_documents WHERE source_path IS NOT NULL').fetchall()
        count = 0
        for rowid, base_id, relative, path, expected_hash in rows:
            copied = dest / 'local-knowledge' / 'knowledge-bases' / base_id / 'documents' / relative
            if not copied.is_file() or hashlib.sha256(copied.read_bytes()).hexdigest() != expected_hash:
                raise RuntimeError(f'copied knowledge document missing or has changed: {copied}')
            db.execute('UPDATE local_kb_documents SET source_path=? WHERE rowid=?', (str(copied), rowid))
            count += 1
        changed['local_kb_documents.source_path'] = count
    headers = []
    for session in (dest / 'runtime-sessions').glob('*.jsonl'):
        raw = session.read_bytes()
        header, sep, rest = raw.partition(b'\n')
        record = json.loads(header)
        cwd = record.get('cwd')
        if cwd and old in cwd:
            record['cwd'] = cwd.replace(old, new)
            content = json.dumps(record, ensure_ascii=False, separators=(',', ':')).encode('utf-8') + sep + rest
            session.write_bytes(content)
            headers.append({'file': session.name, 'old_sha256': hashlib.sha256(raw).hexdigest(), 'new_sha256': hashlib.sha256(content).hexdigest()})
        elif cwd:
            raise RuntimeError(f'session cwd is outside source data: {cwd}')
    args.manifest.parent.mkdir(parents=True, exist_ok=True)
    args.manifest.write_text(json.dumps({'source': str(source), 'destination': str(dest), 'path_updates': changed, 'session_headers': headers}, ensure_ascii=False, indent=2), encoding='utf-8')
    print(json.dumps({'destination': str(dest), 'path_updates': changed, 'session_headers_rewritten': len(headers)}, ensure_ascii=False))


if __name__ == '__main__':
    main()
