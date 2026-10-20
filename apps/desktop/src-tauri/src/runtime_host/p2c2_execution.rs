//! Owns one controlled verification. Executable receipts never leave this module.
#[allow(unused_imports)]
use crate::canonical_json::CanonicalJson;
use super::super::p2b_decision::{self as decision, DecisionFact, FactContext, FactOrigin};
use super::*;
use crate::runtime_host::attachment_compute::{
    authorized_delivery_root_typed, read_authorized_delivery_bytes_typed, DeliveryReadFailure,
};
use crate::runtime_host::delivery_checks_v2::{
    self as leaf, CandidateBinding, CheckLimits, CheckOutcome, CheckState, RuleEnvelope,
    TableCheckRequest, TextConcatRequest, TextSegment,
};
use leaf::text_csv::ParseInput;
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Debug, Clone, PartialEq, Eq)]
struct Obligation {
    id: String,
    check: String,
    discharged: bool,
    reason: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Coverage {
    obligations: Option<Vec<Obligation>>,
}

impl Coverage {
    #[allow(non_upper_case_globals)]
    pub(crate) const Unstated: Self = Self { obligations: None };

    pub(crate) fn fully_discharged(&self) -> bool {
        self.obligations.as_ref().is_some_and(|entries| {
            !entries.is_empty() && entries.iter().all(|entry| entry.discharged)
        })
    }

    pub(crate) fn detail(&self) -> String {
        match &self.obligations {
            None => "尚未提供可证明覆盖完整性的合同义务集合，本入口不判定为已确认".into(),
            Some(entries) if entries.is_empty() => "合同义务集合为空，不能视为覆盖完整".into(),
            Some(entries) => format!(
                "合同存在未履行的核验义务：{}",
                entries
                    .iter()
                    .filter(|entry| !entry.discharged)
                    .map(|entry| format!("{} [{}]: {}", entry.id, entry.check, entry.reason))
                    .collect::<Vec<_>>()
                    .join(", ")
            ),
        }
    }

    #[cfg(test)]
    pub(crate) fn single_check(check: &str, discharged: bool) -> Self {
        Self::test_declared(vec![(check.into(), discharged)])
    }

