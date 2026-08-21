from __future__ import annotations

import hashlib

_CHILD_THREAD_ID_PREFIX = "subagent_"
_CHILD_THREAD_ID_DIGEST_LENGTH = 64 - len(_CHILD_THREAD_ID_PREFIX)


def make_child_thread_id(parent_thread_id: str, agent_slug: str, tool_call_id: str) -> str:
    digest = hashlib.sha256(f"{parent_thread_id}:{agent_slug}:{tool_call_id}".encode()).hexdigest()
    return f"{_CHILD_THREAD_ID_PREFIX}{digest[:_CHILD_THREAD_ID_DIGEST_LENGTH]}"
