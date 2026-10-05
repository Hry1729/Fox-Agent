use super::*;
use crate::kernel_compaction::{self as context, CompactionPlan, CompactionResult};

impl KernelCoordinator<'_> {
    pub(super) fn context_fits(&self, config: &crate::kernel_model_config::KernelModelConfig, view: &[Value], extra: usize) -> Result<bool,String> {
        let usage=self.database.kernel_usage_calibration(&self.binding.run_id)?;
        let mut report=context::context_budget_report(config,view,extra,usage.calibration,usage.latest_usage_input_tokens,"none",crate::database::now_ms())?;
        if !report.within_budget {report.trigger_reason="threshold".into();}
        // Telemetry cannot fail a correctly authorized model dispatch.
        let _=self.database.kernel_store_context_budget(&self.binding.run_id,&report);
        Ok(report.within_budget)
    }

    pub(super) fn model_retry_context(&self, target: &str, mut view: Vec<Value>) -> Result<Vec<Value>, String> {
        let effect_key = if target == "initial" { "initial-model".to_string() } else if target.starts_with("continuation:") { target.to_owned() } else { format!("deliver-batch:{target}") };
        if self.database.kernel_model_retry_needs_completion(&self.binding.run_id, &effect_key)? {
            // This fixed recovery instruction fits within the dispatch's 4096
            // reserved bytes. The durable source/compaction history is unchanged.
            view.push(serde_json::json!({
                "role": "user",
                "content": [{"type": "text", "text": context::INCOMPLETE_MODEL_RETRY_PROMPT}],
                "timestamp": 0,
            }));
        }
        Ok(view)
    }

    fn source_context(&self, target: &str) -> Result<Vec<Value>, String> {
        if target.starts_with("continuation:") { return Ok(self.stored_continuation_input(target)?.messages); }
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
        // The budget check is calibrated by this run's own provider usage
        // (cached input counts at full occupancy); with no valid pair it is
        // exactly the labelled heuristic fallback.
        if self.context_fits(&config, &view, extra_bytes)? {
            return Ok(false);
        }
        let previous = self
            .database
            .kernel_compaction_results(&self.binding.run_id, target)?;
        // Bounded stops, classified (#12): an unproductive pass means another
        // pass cannot help; the pass bound is per target, not a per-run
        // lifetime limit.
        if previous.last().is_some_and(|(_, result)| !result.applied) {
            return Err(context::NO_REDUCTION.into());
        }
        if previous.len() >= context::MAX_PASSES {
            return Err(context::INSUFFICIENT.into());
        }
        let turn = self
            .controller
            .lock()
            .map_err(|_| "Kernel coordinator lock poisoned")?
            .shadow_checkpoint(self.clock.now_monotonic_ms())
            .turn_id;
        let plan = match CompactionPlan::prepare_with_outcome(
            &self.binding.run_id,
            &turn,
            target,
            &view,
            &config,
        )? {
            context::PrepareOutcome::Planned(plan) => plan,
            context::PrepareOutcome::NoCandidates | context::PrepareOutcome::BelowMinGain => {
                // A single protected or recent item that alone exceeds the
                // window is a different failure from having nothing to fold.
                return Err(if context::single_item_too_large(&config, &view)? {
                    context::SINGLE_TOO_LARGE
                } else {
                    context::NO_CANDIDATES
                }
                .into());
            }
        };
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
        mut deliver: impl FnMut(
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
        // Owner and input are durable before the first external request, and every
        // attempt below shares this ONE deadline: a retry never renews the window,
        // and an unknown result is never replayed (recovery refuses a request whose
        // durable owner is already set).
        let deadline_wall_ms = since.saturating_add(self.binding.budgets.model_request_ms);
        let mut attempt: u32 = 0;
        let response = loop {
            let attempt_now = self.clock.read();
            let attempt_facts = self
                .controller
                .lock()
                .map_err(|_| "Kernel coordinator lock poisoned")?
                .shadow_checkpoint(attempt_now.monotonic_ms);
            let remaining = self.binding.budgets.limit_operation_ms(
                deadline_wall_ms.saturating_sub(attempt_now.wall_ms),
                attempt_facts.running_elapsed_ms,
            );
            let outcome = deliver(&self.binding, &plan.request, &token, remaining);
            match outcome {
                Ok(response) => break response,
                Err(error) => {
                    let failure = classify_compaction_failure(&error, &token);
                    let elapsed_ms = attempt_now.wall_ms.saturating_sub(since);
                    // A retry needs a settled, retryable failure, a fresh attempt
                    // inside the same window, and enough of that window left to be
                    // worth spending.
                    let retry = failure.is_retryable()
                        && attempt < context::RETRY_LIMIT
                        && remaining >= context::MIN_RETRY_BUDGET_MS;
                    let (category, http_status) = compaction_failure_evidence(&error);
                    self.apply(None, |controller, _| {
                        controller.fail_context_compaction(
                            &pending.id,
                            owner,
                            attempt + 1,
                            elapsed_ms,
                            remaining,
                            failure.as_str(),
                            category.as_deref(),
                            http_status,
                            retry,
                        )
                    })?;
                    if !retry {
                        return Err(context::FAILED.into());
                    }
                    // Cancellation outranks a retry: a cancelled Run must not spend
                    // another provider call.
                    token.check()?;
                    attempt += 1;
                }
            }
        };
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
                runtime, &config, api_key, binding, request, token, remaining, Some(self.database),
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

/// Classify one failed compaction attempt so the Host can tell a settled
/// provider rejection (safe to re-ask) from an unknown outcome (never replayed).
fn classify_compaction_failure(error: &str, token: &kernel::CancellationToken) -> context::CompactionFailure {
    use super::super::kernel_model_worker;
    if token.is_cancelled() {
        return context::CompactionFailure::Cancelled;
    }
    if let Some(evidence) = kernel_model_worker::settled_failure(error) {
        // Only a rejection that happened before generation is retryable.
        return if evidence.category == "provider_unavailable" {
            context::CompactionFailure::SettledRejection
        } else {
            context::CompactionFailure::UnknownOutcome
        };
    }
    if kernel_model_worker::is_reaped_transport_failure(error) {
        return context::CompactionFailure::UnknownOutcome;
    }
    if error.contains("invalid compaction response")
        || error.contains("identity mismatch")
        || error.contains("invalid compaction")
    {
        return context::CompactionFailure::InvalidResult;
    }
    if error.to_ascii_lowercase().contains("cancel") {
        return context::CompactionFailure::Cancelled;
    }
    context::CompactionFailure::Unclassified
}

/// The provider-side evidence of a settled failure, for the durable event only.
fn compaction_failure_evidence(error: &str) -> (Option<String>, Option<u16>) {
    match super::super::kernel_model_worker::settled_failure(error) {
        Some(evidence) => (Some(evidence.category), evidence.http_status),
        None => (None, None),
    }
}