    #[cfg(test)]
    pub(crate) fn test_declared(entries: Vec<(String, bool)>) -> Self {
        Self {
            obligations: Some(
                entries
                    .into_iter()
                    .map(|(check, discharged)| Obligation {
                        id: check.clone(),
                        check,
                        discharged,
                        reason: "synthetic decision fixture".into(),
                    })
                    .collect(),
            ),
        }
    }
}

fn required(id: impl Into<String>, check: &str, reason: impl Into<String>) -> Obligation {
    Obligation {
        id: id.into(),
        check: check.into(),
        discharged: false,
        reason: reason.into(),
    }
}

fn obligations(contract: &Contract) -> Vec<Obligation> {
    match contract {
        Contract::Concat { .. } => vec![required(
            "concat.all_sources",
            leaf::check_name::TEXT_CONCAT,
            "本次完整来源集合、冻结选择/排序、安全守卫和真实 text_concat 尚未共同履行",
        )],
        Contract::DataSource {
            source,
            sheet,
            headers,
            date_required,
            metrics,
        } => {
            let mut entries = vec![
                required(
                    "data.table_columns",
                    leaf::check_name::TABLE_COLUMNS,
                    "候选尚未由真实有界表格叶核验",
                ),
                required(
                    "data.source_lineage",
                    "lineage",
                    format!("lineage 尚未实现：{} 的候选与来源关联未证", source.path),
                ),
            ];
            if !headers.is_empty() {
                entries.push(required(
                    "data.expected_headers",
                    leaf::check_name::TABLE_COLUMNS,
                    "table_columns 只检查头/宽度结构，不核验合同 headers 的名称与顺序",
                ));
            }
            if let Some(sheet) = sheet {
                entries.push(required(
                    "data.sheet",
                    "lineage",
                    format!("工作簿/工作表适配尚未接入，未核验 sheet={sheet}"),
                ));
            }
            if *date_required {
                entries.push(required(
                    "data.date",
                    "typed_metric",
                    "日期键/日粒度核验尚未实现",
                ));
            }
            for (index, metric) in metrics.iter().enumerate() {
                entries.push(required(
                    format!("data.metric.{index}:{}:{}", metric.column, metric.operation),
                    "typed_metric",
                    "typed_metric 尚未实现，每个 metric 独立未履行",
                ));
            }
            entries
        }
        Contract::FactQualifiers { .. } => vec![required(
            "fact.qualifiers",
            "fact_qualifiers",
            "v2 限定语叶尚未实现",
        )],
        Contract::ForbiddenMention { literal, .. } => vec![required(
            "forbidden.mention",
            "forbidden_mention",
            format!("v2 禁止提及叶尚未实现：{literal}"),
        )],
        Contract::ForbiddenAvailability { literal, .. } => vec![required(
            "forbidden.availability",
            "forbidden_availability",
            format!("v2 可用性声明叶尚未实现：{literal}"),
        )],
        Contract::Unbound { reason } => vec![required(
            "contract.unbound",
            "unbound",
            format!("未绑定合同：{reason}"),
        )],
    }
}

static NEXT_EXECUTION: AtomicU64 = AtomicU64::new(1);

#[cfg(test)]
thread_local! {
    static AFTER_CHECK: std::cell::RefCell<Option<Box<dyn FnOnce(&Database, &str)>>> = std::cell::RefCell::new(None);
}

#[cfg(test)]
fn test_after_check(hook: impl FnOnce(&Database, &str) + 'static) {
    AFTER_CHECK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
}

pub(super) fn execute(
    database: &Database,
    run: &str,
    item: &str,
    spec: &Value,
    path: Option<&Path>,
    finding: &Value,
    repair: bool,
) -> decision::DecisionResult {
    execute_with_limits(
        database,
        run,
        item,
        spec,
        path,
        finding,
        repair,
        MAX_VERIFY_BYTES,
        SOURCE_LIMIT,
        CheckLimits::default(),
    )
}

fn execute_with_limits(
    database: &Database,
    run: &str,
    item: &str,
    spec: &Value,
    path: Option<&Path>,
    finding: &Value,
    repair: bool,
    candidate_limit: u64,
    source_limit: u64,
    leaf_limits: CheckLimits,
) -> decision::DecisionResult {
    let execution_id = format!(
        "{}:{}",
        std::process::id(),
        NEXT_EXECUTION.fetch_add(1, Ordering::Relaxed)
    );
    let spec_hash = digest(&crate::canonical_json::to_vec(spec).expect("JSON value serialization"));
    let decoded = decode_spec(spec);
    let mut coverage = Coverage::Unstated;
    if let DecodedSpec::V2(rule) = &decoded {
        coverage = Coverage {
            obligations: Some(obligations(&rule.contract)),
        };
    }
    let mut reads = Vec::new();
    let mut drifts = Vec::new();
    let mut outcome = CheckOutcome::default();
    let mut candidate: Option<CandidateBinding> = None;
    let mut source_receipts = Vec::<Value>::new();
    let mut sources = BTreeMap::<String, Vec<u8>>::new();
    let mut row_bound = false;
    let root = authorized_delivery_root_typed(database, run)
        .map_err(|error| reads.push(decision::classify_root_failure(&error)))
        .ok();
    let mut authorization_changed = false;
    if let Some(root) = &root {
        let relative = path
            .and_then(Path::to_str)
            .and_then(|path| project_relative_path(path, root.to_str()));
        if let Some(relative) = relative {
            let result = read_authorized_delivery_bytes_typed(root, &relative, candidate_limit);
            reads.extend(decision::classify_read_result(&relative, &result));
            if let Ok(bytes) = result {
                let hash = digest(&bytes);
                candidate = Some(CandidateBinding::from_finding(
                    finding,
                    &relative,
                    &hash,
                    bytes.len() as u64,
                ));
                match decision::classify_checked_hash(finding) {
                    decision::CheckedHash::Recorded(recorded) if recorded != hash => drifts.push(
                        decision::candidate_hash_drift_fact(&recorded, &hash, &relative),
                    ),
                    decision::CheckedHash::Malformed { value } => drifts.push(
                        decision::candidate_hash_malformed_fact(&value, &hash, &relative),
                    ),
                    decision::CheckedHash::Absent(category) => {
                        reads.push(DecisionFact::unconfirmed(
                            FactOrigin::candidate(&relative, ""),
                            Some(leaf::reason_code::EVIDENCE_UNAVAILABLE),
                            format!("产物缺少可核验回执哈希（{category}）"),
                            None,
                        ))
                    }
                    _ => {}
                }
                // sourceRunId is the write-receipt origin, including a continued
                // parent Run. This execution's Run is bound by root + row + spec,
                // never inferred from that origin field.
                if let DecodedSpec::V2(rule) = &decoded {
                    // A caller cannot replace this row's frozen rule with another spec.
                    row_bound = bound_row(database, run, item, spec, &relative, root);
                    if !row_bound {
                        reads.push(DecisionFact::unconfirmed(
                            FactOrigin::CheckUnbound,
                            Some(leaf::reason_code::EVIDENCE_UNAVAILABLE),
                            "本次 run/item/path/spec 未绑定同一耐久合同行",
                            None,
                        ));
                    }
                    let references = contract_source_refs(&rule.contract);
                    let mut total = 0usize;
                    let within_source_count = references.len() <= MAX_SOURCES;
                    if !within_source_count {
                        let mut fact = DecisionFact::unconfirmed(
                            FactOrigin::Spec,
                            Some(leaf::reason_code::BUDGET_EXCEEDED),
                            "冻结来源数量超限，未执行来源读取或内容叶",
                            None,
                        );
                        fact.check = Some("delivery.source_count".into());
                        fact.expected = Some(format!("sources <= {MAX_SOURCES}"));
                        fact.actual = Some(references.len().to_string());
                        reads.push(fact);
                    }
                    for source in references.iter().take(if within_source_count {
                        references.len()
                    } else {
                        0
                    }) {
                        let Some(source_path) = project_relative_path(&source.path, root.to_str())
                        else {
                            reads.push(DecisionFact::unconfirmed(
                                FactOrigin::source(&source.path, &source.sha256),
                                None,
                                "来源不在当前授权项目内",
                                None,
                            ));
                            continue;
                        };
                        match read_authorized_delivery_bytes_typed(root, &source_path, source_limit)
                        {
                            Ok(source_bytes) => {
                                total = total.saturating_add(source_bytes.len());
                                if total > TOTAL_LIMIT {
                                    reads.push(decision::classify_read_failure(
                                        FactOrigin::source(&source.path, &source.sha256),
                                        &DeliveryReadFailure::Resource {
                                            limit: TOTAL_LIMIT as u64,
                                        },
                                    ));
                                    break;
                                }
                                let actual = digest(&source_bytes);
                                source_receipts.push(json!({"path":source.path,"expectedHash":source.sha256,"actualHash":actual,
                                    "expectedBytes":source.bytes,"actualBytes":source_bytes.len(),"firstLine":source.first_line,"lastLine":source.last_line}));
                                if actual != source.sha256
                                    || source_bytes.len() as u64 != source.bytes
                                {
                                    drifts.push(decision::source_hash_drift_fact(
                                        &source.sha256,
                                        &actual,
                                        &source.path,
                                    ));
                                }
                                sources.insert(source.path.clone(), source_bytes);
                            }
                            Err(error) => reads.push(decision::classify_read_failure(
                                FactOrigin::source(&source.path, &source.sha256),
                                &error,
                            )),
                        }
                    }
                    let all_read = sources.len() == references.len();
                    if row_bound && all_read && drifts.is_empty() && reads.is_empty() {
                        let binding = candidate.as_ref().expect("candidate read");
                        if let Ok(envelope) = RuleEnvelope::from_spec(spec) {
                            match &rule.contract {
                                Contract::Concat {
                                    sources: refs,
                                    selected,
                                    selection_order,
                                    tie_order,
                                    merge_order,
                                    ..
                                } => {
                                    let plan = concat_plan(
                                        refs,
                                        selected,
                                        selection_order,
                                        tie_order,
                                        merge_order,
                                    );
                                    if let Err(reason) = ensure_concat_writable(
                                        database,
                                        run,
                                        None,
                                        root.to_str().unwrap_or(""),
                                        path.expect("bound candidate"),
                                    ) {
                                        drifts.push(DecisionFact::refused(
                                            FactOrigin::candidate(&relative, &hash),
                                            leaf::reason_code::security::READONLY_INPUT_WRITE,
                                            reason,
                                        ));
                                    } else if let Err(reason) = plan {
                                        coverage.obligations.as_mut().unwrap()[0].reason = reason;
                                    } else {
                                        let segments = selected
                                            .iter()
                                            .map(|name| TextSegment {
                                                path: name,
                                                bytes: sources[name].as_slice(),
                                            })
                                            .collect::<Vec<_>>();
                                        let observation =
                                            leaf::text_csv::check_text_concat_budgeted(
                                                &TextConcatRequest {
                                                    rule_id: envelope
                                                        .rule_id
                                                        .as_deref()
                                                        .unwrap_or("content:concat"),
                                                    envelope: &envelope,
                                                    segments: &segments,
                                                    actual: &bytes,
                                                    binding,
                                                },
                                                &leaf_limits,
                                                &leaf::budget::SystemClock,
                                            );
                                        let entry = &mut coverage.obligations.as_mut().unwrap()[0];
                                        entry.discharged = observation.state == CheckState::Passed;
                                        entry.reason = if entry.discharged {
                                            "本次受控读取及真实叶已通过".into()
                                        } else {
                                            "本次真实 text_concat 未通过".into()
                                        };
                                        outcome.observations.push(observation);
                                    }
                                }
                                Contract::DataSource { .. } => {
                                    let extension = Path::new(&relative)
                                        .extension()
                                        .and_then(|ext| ext.to_str())
                                        .unwrap_or("")
                                        .to_ascii_lowercase();
                                    if matches!(extension.as_str(), "csv" | "tsv") {
                                        let observation = match leaf::text_csv::parse_table(
                                            &bytes,
                                            &envelope.policy.table,
                                            &leaf_limits,
                                        ) {
                                            Ok(parsed) => leaf::text_csv::check_table_columns(
                                                &TableCheckRequest {
                                                    rule_id: envelope
                                                        .rule_id
                                                        .as_deref()
                                                        .unwrap_or("content:data"),
                                                    parsed: &parsed,
                                                    binding,
                                                    require_unique_headers: false,
                                                    allow_zero_records: false,
                                                },
                                            ),
                                            Err(error) => leaf::text_csv::table_parse_observation(
                                                envelope
                                                    .rule_id
                                                    .as_deref()
                                                    .unwrap_or("content:data"),
                                                leaf::check_name::TABLE_COLUMNS,
                                                binding,
                                                ParseInput::Candidate,
                                                &error,
                                            ),
                                        };
                                        let entry = &mut coverage.obligations.as_mut().unwrap()[0];
                                        entry.discharged = observation.state == CheckState::Passed;
                                        entry.reason = "本次真实候选 CSV/TSV 结构叶已执行；其他 DataSource 义务独立保留".into();
                                        outcome.observations.push(observation);
                                    } else {
                                        coverage.obligations.as_mut().unwrap()[0].reason =
                                            "候选工作簿/PDF表格适配尚未具备，不用简化 CSV 冒充"
                                                .into();
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                    #[cfg(test)]
                    AFTER_CHECK.with(|slot| {
                        if let Some(hook) = slot.borrow_mut().take() {
                            hook(database, run);
                        }
                    });
                    // Re-read identities via the same authorization and byte ceilings.
                    if !outcome.observations.is_empty() {
                        match read_authorized_delivery_bytes_typed(root, &relative, candidate_limit)
                        {
                            Ok(current) if digest(&current) != hash => {
                                drifts.push(decision::candidate_hash_drift_fact(
                                    &hash,
                                    &digest(&current),
                                    &relative,
                                ))
                            }
                            Err(error) => reads.push(decision::classify_read_failure(
                                FactOrigin::candidate(&relative, &hash),
                                &error,
                            )),
                            _ => {}
                        }
                        for source in references {
                            if let Some(source_path) =
                                project_relative_path(&source.path, root.to_str())
                            {
                                match read_authorized_delivery_bytes_typed(
                                    root,
                                    &source_path,
                                    source_limit,
                                ) {
                                    Ok(current)
                                        if digest(&current) != source.sha256
                                            || current.len() as u64 != source.bytes =>
                                    {
                                        drifts.push(decision::source_hash_drift_fact(
                                            &source.sha256,
                                            &digest(&current),
                                            &source.path,
                                        ))
                                    }
                                    Err(error) => reads.push(decision::classify_read_failure(
                                        FactOrigin::source(&source.path, &source.sha256),
                                        &error,
                                    )),
                                    _ => {}
                                }
                            }
                        }
                        row_bound &= bound_row(database, run, item, spec, &relative, root);
                        if !row_bound {
                            reads.push(DecisionFact::unconfirmed(
                                FactOrigin::Spec,
                                Some(leaf::reason_code::EVIDENCE_UNAVAILABLE),
                                "核验期间行合同身份改变或目标路径改绑，本次凭据不能履约",
                                None,
                            ));
                        }
                    }
                }
            }
        } else {
            reads.push(DecisionFact::unconfirmed(
                FactOrigin::CheckUnbound,
                None,
                "内容核验没有绑定真实产物",
                None,
            ));
        }
        authorization_changed = authorized_delivery_root_typed(database, run)
            .as_deref()
            .ok()
            != Some(root.as_path());
    }
    let context = candidate.as_ref().map(|binding| FactContext::Candidate {
        path: binding.path.clone(),
        sha256: binding.sha256.clone(),
    });
    if authorization_changed || !reads.is_empty() || !drifts.is_empty() || !row_bound {
        if let Some(entries) = coverage.obligations.as_mut() {
            for entry in entries.iter_mut().filter(|entry| entry.discharged) {
                entry.discharged = false;
                entry.reason =
                    "真实叶已运行，但本次 run/item/spec/path/hash/授权上下文不再有效，凭据不能履约"
                        .into();
            }
        }
    }
    let obligation_audit = coverage.obligations.as_ref().map(|entries| {
        entries
            .iter()
            .map(|entry| {
                json!({
        "id":entry.id,"check":entry.check,"discharged":entry.discharged,"reason":entry.reason})
            })
            .collect::<Vec<_>>()
    });
    let mut result = decision::classify_and_decide(
        &decision::FactInput {
            spec,
            read_facts: reads,
            drift_facts: drifts,
            outcome: (!outcome.observations.is_empty()).then_some(&outcome),
            authorization_changed,
            coverage,
            context,
        },
        repair,
    );
    if matches!(decoded, DecodedSpec::V2(_)) {
        let mut audit = DecisionFact::passed(FactOrigin::Spec);
        audit.check = Some("delivery.execution".into());
        audit.rule_id = spec["ruleId"].as_str().map(str::to_owned);
        audit.checker_version = Some(2);
        audit.detail = Some(json!({"executionId":execution_id,"runId":run,"itemKey":item,"specHash":spec_hash,
            "candidate":candidate,"sources":source_receipts,"rowBound":row_bound,"checks":outcome.observations,
            "authorizationRechecked":root.is_some(),"obligations":obligation_audit,"auditOnly":true}).canonical_string());
        result.facts.push(audit);
    }
    result
}

fn bound_row(
    database: &Database,
    run: &str,
    item: &str,
    spec: &Value,
    relative: &str,
    root: &Path,
) -> bool {
    !item.trim().is_empty() && !item.chars().any(char::is_control)
        && database.delivery_checklist(run).ok().is_some_and(|rows| rows.iter().any(|row| row.item_key == item
            && row.target_path.as_deref().and_then(|target| project_relative_path(target, root.to_str())).as_deref() == Some(relative)))
        && database.delivery_item_requirements(run, item).ok().is_some_and(|requirements| requirements.iter().any(|requirement|
            matches!(&requirement.kind, StoredRequirementKind::ContentContract { spec: stored } if stored == spec)))
}

fn concat_plan(
    sources: &[Source],
    selected: &[String],
    selection: &str,
    tie: &str,
    merge: &str,
) -> Result<(), String> {
    if sources.is_empty() || selected.is_empty() || selected.len() > sources.len() {
        return Err("完整 Concat 来源/选择集合缺失，不能真空通过".into());
    }
    let unique = sources
        .iter()
        .map(|source| source.path.as_str())
        .collect::<BTreeSet<_>>();
    if unique.len() != sources.len()
        || selected.iter().collect::<BTreeSet<_>>().len() != selected.len()
    {
        return Err("Concat 来源或选择身份重复，不能证明完整集合".into());
    }
    let mut ranked = sources.iter().collect::<Vec<_>>();
    match (selection, tie, merge) {
        ("bytes_ascending" | "bytes_descending", "filename_ascending", "filename_ascending") => {
            ranked.sort_by(|a, b| {
                let order = a.bytes.cmp(&b.bytes);
                let order = if selection == "bytes_descending" {
                    order.reverse()
                } else {
                    order
                };
                order.then_with(|| file_name(&a.path).cmp(file_name(&b.path)))
            });
            let mut expected = ranked
                .iter()
                .take(selected.len())
                .map(|source| source.path.clone())
                .collect::<Vec<_>>();
            expected.sort_by(|a, b| file_name(a).cmp(file_name(b)));
            if &expected != selected {
                return Err("冻结 selected 与声明的选择/平手/合并排序不一致，未执行内容叶".into());
            }
        }
        ("source", "first", "source")
            if sources
                .iter()
                .map(|source| &source.path)
                .eq(selected.iter()) => {}
        _ => return Err("Concat 的选择/平手/合并排序尚无完整适配，保持未履行".into()),
    }
    Ok(())
}

#[cfg(test)]
#[path = "p2c2_execution_tests.rs"]
mod tests;
