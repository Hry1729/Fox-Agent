use super::*;
use crate::kernel_compaction::{self as context, CompactionPlan, CompactionResult};

impl KernelCoordinator<'_> {
    pub(super) fn model_retry_context(&self, target: &str, mut view: Vec<Value>) -> Result<Vec<Value>, String> {
        let effect_key = if target == "initial" { "initial-model".to_string() } else { format!("deliver-batch:{target}") };
        if self.database.kernel_model_retry_needs_completion(&self.binding.run_id, &effect_key)? {
            // This fixed recovery instruction fits within the dispatch's 4096
            // reserved bytes. The durable source/compaction history is unchanged.
            view.push(serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": "Fox 续答提示：上一条模型回复已经停止，但没有给出有效的最终答复。请根据原始用户请求、已有对话以及本轮已完成的工具结果，继续执行尚未完成的工作。不要重复已经完成的操作，不要只说明你将开始分析。需要更多操作时请直接调用可用工具；确实完成、遇到具体阻碍或需要补充信息时，给出有用的答复，并遵守系统提示中的最终答复格式。"}],
                "timestamp": 0,
            }));
        }
        Ok(view)
    }

    fn source_context(&self, target: &str) -> Result<Vec<Value>, String> {
        if target == "initial" {
            return Ok(self
                .database
                .kernel_initial_input(&self.binding.run_id)?
                .messages);
        }
        let event = self
            .database
            .kernel_engine_batch_checkpoint(&self.binding.run_id, target)?
            .ok_or("missing compaction source checkpoint")?;
        let value = &event["checkpoint"]["value"];
        if event["engineId"] != self.binding.engine_id
            || event["checkpoint"]["hash"] != checkpoint_hash(value)
        {
            return Err("compaction source checkpoint identity/hash mismatch".into());
        }
        let checkpoint: fox_engine_protocol::KernelEngineBatchCheckpoint =
            serde_json::from_value(value.clone())
                .map_err(|_| "invalid compaction source checkpoint")?;
        checkpoint.validate()?;
        if checkpoint.batch_id != target {
            return Err("compaction source target mismatch".into());
        }
        Ok(checkpoint.history)
    }

    pub(super) fn context_view(
        &self,
        target: &str,
        original: &[Value],
    ) -> Result<Vec<Value>, String> {
        let mut view = original.to_vec();
        for (plan, result) in self
            .database
            .kernel_compaction_results(&self.binding.run_id, target)?
        {
            if plan.target != target
                || plan.source != view
                || plan.config_hash
                    != self
                        .database
                        .kernel_model_config(&self.binding.run_id)?
                        .hash()?
            {
                return Err("compaction view no longer matches its immutable source".into());
            }
            view = result.view(&plan)?;
        }
        Ok(view)
    }

    pub(super) fn prepare_context_if_needed(
        &self,
        target: &str,
        extra_bytes: usize,
    ) -> Result<bool, String> {
        self.tick()?;
        let config = self.database.kernel_model_config(&self.binding.run_id)?;
        let original = self.source_context(target)?;
        let view = self.context_view(target, &original)?;
        let extra = extra_bytes
            .saturating_add(config.system_prompt.len())
            .saturating_add(context::bytes(&config.proposal_tools)?);
        if context::bytes(&view)? <= context::history_limit(&config, extra) {
            return Ok(false);
        }
        let previous = self
            .database
            .kernel_compaction_results(&self.binding.run_id, target)?;
        if previous.len() >= context::MAX_PASSES
            || previous.last().is_some_and(|(_, result)| !result.applied)
        {
            return Err(context::INSUFFICIENT.into());
        }
        let turn = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?
            .shadow_checkpoint(self.clock.now_monotonic_ms())
            .turn_id;
        let plan = CompactionPlan::prepare(&self.binding.run_id, &turn, target, &view, &config)?
            .ok_or(context::INSUFFICIENT)?;
        self.database
            .kernel_validate_resource_acquisition(&self.binding.run_id)?;
        self.cancellation.run_token(&self.binding.run_id)?.check()?;
        let encoded = serde_json::to_string(&plan).map_err(|_| "invalid compaction plan")?;
        self.apply(None, |controller, now| {
            controller.prepare_context_compaction(
                &plan.request.compaction_id,
                &encoded,
                now.monotonic_ms,
            )
        })?;
        Ok(true)
    }

    pub(super) fn dispatch_pending_compaction(
        &self,
        owner: &str,
        deliver: impl FnOnce(
            &RunControlBinding,
            &fox_engine_protocol::KernelCompactionRequest,
            &kernel::CancellationToken,
            i64,
        ) -> Result<fox_engine_protocol::KernelCompactionResponse, String>,
    ) -> Result<(), String> {
        self.tick()?;
        let pending = self.snapshot()?.compaction.pending.ok_or(context::FAILED)?;
        if pending.owner.is_some() {
            return Err(context::UNCERTAIN.into());
        }
        let plan = self
            .database
            .kernel_compaction_plan(&self.binding.run_id, &pending.id)?;
        let current = self.context_view(&plan.target, &self.source_context(&plan.target)?)?;
        if plan.source != current {
            return Err(context::FAILED.into());
        }
        let token = self.cancellation.run_token(&self.binding.run_id)?;
        token.check()?;
        self.apply(None, |controller, now| {
            controller.dispatch_context_compaction(
                &pending.id,
                owner,
                now.monotonic_ms,
                now.wall_ms,
            )
        })?;
        token.check()?;
        let now = self.clock.read();
        let facts = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?
            .shadow_checkpoint(now.monotonic_ms);
        let since = facts.model_request_since_wall_ms.ok_or(context::FAILED)?;
        if now.wall_ms < since {
            return Err(context::FAILED.into());
        }
        let remaining = self
            .binding
            .budgets
            .run_execution_ms
            .saturating_sub(facts.running_elapsed_ms)
            .min(
                self.binding
                    .budgets
                    .model_request_ms
                    .saturating_sub(now.wall_ms.saturating_sub(since)),
            );
        // Owner and input are durable before the first external request. On an
        // unknown result, recovery never renews this request or its deadline.
        let response = deliver(&self.binding, &plan.request, &token, remaining)
            .map_err(|_| context::FAILED)?;
        self.tick()?;
        token.check()?;
        let result = CompactionResult::new(&plan, response).map_err(|_| context::FAILED)?;
        let encoded = serde_json::to_string(&result).map_err(|_| context::FAILED)?;
        self.apply(None, |controller, now| {
            controller.complete_context_compaction(
                &pending.id,
                owner,
                &encoded,
                result.applied,
                now.monotonic_ms,
            )
        })?;
        Ok(())
    }

    pub(crate) fn resume_context_compaction(
        &self,
        owner: &str,
        runtime: &super::super::RuntimeCommand,
        api_key: &str,
    ) -> Result<(), String> {
        let config = self.database.kernel_model_config(&self.binding.run_id)?;
        self.dispatch_pending_compaction(owner, |binding, request, token, remaining| {
            super::super::kernel_model_worker::compact_context(
                runtime, &config, api_key, binding, request, token, remaining,
            )
        })
    }

    pub(super) fn ensure_context_with_worker(
        &self,
        target: &str,
        extra_bytes: usize,
        owner: &str,
        runtime: &super::super::RuntimeCommand,
        api_key: &str,
    ) -> Result<(), String> {
        while self.prepare_context_if_needed(target, extra_bytes)? {
            self.resume_context_compaction(owner, runtime, api_key)?;
        }
        Ok(())
    }
}
