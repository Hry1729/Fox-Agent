//! Durable authoritative coordinator. Not attached to default RuntimeHost startup:
//! Pi protocol, capability and desktop rollout gates remain separate prerequisites.
//! The executor receives a frozen binding and must revalidate resources itself.
use crate::{
    database::Database,
    kernel::{
        self, CancellationPort, CancellationRegistry, Clock, ClockReading, Effect, KernelError,
        PolicyDecisionPort, RunController, ToolCallRequest,
    },
};
use fox_engine_protocol::{ExecutionAuthority, RunControlBinding};
use serde_json::Value;
use std::sync::Mutex;

pub(crate) struct KernelCoordinator<'a> {
    database: &'a Database,
    clock: &'a dyn Clock,
    cancellation: &'a CancellationRegistry,
    binding: RunControlBinding,
    controller: Mutex<RunController>,
}

impl<'a> KernelCoordinator<'a> {
    pub(crate) fn reopen(
        database: &'a Database,
        clock: &'a dyn Clock,
        run_id: &str,
        cancellation: &'a CancellationRegistry,
    ) -> Result<Self, String> {
        let binding = database
            .run_control_binding(run_id)?
            .ok_or("authoritative Run has no frozen control binding")?;
        let data = database
            .kernel_rehydrate(run_id)?
            .ok_or("authoritative Run has no durable Kernel state")?;
        let config = &data.config;
        if binding.authority != ExecutionAuthority::Authoritative
            || config.kernel_mode != "authoritative"
            || binding.engine_id != config.engine_id
            || binding.permission_snapshot_id != config.permission_snapshot_id
            || binding.execution_profile_id != config.execution_profile_id
            || binding.budgets.approval_wait_ms != config.approval_wait_timeout_ms
            || binding.budgets.model_request_ms != config.model_request_timeout_ms
            || binding.budgets.tool_execution_ms != config.tool_execution_timeout_ms
            || binding.budgets.run_execution_ms != config.run_execution_budget_ms
        {
            return Err(
                "Kernel and resource control bindings disagree; no authority fallback is allowed"
                    .into(),
            );
        }
        let controller = RunController::rehydrate(data).map_err(|error| error.to_string())?;
        cancellation.register_run(run_id)?;
        if controller.is_terminal() || controller.state() == kernel::RunState::Cancelling {
            cancellation.request_run_cancel(run_id);
        }
        Ok(Self {
            database,
            clock,
            cancellation,
            binding,
            controller: Mutex::new(controller),
        })
    }

    /// A failed decision transaction never leaves speculative state in memory.
    fn apply(
        &self,
        lease: Option<(&str, &str)>,
        action: impl FnOnce(&mut RunController, ClockReading) -> Result<Vec<Effect>, KernelError>,
    ) -> Result<(), String> {
        // Re-read the immutable, hash-verified resource binding on each entry.
        if self
            .database
            .run_control_binding(&self.binding.run_id)?
            .as_ref()
            != Some(&self.binding)
        {
            return Err("authoritative Run control binding changed".into());
        }
        let mut guard = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?;
        let mut candidate = guard.clone();
        let now = self.clock.read();
        let effects = action(&mut candidate, now).map_err(|error| error.to_string())?;
        let command = candidate.persist_command(&effects);
        if let Some((tool_call_id, owner)) = lease {
            self.database.kernel_commit_tool_result(
                &self.binding.run_id,
                now.wall_ms,
                &command,
                tool_call_id,
                owner,
            )?;
        } else {
            self.database
                .kernel_commit_decision(&self.binding.run_id, now.wall_ms, &command)?;
        }
        // Persist cancellation first, then signal every issued execution token.
        for effect in &effects {
            if let Effect::CancelToolCall { tool_call_id } = effect {
                self.cancellation
                    .request_tool_cancel(&self.binding.run_id, tool_call_id);
            }
        }
        if candidate.is_terminal() || candidate.state() == kernel::RunState::Cancelling {
            self.cancellation.request_run_cancel(&self.binding.run_id);
        }
        *guard = candidate;
        Ok(())
    }

