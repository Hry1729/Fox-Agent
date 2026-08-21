from __future__ import annotations

from typing import Annotated, Literal, TypedDict

from yuxi.agents.state import BaseState


class SubAgentRunState(TypedDict, total=False):
    id: str
    subagent_type: str
    subagent_name: str
    child_thread_id: str
    description: str
    status: Literal["running", "completed", "failed"]
    created_at: str
    completed_at: str
    result_preview: str
    error: str | None
    artifacts: list[str]


def merge_subagent_runs(
    existing: list[SubAgentRunState] | None,
    new: list[SubAgentRunState] | None,
) -> list[SubAgentRunState]:
    if existing is None:
        return list(new or [])
    if new is None:
        return existing

    merged = [dict(item) for item in existing]
    child_thread_index = {
        item.get("child_thread_id"): position for position, item in enumerate(merged) if item.get("child_thread_id")
    }
    id_index = {item.get("id"): position for position, item in enumerate(merged) if item.get("id")}
    for item in new:
        run = dict(item)
        child_thread_id = run.get("child_thread_id")
        run_id = run.get("id")
        position = None
        if child_thread_id and child_thread_id in child_thread_index:
            position = child_thread_index[child_thread_id]
        elif run_id and run_id in id_index:
            position = id_index[run_id]

        if position is None:
            position = len(merged)
            merged.append(run)
        else:
            merged[position] = {**merged[position], **run}

        if child_thread_id:
            child_thread_index[child_thread_id] = position
        if run_id:
            id_index[run_id] = position
    return merged


class ChatBotState(BaseState):
    subagent_runs: Annotated[list[SubAgentRunState], merge_subagent_runs]