    pub(crate) fn tick(&self) -> Result<(), String> {
        self.apply(None, |controller, now| {
            Ok(controller.tick(now.monotonic_ms, now.wall_ms))
        })
    }

    pub(crate) fn propose_tools(
        &self,
        batch_id: &str,
        calls: Vec<ToolCallRequest>,
        policy: &dyn PolicyDecisionPort,
    ) -> Result<(), String> {
        self.tick()?;
        self.apply(None, |controller, now| {
            controller.propose_tool_batch(batch_id, calls, policy, now.monotonic_ms, now.wall_ms)
        })
    }

    pub(crate) fn resolve_approval(
        &self,
        tool_call_id: &str,
        decision: kernel::ApprovalDecision,
    ) -> Result<(), String> {
        self.tick()?;
        self.apply(None, |controller, now| {
            controller.resolve_approval(tool_call_id, decision, now.monotonic_ms)
        })
    }

    pub(crate) fn cancel(&self) -> Result<(), String> {
        self.apply(None, |controller, _| Ok(controller.request_cancel()))
    }

    pub(crate) fn snapshot(&self) -> Result<kernel::KernelSnapshot, String> {
        self.database
            .kernel_build_full_snapshot(&self.binding.run_id)
    }

    /// Dispatch exactly one durable intent. Err after lease is uncertain and is
    /// deliberately left leased; neither this method nor reopen retries it.
    /// The callback runs outside the controller lock, permitting cancellation and
    /// parallel independent calls. Results are committed in source-order batches.
    pub(crate) fn dispatch_tool(
        &self,
        tool_call_id: &str,
        owner: &str,
        execute: impl FnOnce(
            &RunControlBinding,
            &kernel::OutboxEffect,
            &kernel::CancellationToken,
        ) -> Result<(bool, Value), String>,
    ) -> Result<bool, String> {
        self.tick()?;
        let effect_key = kernel::dispatch_effect_key(tool_call_id);
        let leased = {
            let guard = self
                .controller
                .lock()
                .map_err(|_| "Kernel coordinator lock poisoned")?;
            let data = guard.shadow_checkpoint(self.clock.now_monotonic_ms());
            let tool = data
                .tools
                .iter()
                .find(|tool| tool.tool_call_id == tool_call_id)
                .ok_or("unknown Kernel tool")?;
            if data.state.is_terminal()
                || data.state == kernel::RunState::Cancelling
                || tool.state != kernel::ToolCallState::Running
            {
                return Ok(false);
            }
            let Some(effect) = self.database.kernel_outbox_lease_effect(
                &self.binding.run_id,
                &effect_key,
                owner,
            )?
            else {
                return Ok(false);
            };
            let payload: Value =
                serde_json::from_str(&effect.payload_json).map_err(|error| error.to_string())?;
            let expected_input: Value =
                serde_json::from_str(&tool.input_json).map_err(|error| error.to_string())?;
            if effect.kind != kernel::OutboxEffectKind::DispatchTool
                || effect.tool_call_id.as_deref() != Some(tool_call_id)
                || effect.idempotency_key != kernel::dispatch_idempotency_key(tool_call_id)
                || payload.get("tool").and_then(Value::as_str) != Some(tool.tool.as_str())
                || payload.get("input") != Some(&expected_input)
            {
                return Err("durable dispatch differs from the approved tool identity".into());
            }
            effect
        };
        let token = self
            .cancellation
            .tool_token(&self.binding.run_id, tool_call_id)?;
        token.check()?;
        let (succeeded, result) = execute(&self.binding, &leased, &token)?;
        self.tick()?;
        self.apply(Some((tool_call_id, owner)), |controller, _| {
            controller.tool_settled(tool_call_id, succeeded, &result.to_string())
        })?;
        Ok(true)
    }
}

#[cfg(test)]
mod tests;
