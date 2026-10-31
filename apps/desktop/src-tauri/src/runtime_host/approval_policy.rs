//! A1: the single Host approval engine for Fox's own tools.
//!
//! Fox does not confine its own commands in a sandbox. What stands between a
//! model's tool call and the user's machine is this decision, made by the Host
//! from facts it can check itself:
//!
//! 1. **block** – built-in dangerous deletions (the project root, a drive or
//!    filesystem root, the home directory, system directories) and the project's
//!    own `block` rules. Refused in every mode, full access included.
//! 2. **protected / outside paths** – `.git`, `.fox`, shell profiles, and any
//!    path that leaves the project root. Only full access lets them through.
//! 3. **mode** – read only / ask / auto / full access (see the table in
//!    `docs` and in [`decide`]).
//! 4. **allow / ask rules** – project rules matched by token prefix. A compound
//!    command needs every segment covered, and a broad prefix (`git`) never
//!    covers a high-risk segment (`git push`).
//!
//! Risk is the Host's own judgement over the command's tokens. A model may add
//! a `risk` field to a command tool's input, but it can only raise the risk,
//! never lower it.
use serde::Serialize;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};

/// The four execution modes of a Fox conversation. Stored as the legacy
/// `permission_mode` (`read_only | ask | allow`) plus `conversations.full_access`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    ReadOnly,
    Ask,
    Auto,
    FullAccess,
}

impl AccessMode {
    /// Full access is effective only when the stored mode is `allow` AND the
    /// conversation's full-access flag is set.
    pub fn from_stored(mode: &str, full_access: bool) -> Result<Self, String> {
        match mode {
            "read_only" => Ok(Self::ReadOnly),
            "ask" => Ok(Self::Ask),
            "allow" if full_access => Ok(Self::FullAccess),
            "allow" => Ok(Self::Auto),
            other => Err(format!("permission.invalid_mode: unknown stored permission mode {other}")),
        }
    }

    /// Parse the four-mode vocabulary the UI sends (`allow` is accepted as `auto`).
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "read_only" => Ok(Self::ReadOnly),
            "ask" => Ok(Self::Ask),
            "auto" | "allow" => Ok(Self::Auto),
            "full_access" => Ok(Self::FullAccess),
            other => Err(format!("permission.invalid_mode: unknown access mode {other}")),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Ask => "ask",
            Self::Auto => "auto",
            Self::FullAccess => "full_access",
        }
    }

    /// The legacy stored `permission_mode` for this mode.
    pub fn stored_mode(self) -> &'static str {
        match self {
            Self::ReadOnly => "read_only",
            Self::Ask => "ask",
            Self::Auto | Self::FullAccess => "allow",
        }
    }

    pub fn full_access(self) -> bool {
        matches!(self, Self::FullAccess)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum RuleEffect {
    Allow,
    Ask,
    Block,
}

impl RuleEffect {
    pub fn parse(value: &str) -> Result<Self, String> {
        match value {
            "allow" => Ok(Self::Allow),
            "ask" => Ok(Self::Ask),
            "block" => Ok(Self::Block),
            other => Err(format!("permission.invalid_rule: unknown rule effect {other}")),
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Allow => "allow",
            Self::Ask => "ask",
            Self::Block => "block",
        }
    }
}

/// One project rule as the engine sees it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PolicyRule {
    pub id: String,
    pub effect: RuleEffect,
    /// The command tool the rule names (`run_command`, `test_run`, ...) or `*`.
    pub tool: String,
    pub pattern: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Risk {
    Low,
    High,
}

impl Risk {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Low => "low",
            Self::High => "high",
        }
    }
}

/// The category a tool call is judged in (rows of the decision table).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolClass {
    Read,
    ProjectWrite,
    Command,
    Network,
    /// A path outside the project root or a protected path.
    PathBoundary,
    Other,
}

impl ToolClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::ProjectWrite => "project_write",
            Self::Command => "command",
            Self::Network => "network",
            Self::PathBoundary => "path_boundary",
            Self::Other => "other",
        }
    }
}

/// What a tool call is, in the terms the engine decides on. Built by the
/// caller from the Host's own prepared preview, never from model prose.
#[derive(Debug, Clone)]
pub enum ToolSubject {
    Read,
    Write { target: PathBuf },
    Command { command: String, cwd: PathBuf, model_risk: Option<String> },
    Network,
    Other,
}

pub const COMMAND_TOOLS: [&str; 4] = ["run_command", "test_run", "code_check", "format_code"];

pub fn is_command_tool(tool: &str) -> bool {
    COMMAND_TOOLS.contains(&tool)
}

/// Classify a tool by name and input. `command`/`cwd` come from the Host's
/// prepared preview (for capability process tools this is the exact program
/// line the Host will run).
pub fn subject_for(tool: &str, input: &Value, command: Option<&str>, cwd: Option<&Path>, write_target: Option<&Path>) -> ToolSubject {
    let model_risk = input.get("risk").and_then(Value::as_str).map(str::to_owned);
    match tool {
        "read" | "list" | "search" | "glob" | "grep" | "git_read" | "sqlite_read" | "structured_data"
        | "tabular_data" | "read_tool_result" => ToolSubject::Read,
        "run_command" if matches!(input.get("action").and_then(Value::as_str), Some("status" | "output" | "cancel")) => ToolSubject::Read,
        "write_file" | "edit_file" => match write_target {
            Some(target) => ToolSubject::Write { target: target.to_path_buf() },
            None => ToolSubject::Other,
        },
        tool if is_command_tool(tool) => match (command, cwd) {
            (Some(command), Some(cwd)) => ToolSubject::Command { command: command.to_owned(), cwd: cwd.to_path_buf(), model_risk },
            _ => ToolSubject::Other,
        },
        "web_search" | "web_read" | "http_request" | "system_info" => ToolSubject::Network,
        _ => ToolSubject::Other,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case", tag = "verdict")]
pub enum Verdict {
    Allow,
    Ask,
    Deny { code: String, message: String },
}

/// Why a call was allowed without asking (written to the audit log).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AllowReason {
    Read,
    Mode,
    FullAccess,
    Rule(Vec<PolicyRule>),
}

impl AllowReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Mode => "mode",
            Self::FullAccess => "full_access",
            Self::Rule(_) => "rule",
        }
    }
}

#[derive(Debug, Clone)]
pub struct PolicyDecision {
    pub verdict: Verdict,
    pub mode: AccessMode,
    pub class: ToolClass,
    pub risk: Risk,
    /// Host findings that shaped the decision (`git_push`, `outside_project`, ...).
    pub findings: Vec<String>,
    pub allow_reason: Option<AllowReason>,
    /// The `ask` rule that forced a prompt, or the `block` rule that refused.
    pub forcing_rule: Option<PolicyRule>,
    /// Prefix the approval card offers for "always allow in this project".
    pub suggested_prefix: Option<String>,
    /// Whether "always allow in this project" can be offered at all.
    pub rule_eligible: bool,
}

impl PolicyDecision {
    pub fn is_allow(&self) -> bool {
        matches!(self.verdict, Verdict::Allow)
    }
    pub fn is_ask(&self) -> bool {
        matches!(self.verdict, Verdict::Ask)
    }
    /// 内置 block 或项目 block 规则：拒绝码 `permission.blocked`。
    pub fn is_block(&self) -> bool {
        matches!(&self.verdict, Verdict::Deny { code, .. } if code == "permission.blocked")
    }
    /// Short machine summary for the approval card's `policy` field.
    pub fn card_json(&self) -> Value {
        json!({
            "mode": self.mode.as_str(),
            "class": self.class.as_str(),
            "risk": self.risk.as_str(),
            "findings": self.findings,
            "forcingRule": self.forcing_rule.as_ref().map(|rule| json!({"id":rule.id,"effect":rule.effect.as_str(),"pattern":rule.pattern})),
        })
    }
    /// The `projectRule` field of the approval card.
    pub fn project_rule_json(&self, tool: &str) -> Value {
        json!({
            "eligible": self.rule_eligible,
            "tool": tool,
            "suggestedPrefix": self.suggested_prefix,
        })
    }
    pub fn rule_ids(&self) -> Vec<String> {
        match &self.allow_reason {
            Some(AllowReason::Rule(rules)) => rules.iter().map(|rule| rule.id.clone()).collect(),
            _ => Vec::new(),
        }
    }
}

/// The `hostExecution` field of an approval card for command tools: Fox runs
/// them on the Host as the current user, with no sandbox.
pub fn host_execution_json() -> Value {
    json!({
        "isolation": "none",
        "containment": if cfg!(windows) { "fox_job" } else { "process_group" },
        "identity": "current_user",
        "note": "在 Host 上以当前用户身份执行，不受沙箱限制；Fox 只负责超时、取消和清理整棵进程树。",
    })
}

/// Everything the engine needs, as values (no I/O).
pub struct PolicyRequest<'a> {
    pub tool: &'a str,
    pub subject: &'a ToolSubject,
    pub mode: AccessMode,
    pub project_root: Option<&'a Path>,
    pub home: Option<&'a Path>,
    pub rules: &'a [PolicyRule],
}

/// The decision table:
///
/// | class | read only | ask | auto | full access |
/// |---|---|---|---|---|
/// | read | allow | allow | allow | allow |
/// | project write | deny | ask | allow | allow |
/// | command | deny | ask (allow rules) | low allow, high ask (allow rules) | allow |
/// | network | ask | ask | allow | allow |
/// | outside / protected | deny | ask | ask | allow |
/// | block | deny | deny | deny | deny |
pub fn decide(request: &PolicyRequest<'_>) -> PolicyDecision {
    let mode = request.mode;
    let mut decision = PolicyDecision {
        verdict: Verdict::Ask,
        mode,
        class: ToolClass::Other,
        risk: Risk::Low,
        findings: Vec::new(),
        allow_reason: None,
        forcing_rule: None,
        suggested_prefix: None,
        rule_eligible: false,
    };
    let deny = |code: &str, message: String| Verdict::Deny { code: code.to_owned(), message };
    match request.subject {
        ToolSubject::Read => {
            decision.class = ToolClass::Read;
            decision.verdict = Verdict::Allow;
            decision.allow_reason = Some(AllowReason::Read);
        }
        ToolSubject::Network => {
            decision.class = ToolClass::Network;
            if matches!(mode, AccessMode::Auto | AccessMode::FullAccess) {
                decision.verdict = Verdict::Allow;
                decision.allow_reason = Some(if mode.full_access() { AllowReason::FullAccess } else { AllowReason::Mode });
            }
        }
        ToolSubject::Other => {
            decision.class = ToolClass::Other;
            if mode.full_access() {
                decision.verdict = Verdict::Allow;
                decision.allow_reason = Some(AllowReason::FullAccess);
            }
        }
        ToolSubject::Write { target } => {
            let root = request.project_root;
            let boundary = path_boundary(target, root, request.home);
            if let Some(finding) = boundary {
                decision.class = ToolClass::PathBoundary;
                decision.risk = Risk::High;
                decision.findings.push(finding.to_owned());
                decision.verdict = match mode {
                    AccessMode::ReadOnly => deny("permission.read_only", format!("tool {} is blocked in read-only mode", request.tool)),
                    AccessMode::FullAccess => {
                        decision.allow_reason = Some(AllowReason::FullAccess);
                        Verdict::Allow
                    }
                    _ => Verdict::Ask,
                };
            } else {
                decision.class = ToolClass::ProjectWrite;
                decision.verdict = match mode {
                    AccessMode::ReadOnly => deny("permission.read_only", format!("tool {} is blocked in read-only mode", request.tool)),
                    AccessMode::Ask => Verdict::Ask,
                    AccessMode::Auto => {
                        decision.allow_reason = Some(AllowReason::Mode);
                        Verdict::Allow
                    }
                    AccessMode::FullAccess => {
                        decision.allow_reason = Some(AllowReason::FullAccess);
                        Verdict::Allow
                    }
                };
            }
        }
        ToolSubject::Command { command, cwd, model_risk } => {
            decide_command(request, &mut decision, command, cwd, model_risk.as_deref());
        }
    }
    decision
}

fn decide_command(request: &PolicyRequest<'_>, decision: &mut PolicyDecision, command: &str, cwd: &Path, model_risk: Option<&str>) {
    let mode = request.mode;
    let analysis = analyze_command(command, cwd, request.project_root, request.home);
    decision.class = ToolClass::Command;
    decision.risk = analysis.risk;
    decision.findings = analysis.findings.clone();
    if model_risk.is_some_and(|risk| risk.eq_ignore_ascii_case("high")) && decision.risk < Risk::High {
        decision.risk = Risk::High;
        decision.findings.push("model_declared_high".into());
    }
    // 1. block: built-in dangerous deletions, then project block rules.
    // block 先于 path_boundary，class 保持 Command。
    if let Some(block) = &analysis.block {
        decision.findings.push("builtin_block".into());
        decision.verdict = Verdict::Deny {
            code: "permission.blocked".into(),
            message: format!("[permission.blocked] {block}。这条内置禁止规则在任何模式下都生效（包括完全访问），命令没有执行。"),
        };
        return;
    }
    let segments = &analysis.segments;
    if let Some(rule) = request.rules.iter().filter(|rule| rule.effect == RuleEffect::Block && rule_names_tool(rule, request.tool))
        .find(|rule| segments.iter().any(|segment| rule_prefix_matches(rule, segment)))
    {
        decision.forcing_rule = Some(rule.clone());
        decision.findings.push("project_block_rule".into());
        decision.verdict = Verdict::Deny {
            code: "permission.blocked".into(),
            message: format!("[permission.blocked] 本项目的禁止规则「{}」命中了这条命令，命令没有执行。", rule.pattern),
        };
        return;
    }
    // 2. protected / outside paths: only full access lets them through.
    if analysis.path_boundary {
        decision.class = ToolClass::PathBoundary;
        decision.verdict = match mode {
            AccessMode::ReadOnly => Verdict::Deny { code: "permission.read_only".into(), message: format!("tool {} is blocked in read-only mode", request.tool) },
            AccessMode::FullAccess => {
                decision.allow_reason = Some(AllowReason::FullAccess);
                Verdict::Allow
            }
            _ => Verdict::Ask,
        };
        return;
    }
    // 3. mode.
    match mode {
        AccessMode::ReadOnly => {
            decision.verdict = Verdict::Deny { code: "permission.read_only".into(), message: format!("tool {} is blocked in read-only mode", request.tool) };
            return;
        }
        AccessMode::FullAccess => {
            decision.verdict = Verdict::Allow;
            decision.allow_reason = Some(AllowReason::FullAccess);
            return;
        }
        AccessMode::Ask | AccessMode::Auto => {}
    }
    decision.rule_eligible = !analysis.opaque && !segments.is_empty();
    decision.suggested_prefix = if decision.rule_eligible { suggested_prefix(segments) } else { None };
    // 4. ask rules force a prompt; allow rules cover segment by segment.
    if let Some(rule) = request.rules.iter().filter(|rule| rule.effect == RuleEffect::Ask && rule_names_tool(rule, request.tool))
        .find(|rule| segments.iter().any(|segment| rule_prefix_matches(rule, segment)))
    {
        decision.forcing_rule = Some(rule.clone());
        decision.verdict = Verdict::Ask;
        return;
    }
    if decision.rule_eligible && !(model_risk.is_some_and(|risk| risk.eq_ignore_ascii_case("high")) && analysis.risk < Risk::High) {
        let allow_rules: Vec<&PolicyRule> = request.rules.iter()
            .filter(|rule| rule.effect == RuleEffect::Allow && rule_names_tool(rule, request.tool)).collect();
        let mut used: Vec<PolicyRule> = Vec::new();
        let covered = segments.iter().all(|segment| {
            match allow_rules.iter().find(|rule| rule_covers_segment(rule, segment)) {
                Some(rule) => {
                    if !used.iter().any(|known| known.id == rule.id) {
                        used.push((*rule).clone());
                    }
                    true
                }
                None => false,
            }
        });
        if covered {
            decision.verdict = Verdict::Allow;
            decision.allow_reason = Some(AllowReason::Rule(used));
            return;
        }
    }
    // 5. default.
    if mode == AccessMode::Auto && decision.risk == Risk::Low {
        decision.verdict = Verdict::Allow;
        decision.allow_reason = Some(AllowReason::Mode);
    } else {
        decision.verdict = Verdict::Ask;
    }
}

fn rule_names_tool(rule: &PolicyRule, tool: &str) -> bool {
    rule.tool == "*" || rule.tool == tool
}

/// Token-prefix match: every rule token equals the segment's token at the same
/// position (`npm test` matches `npm test -- foo`, never `npm testx`).
fn rule_prefix_matches(rule: &PolicyRule, segment: &Segment) -> bool {
    let Ok(pattern) = parse_rule_pattern(&rule.pattern) else { return false };
    if pattern.len() > segment.words.len() || segment.substitution {
        return false;
    }
    pattern.iter().zip(segment.words.iter()).enumerate().all(|(index, (want, have))| {
        if index == 0 { command_name(want) == command_name(have) } else { want == have }
    })
}

/// An allow rule covers a segment when it prefix-matches and, for a high-risk
/// segment, the rule itself names the high-risk operation (so `git` never
/// covers `git push`, but `git push` does).
fn rule_covers_segment(rule: &PolicyRule, segment: &Segment) -> bool {
    if !rule_prefix_matches(rule, segment) {
        return false;
    }
    if segment.risk == Risk::Low {
        return true;
    }
    let Ok(pattern) = parse_rule_pattern(&rule.pattern) else { return false };
    let mut findings = Vec::new();
    segment_risk(&pattern, &mut findings) == Risk::High
        && segment.findings.iter().all(|finding| findings.contains(finding))
}

/// Validate a rule pattern: one plain command segment, no operators, no
/// redirections, no substitutions.
pub fn parse_rule_pattern(pattern: &str) -> Result<Vec<String>, String> {
    let trimmed = pattern.trim();
    if trimmed.is_empty() || trimmed.len() > 500 {
        return Err("permission.invalid_rule: 规则前缀不能为空，且不超过 500 个字符".into());
    }
    let parsed = tokenize(trimmed);
    if parsed.len() != 1 {
        return Err("permission.invalid_rule: 规则前缀只能是一条命令，不能包含 &&、;、| 等连接符".into());
    }
    let segment = &parsed[0];
    if segment.substitution || !segment.redirects.is_empty() || segment.words.is_empty() {
        return Err("permission.invalid_rule: 规则前缀不能包含重定向或命令替换".into());
    }
    Ok(segment.words.clone())
}

/// Whether `pattern` (an allow rule) covers every segment of `command`.
pub fn pattern_covers_command(pattern: &str, command: &str, cwd: &Path, project_root: Option<&Path>, home: Option<&Path>) -> bool {
    let rule = PolicyRule { id: "candidate".into(), effect: RuleEffect::Allow, tool: "*".into(), pattern: pattern.to_owned() };
    let analysis = analyze_command(command, cwd, project_root, home);
    analysis.block.is_none() && !analysis.path_boundary && !analysis.opaque && !analysis.segments.is_empty()
        && analysis.segments.iter().all(|segment| rule_covers_segment(&rule, segment))
}

fn suggested_prefix(segments: &[Segment]) -> Option<String> {
    let first = segments.first()?;
    let words = &first.words;
    let head = words.first()?;
    if let Some(signature) = &first.high_signature {
        return Some(signature.join(" "));
    }
    let mut prefix = vec![head.clone()];
    if let Some(second) = words.get(1) {
        let plain = !second.starts_with('-') && !second.starts_with('/') && !looks_like_path(second) && !second.contains('=');
        if plain {
            prefix.push(second.clone());
        }
    }
    Some(prefix.join(" "))
}

// ---------------------------------------------------------------------------
// Command analysis
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default)]
struct Segment {
    words: Vec<String>,
    redirects: Vec<String>,
    /// The previous segment was joined to this one by a pipe.
    piped: bool,
    substitution: bool,
    risk: Risk,
    findings: Vec<String>,
    /// The tokens that name the high-risk operation (`git push`, `rm -rf`).
    high_signature: Option<Vec<String>>,
}

impl Default for Risk {
    fn default() -> Self {
        Risk::Low
    }
}

#[derive(Debug, Clone)]
struct CommandAnalysis {
    segments: Vec<Segment>,
    risk: Risk,
    findings: Vec<String>,
    block: Option<String>,
    path_boundary: bool,
    /// Something the Host cannot see through (command substitution, a nested
    /// shell script): rules never cover it.
    opaque: bool,
}

fn windows_syntax() -> bool {
    cfg!(windows)
}

/// Split a command line into segments at `&&`, `||`, `;`, `|`, `&` and new
/// lines, honouring quotes. Backslash escapes only on non-Windows hosts
/// (on Windows it is the path separator).
fn tokenize(command: &str) -> Vec<Segment> {
    tokenize_with(command, !windows_syntax())
}

/// 与 [`tokenize`] 相同，但由调用方指定是否处理反斜杠转义。
/// Host 默认方言和另一种方言都会用来做 block 检测（pi 在 Windows 上实际是 bash）。
fn tokenize_with(command: &str, escapes: bool) -> Vec<Segment> {
    let mut segments = Vec::new();
    let mut current = Segment::default();
    let mut word = String::new();
    let mut word_started = false;
    let mut redirect_next = false;
    let mut chars = command.chars().peekable();
    let mut quote: Option<char> = None;
    let flush_word = |current: &mut Segment, word: &mut String, started: &mut bool, redirect_next: &mut bool| {
        if *started {
            if *redirect_next {
                current.redirects.push(std::mem::take(word));
                *redirect_next = false;
            } else {
                current.words.push(std::mem::take(word));
            }
            *started = false;
        }
    };
    while let Some(ch) = chars.next() {
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            } else if ch == '\\' && escapes && q == '"' {
                if let Some(next) = chars.next() {
                    word.push(next);
                }
            } else {
                if q == '"' && (ch == '`' || (ch == '$' && chars.peek() == Some(&'('))) {
                    current.substitution = true;
                }
                word.push(ch);
            }
            continue;
        }
        match ch {
            '\'' | '"' => {
                quote = Some(ch);
                word_started = true;
            }
            '\\' if escapes => {
                if let Some(next) = chars.next() {
                    word.push(next);
                    word_started = true;
                }
            }
            ' ' | '\t' => flush_word(&mut current, &mut word, &mut word_started, &mut redirect_next),
            '\n' | '\r' | ';' => {
                flush_word(&mut current, &mut word, &mut word_started, &mut redirect_next);
                push_segment(&mut segments, &mut current, false);
            }
            '&' => {
                flush_word(&mut current, &mut word, &mut word_started, &mut redirect_next);
                if chars.peek() == Some(&'&') {
                    chars.next();
                }
                push_segment(&mut segments, &mut current, false);
            }
            '|' => {
                flush_word(&mut current, &mut word, &mut word_started, &mut redirect_next);
                if chars.peek() == Some(&'|') {
                    chars.next();
                    push_segment(&mut segments, &mut current, false);
                } else {
                    push_segment(&mut segments, &mut current, true);
                }
            }
            '>' | '<' => {
                // `2>` / `2>&1`: a pure-digit word right before is the fd.
                if word_started && !word.is_empty() && word.chars().all(|c| c.is_ascii_digit()) {
                    word.clear();
                    word_started = false;
                } else {
                    flush_word(&mut current, &mut word, &mut word_started, &mut redirect_next);
                }
                if ch == '<' && chars.peek() == Some(&'(') {
                    current.substitution = true;
                }
                if ch == '>' && chars.peek() == Some(&'(') {
                    current.substitution = true;
                }
                if chars.peek() == Some(&ch) {
                    chars.next();
                }
                if chars.peek() == Some(&'&') {
                    // `>&1`: duplicate a descriptor, no path follows.
                    chars.next();
                    while chars.peek().is_some_and(|c| c.is_ascii_digit()) {
                        chars.next();
                    }
                    continue;
                }
                redirect_next = true;
            }
            '`' => {
                current.substitution = true;
                word.push(ch);
                word_started = true;
            }
            '$' if chars.peek() == Some(&'(') => {
                current.substitution = true;
                word.push(ch);
                word_started = true;
            }
            _ => {
                word.push(ch);
                word_started = true;
            }
        }
    }
    flush_word(&mut current, &mut word, &mut word_started, &mut redirect_next);
    push_segment(&mut segments, &mut current, false);
    segments
}

fn push_segment(segments: &mut Vec<Segment>, current: &mut Segment, next_piped: bool) {
    let finished = std::mem::take(current);
    if !finished.words.is_empty() || !finished.redirects.is_empty() || finished.substitution {
        segments.push(finished);
    }
    current.piped = next_piped;
}

/// `C:\tools\Git.EXE` → `git`.
fn command_name(word: &str) -> String {
    let base = word.rsplit(['/', '\\']).next().unwrap_or(word).to_ascii_lowercase();
    for suffix in [".exe", ".cmd", ".bat", ".com", ".ps1"] {
        if let Some(stripped) = base.strip_suffix(suffix) {
            return stripped.to_owned();
        }
    }
    base
}

const SHELLS: [&str; 9] = ["sh", "bash", "zsh", "dash", "fish", "cmd", "powershell", "pwsh", "ksh"];
const DELETE_COMMANDS: [&str; 8] = ["rm", "del", "erase", "rd", "rmdir", "remove-item", "ri", "rimraf"];
/// cmd 内建和常见 Windows 程序。这些命令的 `/x`、`//x` 是开关，不是路径。
const WINDOWS_NATIVE_COMMANDS: [&str; 44] = [
    "dir", "del", "erase", "rd", "rmdir", "copy", "xcopy", "robocopy", "move", "ren", "rename", "type", "more", "findstr", "where",
    "attrib", "tree", "cmd", "start", "tasklist", "taskkill", "ipconfig", "ping", "net", "sc", "reg", "icacls", "takeown", "cacls",
    "fc", "comp", "chcp", "cls", "cd", "chdir", "pushd", "popd", "timeout", "schtasks", "wmic", "certutil", "bitsadmin", "shutdown", "format",
];
const FETCHERS: [&str; 7] = ["curl", "wget", "iwr", "invoke-webrequest", "irm", "invoke-restmethod", "fetch"];
const INTERPRETERS: [&str; 8] = ["python", "python3", "node", "perl", "ruby", "iex", "invoke-expression", "php"];

/// Skip leading wrappers (`env`, `nohup`, `time`, assignments) to the command
/// that actually runs.
fn effective_words(words: &[String]) -> &[String] {
    let mut index = 0;
    while index < words.len() {
        let word = &words[index];
        let name = command_name(word);
        if is_assignment(word) {
            index += 1;
        } else if matches!(name.as_str(), "env" | "nohup" | "time" | "nice" | "command" | "exec" | "xargs" | "call" | "start") {
            index += 1;
            while index < words.len() && words[index].starts_with('-') {
                index += 1;
            }
        } else {
            break;
        }
    }
    &words[index..]
}

fn is_assignment(word: &str) -> bool {
    match word.split_once('=') {
        Some((name, _)) => !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') && !name.starts_with(|c: char| c.is_ascii_digit()),
        None => false,
    }
}

/// The Host's own risk judgement for one segment's words.
fn segment_risk(words: &[String], findings: &mut Vec<String>) -> Risk {
    segment_risk_with_signature(words, findings).0
}

fn segment_risk_with_signature(words: &[String], findings: &mut Vec<String>) -> (Risk, Option<Vec<String>>) {
    let words = effective_words(words);
    let Some(head) = words.first() else { return (Risk::Low, None) };
    let name = command_name(head);
    let lower: Vec<String> = words.iter().map(|word| word.to_ascii_lowercase()).collect();
    let mut high = |finding: &str, signature: Vec<String>| {
        findings.push(finding.to_owned());
        (Risk::High, Some(signature))
    };
    match name.as_str() {
        "sudo" | "doas" | "su" | "runas" | "gsudo" | "pkexec" => return high("elevation", vec![head.clone()]),
        "git" => {
            let mut index = 1;
            while index < words.len() {
                let word = &lower[index];
                if word == "-c" || word == "-C" || word == "--git-dir" || word == "--work-tree" || words[index] == "-C" {
                    index += 2;
                } else if word.starts_with('-') {
                    index += 1;
                } else {
                    break;
                }
            }
            match lower.get(index).map(String::as_str) {
                Some("push") => return high("git_push", vec![head.clone(), words[index].clone()]),
                Some("clean") => return high("git_clean", vec![head.clone(), words[index].clone()]),
                Some("reset") if lower[index..].iter().any(|word| word == "--hard") => {
                    return high("git_reset_hard", vec![head.clone(), words[index].clone(), "--hard".into()])
                }
                _ => {}
            }
        }
        "rm" => {
            let recursive = lower[1..].iter().any(|word| {
                word == "--recursive" || word == "-recurse" || word.starts_with("-rec")
                    || (word.starts_with('-') && !word.starts_with("--") && word[1..].chars().any(|c| c == 'r'))
            }) || words[1..].iter().any(|word| word.starts_with('-') && !word.starts_with("--") && word[1..].contains('R'));
            if recursive {
                let flag = words[1..].iter().find(|word| word.starts_with('-')).cloned().unwrap_or_default();
                return high("recursive_delete", vec![head.clone(), flag]);
            }
        }
        "del" | "erase" | "rd" | "rmdir" => {
            // `/s` 与 MSYS 双斜杠 `//s` 都是递归删除。
            if lower[1..].iter().any(|word| is_recursive_windows_switch(word)) {
                return high("recursive_delete", vec![head.clone(), "/s".into()]);
            }
        }
        "remove-item" | "ri" => {
            if let Some(flag) = words[1..].iter().find(|word| {
                let word = word.to_ascii_lowercase();
                word.starts_with("-r") && "-recurse".starts_with(&word)
            }) {
                return high("recursive_delete", vec![head.clone(), flag.clone()]);
            }
        }
        "rimraf" => return high("recursive_delete", vec![head.clone()]),
        "iex" | "invoke-expression" => return high("remote_script", vec![head.clone()]),
        "npm" | "pnpm" | "yarn" | "bun" | "cnpm" => {
            if lower[1..].iter().any(|word| word == "-g" || word == "--global" || word == "--location=global")
                || lower.get(1).map(String::as_str) == Some("global")
            {
                return high("global_install", vec![head.clone()]);
            }
        }
        "cargo" | "go" | "gem" | "pipx" | "brew" | "choco" | "winget" | "scoop" | "apt" | "apt-get" | "yum" | "dnf" | "pacman" | "snap" | "port" => {
            if matches!(lower.get(1).map(String::as_str), Some("install")) || (name == "pacman" && words.get(1).is_some_and(|word| word.starts_with("-S"))) {
                return high("global_install", vec![head.clone(), words[1].clone()]);
            }
        }
        "pip" | "pip3" => {
            if lower.get(1).map(String::as_str) == Some("install")
                && lower[2..].iter().any(|word| word == "--user" || word == "--system" || word == "--break-system-packages")
            {
                return high("global_install", vec![head.clone(), words[1].clone()]);
            }
        }
        _ => {}
    }
    (Risk::Low, None)
}

/// cmd 内建 / 常见 Windows 程序（按 [`command_name`] 比较）。
fn is_windows_native_command(name: &str) -> bool {
    WINDOWS_NATIVE_COMMANDS.contains(&name)
}

/// `/x`、`/xy`、`/x:value`，以及 MSYS 双斜杠 `//x`。
/// 开关名由 1~3 个字母、数字或 `?` 组成，可带 `:值`。
fn is_windows_switch(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    let body = if let Some(rest) = lower.strip_prefix("//") {
        rest
    } else if let Some(rest) = lower.strip_prefix('/') {
        rest
    } else {
        return false;
    };
    if body.is_empty() || body.starts_with('/') {
        return false;
    }
    let flag = body.split_once(':').map(|(flag, _)| flag).unwrap_or(body);
    (1..=3).contains(&flag.len()) && flag.chars().all(|c| c.is_ascii_alphanumeric() || c == '?')
}

/// `del /s` 与 `rd //s` 都算递归删除。
fn is_recursive_windows_switch(word: &str) -> bool {
    let lower = word.to_ascii_lowercase();
    lower.starts_with('/') && lower.trim_start_matches('/') == "s"
}

/// The arguments a delete command removes (options and their values skipped).
fn delete_targets(words: &[String]) -> Vec<String> {
    let words = effective_words(words);
    let Some(head) = words.first() else { return Vec::new() };
    let name = command_name(head);
    if !DELETE_COMMANDS.contains(&name.as_str()) {
        return Vec::new();
    }
    // 开关只对 del/erase/rd/rmdir 跳过（含 //x）。rm、remove-item、ri、rimraf 的 /x 是目标。
    let windows_delete = matches!(name.as_str(), "del" | "erase" | "rd" | "rmdir");
    let mut targets = Vec::new();
    let mut index = 1;
    while index < words.len() {
        let word = &words[index];
        let lower = word.to_ascii_lowercase();
        if lower == "-path" || lower == "-literalpath" {
            if let Some(value) = words.get(index + 1) {
                targets.extend(value.split(',').map(|part| part.trim().to_owned()));
            }
            index += 2;
            continue;
        }
        if lower == "--" {
            targets.extend(words[index + 1..].iter().cloned());
            break;
        }
        if lower.starts_with('-') || (windows_delete && is_windows_switch(word)) {
            index += 1;
            continue;
        }
        targets.push(word.clone());
        index += 1;
    }
    targets
}

/// `cd` / `chdir` / `pushd` / `set-location` / `sl` 的下一工作目录。
/// `None` 表示这段不是切目录。无法解析（`-`、未知变量）时用文件系统根 `/`。
fn directory_change(words: &[String], cwd: &Path, project_root: Option<&Path>, home: Option<&Path>) -> Option<PathBuf> {
    let words = effective_words(words);
    let name = words.first().map(|word| command_name(word)).unwrap_or_default();
    if !matches!(name.as_str(), "cd" | "chdir" | "pushd" | "set-location" | "sl") {
        return None;
    }
    let fallback = home.map(normalize).unwrap_or_else(|| PathBuf::from("/"));
    let mut index = 1;
    while index < words.len() {
        let word = &words[index];
        let lower = word.to_ascii_lowercase();
        if lower == "-path" || lower == "-literalpath" {
            return Some(match words.get(index + 1) {
                Some(value) => cwd_after_cd(value, cwd, project_root, home),
                None => fallback,
            });
        }
        // `-` 是上一个目录，Host 看不到，后续按未知目录处理。
        if word == "-" {
            return Some(PathBuf::from("/"));
        }
        if is_windows_switch(word) || lower.starts_with('-') {
            index += 1;
            continue;
        }
        return Some(cwd_after_cd(word, cwd, project_root, home));
    }
    Some(fallback)
}

/// `cd -` 和无法解析的变量都把后续 cwd 定为文件系统根。
fn cwd_after_cd(target: &str, cwd: &Path, project_root: Option<&Path>, home: Option<&Path>) -> PathBuf {
    if target == "-" {
        PathBuf::from("/")
    } else {
        resolve_word(target, cwd, home, project_root)
    }
}

fn analyze_command(command: &str, cwd: &Path, project_root: Option<&Path>, home: Option<&Path>) -> CommandAnalysis {
    let mut analysis = CommandAnalysis { segments: Vec::new(), risk: Risk::Low, findings: Vec::new(), block: None, path_boundary: false, opaque: false };
    let escapes = !windows_syntax();
    analyze_into(&mut analysis, command, cwd, project_root, home, 0, escapes);
    // 默认方言没命中 block 时，用另一种方言只补 block（路径边界和风险仍按默认方言）。
    if analysis.block.is_none() {
        let mut alternate = CommandAnalysis { segments: Vec::new(), risk: Risk::Low, findings: Vec::new(), block: None, path_boundary: false, opaque: false };
        analyze_into(&mut alternate, command, cwd, project_root, home, 0, !escapes);
        if alternate.block.is_some() {
            analysis.block = alternate.block;
        }
    }
    analysis
}

fn analyze_into(analysis: &mut CommandAnalysis, command: &str, cwd: &Path, project_root: Option<&Path>, home: Option<&Path>, depth: usize, escapes: bool) {
    let mut segments = tokenize_with(command, escapes);
    let mut cwd_now = cwd.to_path_buf();
    let mut previous_head: Option<String> = None;
    for segment in segments.iter_mut() {
        let mut findings = Vec::new();
        let (mut risk, signature) = segment_risk_with_signature(&segment.words, &mut findings);
        segment.high_signature = signature;
        let words = effective_words(&segment.words).to_vec();
        let head = words.first().map(|word| command_name(word)).unwrap_or_default();
        if segment.piped {
            let from_fetcher = previous_head.as_deref().is_some_and(|prev| FETCHERS.contains(&prev));
            if from_fetcher && (SHELLS.contains(&head.as_str()) || INTERPRETERS.contains(&head.as_str())) {
                risk = Risk::High;
                findings.push("remote_script".into());
                segment.high_signature = None;
            }
        }
        if segment.substitution {
            risk = Risk::High;
            findings.push("command_substitution".into());
            analysis.opaque = true;
        }
        // Built-in block: deleting the project root, a filesystem/drive root,
        // the home directory (or anything above them), or a system directory.
        if analysis.block.is_none() {
            for target in delete_targets(&segment.words) {
                if let Some(reason) = dangerous_delete_target(&target, &cwd_now, project_root, home) {
                    analysis.block = Some(reason);
                    break;
                }
            }
        }
        // Paths: every argument and redirect target that names a path.
        // Windows 原生命令的 /x、//x 是开关，不参与 path_boundary。
        let windows_native = is_windows_native_command(&head);
        let path_words = words.iter().skip(1).chain(segment.redirects.iter());
        for word in path_words {
            if windows_native && is_windows_switch(word) {
                continue;
            }
            let candidate = match word.split_once('=') {
                Some((name, value)) if name.starts_with('-') => value,
                _ => word.as_str(),
            };
            if !looks_like_path(candidate) {
                continue;
            }
            let resolved = resolve_word(candidate, &cwd_now, home, project_root);
            if let Some(finding) = path_boundary(&resolved, project_root, home) {
                analysis.path_boundary = true;
                if !findings.iter().any(|known| known == finding) {
                    findings.push(finding.to_owned());
                }
                risk = Risk::High;
            }
        }
        // `bash -c "..."`、`cmd /c`、`cmd //c`、`powershell -Command`：看进去。
        // `-EncodedCommand` 看不到内容，只标 opaque 且高风险。
        if SHELLS.contains(&head.as_str()) && depth < 2 {
            match nested_script(&head, &words) {
                Some(NestedScript::Opaque) => {
                    analysis.opaque = true;
                    risk = Risk::High;
                    if !findings.iter().any(|known| known == "encoded_command") {
                        findings.push("encoded_command".into());
                    }
                }
                Some(NestedScript::Script(script)) => {
                    analysis.opaque = true;
                    let mut nested = CommandAnalysis { segments: Vec::new(), risk: Risk::Low, findings: Vec::new(), block: None, path_boundary: false, opaque: false };
                    analyze_into(&mut nested, &script, &cwd_now, project_root, home, depth + 1, escapes);
                    if nested.block.is_some() && analysis.block.is_none() {
                        analysis.block = nested.block;
                    }
                    analysis.path_boundary |= nested.path_boundary;
                    if nested.risk == Risk::High {
                        risk = Risk::High;
                    }
                    for finding in nested.findings {
                        if !findings.contains(&finding) {
                            findings.push(finding);
                        }
                    }
                }
                None => {}
            }
        }
        segment.risk = risk;
        segment.findings = findings.clone();
        if risk == Risk::High {
            analysis.risk = Risk::High;
        }
        for finding in findings {
            if !analysis.findings.contains(&finding) {
                analysis.findings.push(finding);
            }
        }
        if let Some(next) = directory_change(&words, &cwd_now, project_root, home) {
            cwd_now = next;
        }
        previous_head = Some(head);
    }
    analysis.segments.extend(segments);
}

enum NestedScript {
    /// 看得见的脚本，继续分析。
    Script(String),
    /// `-EncodedCommand` / `-enc` / `-e`：内容不可见。
    Opaque,
}

fn nested_script(head: &str, words: &[String]) -> Option<NestedScript> {
    let powershell = matches!(head, "powershell" | "pwsh");
    let mut index = 1;
    while index < words.len() {
        let lower = words[index].to_ascii_lowercase();
        // MSYS 把 `/c` 写成 `//c`，归一成单斜杠再认。
        let normalized = if let Some(rest) = lower.strip_prefix("//") {
            format!("/{rest}")
        } else {
            lower.clone()
        };
        if powershell && matches!(normalized.as_str(), "-encodedcommand" | "-enc" | "-e") {
            return Some(NestedScript::Opaque);
        }
        if matches!(normalized.as_str(), "-c" | "-lc" | "-ic" | "/c" | "/k" | "-command" | "-commandwithargs") {
            let rest = words[index + 1..].join(" ");
            return (!rest.trim().is_empty()).then_some(NestedScript::Script(rest));
        }
        // `cmd /d /s /c` 以及 `//d`、`//s`、`-NoProfile` 这类前置开关。
        if lower.starts_with('-') || lower.starts_with('/') {
            index += 1;
            continue;
        }
        return None;
    }
    None
}

fn looks_like_path(word: &str) -> bool {
    if word.is_empty() || word.contains("://") {
        return false;
    }
    let lower = word.to_ascii_lowercase();
    if matches!(lower.as_str(), "/dev/null" | "/dev/stdout" | "/dev/stderr" | "nul" | "con") {
        return false;
    }
    word.contains('/') || word.contains('\\') || word.starts_with('~') || word == ".." || word == "."
        || is_drive_path(word) || word.starts_with('%') || word.starts_with('$')
}

fn is_drive_path(word: &str) -> bool {
    let bytes = word.as_bytes();
    bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':'
}

fn is_absolute_word(word: &str) -> bool {
    word.starts_with('/') || word.starts_with('\\') || is_drive_path(word)
}

/// Expand `~`, `$HOME`, `%USERPROFILE%`, `$env:USERPROFILE` and resolve the
/// word lexically against `cwd`.
/// 当前目录或项目根是盘符路径时，把 MSYS / Cygwin / WSL 盘符路径映射成 `X:/...`。
fn resolve_word(word: &str, cwd: &Path, home: Option<&Path>, project_root: Option<&Path>) -> PathBuf {
    let home_text = home.map(|home| home.to_string_lossy().into_owned());
    let mut text = word.to_owned();
    for variable in ["${HOME}", "$HOME", "%USERPROFILE%", "%HOMEPATH%", "$env:USERPROFILE", "$env:HOME", "%HOME%"] {
        if let Some(rest) = strip_prefix_ignore_case(&text, variable) {
            text = format!("{}{}", home_text.clone().unwrap_or_else(|| "/".into()), rest);
            break;
        }
    }
    if text == "~" || text.starts_with("~/") || text.starts_with("~\\") {
        text = format!("{}{}", home_text.clone().unwrap_or_else(|| "/".into()), &text[1..]);
    }
    if text.starts_with('$') || (text.starts_with('%') && text[1..].contains('%')) {
        // An unknown variable: the Host cannot tell where it points.
        return PathBuf::from("/");
    }
    if drive_context(cwd, project_root) {
        if let Some(mapped) = map_msys_drive(&text) {
            text = mapped;
        }
    }
    if is_absolute_word(&text) {
        normalize(Path::new(&text))
    } else {
        normalize(&cwd.join(text))
    }
}

/// cwd 或项目根是 Windows 盘符路径（含 `\\?\C:\...`）。
fn drive_context(cwd: &Path, project_root: Option<&Path>) -> bool {
    path_is_drive(cwd) || project_root.is_some_and(path_is_drive)
}

fn path_is_drive(path: &Path) -> bool {
    let text = path.to_string_lossy().replace('\\', "/");
    if is_drive_path(&text) {
        return true;
    }
    text.to_ascii_lowercase().strip_prefix("//?/").is_some_and(is_drive_path)
}

/// `/x`、`/x/...`、`/cygdrive/x/...`、`/mnt/x/...` → `X:/...`。`x` 必须是单个字母。
fn map_msys_drive(text: &str) -> Option<String> {
    let slash = text.replace('\\', "/");
    let lower = slash.to_ascii_lowercase();
    for prefix in ["/cygdrive/", "/mnt/"] {
        if let Some(rest) = lower.strip_prefix(prefix) {
            return drive_from_rest(&slash[slash.len() - rest.len()..]);
        }
    }
    let body = slash.strip_prefix('/')?;
    if body.starts_with('/') {
        return None;
    }
    drive_from_rest(body).filter(|_mapped| {
        // `/x` 或 `/x/...`：第二个字符必须是结尾或 `/`，避免把 `/usr` 当成盘符。
        let after_letter = body.chars().nth(1);
        after_letter.is_none() || after_letter == Some('/')
    })
}

fn drive_from_rest(rest: &str) -> Option<String> {
    let mut chars = rest.chars();
    let letter = chars.next()?;
    if !letter.is_ascii_alphabetic() {
        return None;
    }
    let after = chars.as_str();
    if after.is_empty() {
        return Some(format!("{}:/", letter.to_ascii_uppercase()));
    }
    if !after.starts_with('/') {
        return None;
    }
    Some(format!("{}:{}", letter.to_ascii_uppercase(), after))
}

fn strip_prefix_ignore_case<'a>(text: &'a str, prefix: &str) -> Option<&'a str> {
    (text.len() >= prefix.len() && text.is_char_boundary(prefix.len()) && text[..prefix.len()].eq_ignore_ascii_case(prefix)).then(|| &text[prefix.len()..])
}

/// Lexical normalization over both separators (`.` and `..` folded).
fn normalize(path: &Path) -> PathBuf {
    let text = path.to_string_lossy().replace('\\', "/");
    // Host-canonical Windows paths are verbatim (`\\?\C:\...`,
    // `\\?\UNC\server\share`); compare them as the plain path they name.
    let text = if let Some(rest) = text.strip_prefix("//?/UNC/") {
        format!("//{rest}")
    } else if let Some(rest) = text.strip_prefix("//?/").filter(|rest| is_drive_path(rest)) {
        rest.to_owned()
    } else {
        text
    };
    let (prefix, rest) = if is_drive_path(&text) {
        (format!("{}/", &text[..2].to_ascii_uppercase()), text[2..].to_owned())
    } else if text.starts_with('/') {
        ("/".to_owned(), text.clone())
    } else {
        (String::new(), text.clone())
    };
    let mut parts: Vec<&str> = Vec::new();
    for part in rest.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                parts.pop();
            }
            other => parts.push(other),
        }
    }
    PathBuf::from(format!("{prefix}{}", parts.join("/")))
}

fn components(path: &Path) -> Vec<String> {
    let raw = path.to_string_lossy().replace('\\', "/");
    let text = normalize(path).to_string_lossy().into_owned();
    // 盘符路径和 UNC 永远大小写不敏感（Linux CI 上的 Windows 路径测试也成立）。
    // 普通 POSIX 路径仍只在 Windows 主机上折叠大小写。
    let casefold = is_drive_path(&text) || is_unc_raw(&raw) || windows_syntax();
    let mut parts: Vec<String> = Vec::new();
    if is_drive_path(&text) {
        parts.push(text[..2].to_ascii_uppercase());
    } else if text.starts_with('/') {
        parts.push("/".into());
    }
    let body = if is_drive_path(&text) { &text[2..] } else { text.as_str() };
    parts.extend(body.split('/').filter(|part| !part.is_empty()).map(|part| {
        if casefold { part.to_lowercase() } else { part.to_owned() }
    }));
    parts
}

/// `\\server\share` 或 `\\?\UNC\server\share`。
fn is_unc_raw(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    if lower.starts_with("//?/unc/") {
        return true;
    }
    text.starts_with("//") && !lower.starts_with("//?/")
}

/// `inner` equals `outer` or sits below it.
fn is_within(inner: &Path, outer: &Path) -> bool {
    let inner = components(inner);
    let outer = components(outer);
    inner.len() >= outer.len() && inner[..outer.len()] == outer[..]
}

fn is_filesystem_root(path: &Path) -> bool {
    components(path).len() <= 1
}

const SHELL_PROFILES: [&str; 10] = [
    ".bashrc", ".bash_profile", ".bash_login", ".profile", ".zshrc", ".zprofile", ".zshenv",
    "microsoft.powershell_profile.ps1", "profile.ps1", "config.fish",
];

/// `Some(finding)` when the path is outside the project or protected.
fn path_boundary(path: &Path, project_root: Option<&Path>, _home: Option<&Path>) -> Option<&'static str> {
    let normalized = normalize(path);
    match project_root {
        Some(root) if !is_within(&normalized, root) => return Some("outside_project"),
        None => return Some("outside_project"),
        _ => {}
    }
    let parts = components(&normalized);
    let root_len = project_root.map(|root| components(root).len()).unwrap_or(0);
    let inside = &parts[root_len.min(parts.len())..];
    if inside.iter().any(|part| part.eq_ignore_ascii_case(".git")) {
        return Some("protected_git");
    }
    if inside.iter().any(|part| part.eq_ignore_ascii_case(".fox")) {
        return Some("protected_fox_config");
    }
    if inside.last().is_some_and(|name| SHELL_PROFILES.iter().any(|profile| name.eq_ignore_ascii_case(profile))) {
        return Some("protected_shell_config");
    }
    None
}

/// 本身及其内部都禁止删除的 POSIX 目录。
const POSIX_SYSTEM_TREE: [&str; 13] = ["bin", "boot", "dev", "etc", "lib", "lib32", "lib64", "libx32", "proc", "sbin", "sys", "usr", "System"];
/// 只禁止删除目录本身（其上级已被根目录规则覆盖）。
const POSIX_SYSTEM_SELF: [&str; 12] = ["var", "opt", "home", "Users", "root", "Applications", "Library", "private", "srv", "mnt", "media", "snap"];

/// Built-in block for one delete target.
fn dangerous_delete_target(target: &str, cwd: &Path, project_root: Option<&Path>, home: Option<&Path>) -> Option<String> {
    // `*`, `./*`, `~/*`, `/*`: a glob that empties the directory it names.
    let trimmed = target.trim_end_matches(['/', '\\']);
    let (dir_part, glob) = match trimmed.strip_suffix('*') {
        Some(prefix) if prefix.is_empty() || prefix.ends_with('/') || prefix.ends_with('\\') => (if prefix.is_empty() { "." } else { prefix }, true),
        _ => (trimmed, false),
    };
    let dir_part = if dir_part.is_empty() { "/" } else { dir_part };
    let resolved = resolve_word(dir_part, cwd, home, project_root);
    let what = if glob { "清空" } else { "删除" };
    if is_filesystem_root(&resolved) {
        return Some(format!("禁止{what}盘符或文件系统根目录"));
    }
    if let Some(home) = home {
        if is_within(home, &resolved) {
            return Some(format!("禁止{what}家目录"));
        }
    }
    if let Some(root) = project_root {
        if is_within(root, &resolved) {
            return Some(format!("禁止{what}项目根目录"));
        }
    }
    if is_system_directory(&resolved) {
        return Some("禁止删除系统目录".into());
    }
    None
}

/// Windows 系统目录（盘符路径）和 POSIX 系统目录。任何模式都拦。
fn is_system_directory(path: &Path) -> bool {
    let parts = components(path);
    if parts.first().is_some_and(|part| is_drive_path(part)) {
        return windows_system_directory(&parts);
    }
    if parts.first().map(String::as_str) == Some("/") {
        return posix_system_directory(&parts);
    }
    false
}

fn windows_system_directory(parts: &[String]) -> bool {
    let rest = &parts[1..];
    let Some(first) = rest.first() else { return false };
    if first.eq_ignore_ascii_case("windows") || first.eq_ignore_ascii_case("programdata") || first.eq_ignore_ascii_case("program files") || first.eq_ignore_ascii_case("program files (x86)") {
        return true;
    }
    // `X:/Users` 本身，以及 `X:/Users/<用户>` 这一层（用户目录本身，不含其内部）。
    first.eq_ignore_ascii_case("users") && rest.len() <= 2
}

fn posix_system_directory(parts: &[String]) -> bool {
    let rest = &parts[1..];
    let Some(first) = rest.first() else { return false };
    if POSIX_SYSTEM_TREE.iter().any(|name| first.eq_ignore_ascii_case(name)) {
        return true;
    }
    POSIX_SYSTEM_SELF.iter().any(|name| first.eq_ignore_ascii_case(name)) && rest.len() == 1
}

/// The current user's home directory, as the Host sees it.
pub fn host_home() -> Option<PathBuf> {
    let variable = if cfg!(windows) { "USERPROFILE" } else { "HOME" };
    std::env::var_os(variable).filter(|value| !value.is_empty()).map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a1_verbatim_host_paths_compare_as_the_plain_path() {
        assert!(is_within(Path::new(r"\\?\C:\work\proj\note.txt"), Path::new(r"C:\work\proj")));
        assert_eq!(path_boundary(Path::new(r"\\?\C:\work\proj\note.txt"), Some(Path::new("C:/work/proj")), None), None);
        assert_eq!(normalize(Path::new(r"\\?\C:\Work\a\..\b")), PathBuf::from("C:/Work/b"));
        // UNC targets compare with a plain `\\srv\share` root the same way.
        assert!(is_within(Path::new(r"\\?\UNC\srv\share\x"), Path::new(r"\\srv\share")));
    }

    fn root() -> PathBuf {
        if cfg!(windows) { PathBuf::from("C:/work/proj") } else { PathBuf::from("/work/proj") }
    }
    fn home() -> PathBuf {
        if cfg!(windows) { PathBuf::from("C:/Users/fox") } else { PathBuf::from("/home/fox") }
    }
    fn rule(id: &str, effect: RuleEffect, tool: &str, pattern: &str) -> PolicyRule {
        PolicyRule { id: id.into(), effect, tool: tool.into(), pattern: pattern.into() }
    }
    fn command(tool: &str, command: &str, mode: AccessMode, rules: &[PolicyRule]) -> PolicyDecision {
        command_risk(tool, command, None, mode, rules)
    }
    fn command_risk(tool: &str, command: &str, risk: Option<&str>, mode: AccessMode, rules: &[PolicyRule]) -> PolicyDecision {
        let root = root();
        let home = home();
        let subject = ToolSubject::Command { command: command.into(), cwd: root.clone(), model_risk: risk.map(str::to_owned) };
        decide(&PolicyRequest { tool, subject: &subject, mode, project_root: Some(&root), home: Some(&home), rules })
    }
    fn subject(subject: ToolSubject, mode: AccessMode) -> PolicyDecision {
        let root = root();
        let home = home();
        decide(&PolicyRequest { tool: "write_file", subject: &subject, mode, project_root: Some(&root), home: Some(&home), rules: &[] })
    }
    fn verdict(decision: &PolicyDecision) -> &'static str {
        match decision.verdict { Verdict::Allow => "allow", Verdict::Ask => "ask", Verdict::Deny { .. } => "deny" }
    }
    const MODES: [AccessMode; 4] = [AccessMode::ReadOnly, AccessMode::Ask, AccessMode::Auto, AccessMode::FullAccess];

    #[test]
    fn a1_policy_four_modes_by_six_tool_classes() {
        let write = || ToolSubject::Write { target: root().join("src/main.rs") };
        let outside = || ToolSubject::Write { target: root().join("../other/x.txt") };
        let protected = || ToolSubject::Write { target: root().join(".git/config") };
        let table: [(&str, [&str; 4]); 7] = [
            ("read", ["allow", "allow", "allow", "allow"]),
            ("write", ["deny", "ask", "allow", "allow"]),
            ("command_low", ["deny", "ask", "allow", "allow"]),
            ("command_high", ["deny", "ask", "ask", "allow"]),
            ("network", ["ask", "ask", "allow", "allow"]),
            ("boundary", ["deny", "ask", "ask", "allow"]),
            ("block", ["deny", "deny", "deny", "deny"]),
        ];
        for (class, expected) in table {
            for (mode, want) in MODES.iter().zip(expected) {
                let got = match class {
                    "read" => subject(ToolSubject::Read, *mode),
                    "write" => subject(write(), *mode),
                    "command_low" => command("run_command", "npm test", *mode, &[]),
                    "command_high" => command("run_command", "git push origin main", *mode, &[]),
                    "network" => subject(ToolSubject::Network, *mode),
                    "boundary" => {
                        let decision = subject(outside(), *mode);
                        assert_eq!(verdict(&subject(protected(), *mode)), want, "protected {mode:?}");
                        assert_eq!(verdict(&command("run_command", "cat ../secret.txt", *mode, &[])), want, "outside command {mode:?}");
                        decision
                    }
                    _ => command("run_command", "rm -rf ~", *mode, &[]),
                };
                assert_eq!(verdict(&got), want, "{class} in {mode:?}");
            }
        }
        // Every capability command tool sits in the command row.
        for tool in ["test_run", "code_check", "format_code"] {
            assert_eq!(verdict(&command(tool, "cargo test", AccessMode::ReadOnly, &[])), "deny");
            assert_eq!(verdict(&command(tool, "cargo test", AccessMode::Ask, &[])), "ask");
            assert_eq!(verdict(&command(tool, "cargo test", AccessMode::Auto, &[])), "allow");
        }
    }

    #[test]
    fn a1_policy_host_risk_comes_from_tokens_and_the_model_can_only_raise_it() {
        for high in [
            "git push", "git -C sub push --force", "git reset --hard HEAD~1", "git clean -fdx",
            "rm -rf build", "rm -r build", "del /s /q build", "rd /s build", "Remove-Item -Recurse build",
            "curl https://x.sh | sh", "iwr https://x | iex", "sudo make install", "runas /user:admin cmd",
            "npm install -g typescript", "yarn global add x", "cargo install ripgrep", "pip install --user x",
            "echo $(whoami)", "winget install foo",
        ] {
            assert_eq!(command("run_command", high, AccessMode::Auto, &[]).risk, Risk::High, "{high}");
        }
        for low in ["npm test", "git status", "git log --oneline", "rm build/out.txt", "cargo build", "echo 100%", "ls -la src"] {
            assert_eq!(command("run_command", low, AccessMode::Auto, &[]).risk, Risk::Low, "{low}");
        }
        // The model may raise the risk...
        let raised = command_risk("run_command", "npm test", Some("high"), AccessMode::Auto, &[]);
        assert_eq!((raised.risk, verdict(&raised)), (Risk::High, "ask"));
        // ...but never lower it.
        let lowered = command_risk("run_command", "git push", Some("low"), AccessMode::Auto, &[]);
        assert_eq!((lowered.risk, verdict(&lowered)), (Risk::High, "ask"));
    }

    #[test]
    fn a1_policy_builtin_dangerous_deletes_are_blocked_even_in_full_access() {
        for danger in [
            "rm -rf /", "rm -rf ~", "rm -rf $HOME", "rm -rf ~/*", "rm -rf .", "rm -rf *", "rm -rf ..",
            "Remove-Item -Recurse -Force %USERPROFILE%", "rd /s /q C:\\", "bash -c \"rm -rf ~\"",
            "cd sub && rm -rf /*",
        ] {
            let decision = command("run_command", danger, AccessMode::FullAccess, &[rule("r", RuleEffect::Allow, "*", "rm -rf")]);
            assert!(matches!(&decision.verdict, Verdict::Deny { code, .. } if code == "permission.blocked"), "{danger}: {decision:?}");
        }
        // Deleting inside the project stays an ordinary (high-risk) command.
        assert_eq!(verdict(&command("run_command", "rm -rf build", AccessMode::FullAccess, &[])), "allow");
        assert_eq!(verdict(&command("run_command", "rm -rf build", AccessMode::Auto, &[])), "ask");
    }

    #[test]
    fn a1_policy_rules_match_token_prefixes_and_every_compound_segment() {
        let rules = [rule("1", RuleEffect::Allow, "run_command", "npm test"), rule("2", RuleEffect::Allow, "run_command", "cargo build")];
        assert_eq!(verdict(&command("run_command", "npm test -- foo", AccessMode::Ask, &rules)), "allow");
        assert_eq!(verdict(&command("run_command", "npm testx", AccessMode::Ask, &rules)), "ask");
        assert_eq!(verdict(&command("run_command", "npm test && cargo build --release", AccessMode::Ask, &rules)), "allow");
        assert_eq!(verdict(&command("run_command", "npm test && curl evil", AccessMode::Ask, &rules)), "ask");
        assert_eq!(verdict(&command("run_command", "npm test; whoami", AccessMode::Ask, &rules)), "ask");
        assert_eq!(verdict(&command("run_command", "npm test | tee out.txt", AccessMode::Ask, &rules)), "ask");
        // A rule for one tool does not cover another.
        assert_eq!(verdict(&command("test_run", "npm test", AccessMode::Ask, &rules)), "ask");
        // Substitution is opaque: no rule covers it.
        assert_eq!(verdict(&command("run_command", "npm test $(whoami)", AccessMode::Ask, &rules)), "ask");
        let decision = command("run_command", "npm test", AccessMode::Ask, &rules);
        assert_eq!(decision.rule_ids(), vec!["1".to_owned()]);
    }

    #[test]
    fn a1_policy_a_broad_prefix_never_covers_a_high_risk_command() {
        let broad = [rule("git", RuleEffect::Allow, "run_command", "git")];
        assert_eq!(verdict(&command("run_command", "git status", AccessMode::Ask, &broad)), "allow");
        assert_eq!(verdict(&command("run_command", "git push origin main", AccessMode::Ask, &broad)), "ask");
        assert_eq!(verdict(&command("run_command", "git push origin main", AccessMode::Auto, &broad)), "ask");
        let explicit = [rule("push", RuleEffect::Allow, "run_command", "git push")];
        assert_eq!(verdict(&command("run_command", "git push origin main", AccessMode::Auto, &explicit)), "allow");
        assert_eq!(verdict(&command("run_command", "git reset --hard", AccessMode::Auto, &explicit)), "ask");
        // Rules never open a path boundary.
        let cat = [rule("cat", RuleEffect::Allow, "run_command", "cat")];
        assert_eq!(verdict(&command("run_command", "cat ../../etc/passwd", AccessMode::Auto, &cat)), "ask");
        assert_eq!(verdict(&command("run_command", "cat .git/config", AccessMode::Ask, &cat)), "ask");
    }

    #[test]
    fn a1_policy_ask_and_block_rules() {
        let rules = [rule("a", RuleEffect::Ask, "run_command", "npm publish"), rule("b", RuleEffect::Block, "*", "terraform destroy")];
        assert_eq!(verdict(&command("run_command", "npm publish --tag next", AccessMode::Auto, &rules)), "ask");
        assert_eq!(verdict(&command("run_command", "npm run build", AccessMode::Auto, &rules)), "allow");
        let blocked = command("run_command", "terraform destroy -auto-approve", AccessMode::FullAccess, &rules);
        assert_eq!(verdict(&blocked), "deny");
        assert_eq!(blocked.forcing_rule.as_ref().map(|rule| rule.id.as_str()), Some("b"));
    }

    #[test]
    fn a1_policy_rewriting_package_json_does_not_widen_an_allow_rule() {
        // TX-1: the rule names the command prefix the Host runs, never a hash of
        // the model's input. A rewritten `test` script still runs as `npm test`
        // (matched, and the match is what the audit records), while a different
        // program line is a different command.
        let rules = [rule("t", RuleEffect::Allow, "test_run", "npm test")];
        let decision = command("test_run", "npm test", AccessMode::Ask, &rules);
        assert!(decision.is_allow());
        assert_eq!(decision.allow_reason.as_ref().map(AllowReason::as_str), Some("rule"));
        assert_eq!(decision.rule_ids(), vec!["t".to_owned()]);
        assert_eq!(verdict(&command("test_run", "npm run test:e2e", AccessMode::Ask, &rules)), "ask");
    }

    #[test]
    fn a1_policy_suggested_prefix_and_pattern_validation() {
        assert_eq!(command("run_command", "npm test -- --watch", AccessMode::Ask, &[]).suggested_prefix.as_deref(), Some("npm test"));
        assert_eq!(command("run_command", "git push origin main", AccessMode::Auto, &[]).suggested_prefix.as_deref(), Some("git push"));
        assert_eq!(command("run_command", "ls -la", AccessMode::Ask, &[]).suggested_prefix.as_deref(), Some("ls"));
        assert!(!command("run_command", "cat ../x", AccessMode::Ask, &[]).rule_eligible);
        assert!(parse_rule_pattern("npm test").is_ok());
        for bad in ["", "npm test && rm x", "npm test > out", "echo $(id)", "a | b"] {
            assert!(parse_rule_pattern(bad).is_err(), "{bad}");
        }
        let root = root();
        assert!(pattern_covers_command("npm test", "npm test -- a", &root, Some(&root), None));
        assert!(!pattern_covers_command("npm", "npm test && whoami", &root, Some(&root), None));
        assert!(!pattern_covers_command("git", "git push", &root, Some(&root), None));
    }

    #[test]
    fn a1_policy_access_mode_storage_round_trip() {
        assert_eq!(AccessMode::from_stored("allow", true).unwrap(), AccessMode::FullAccess);
        assert_eq!(AccessMode::from_stored("allow", false).unwrap(), AccessMode::Auto);
        assert_eq!(AccessMode::from_stored("ask", true).unwrap(), AccessMode::Ask);
        assert_eq!(AccessMode::from_stored("read_only", true).unwrap(), AccessMode::ReadOnly);
        assert!(AccessMode::from_stored("bogus", false).is_err());
        for mode in MODES {
            assert_eq!(AccessMode::parse(mode.as_str()).unwrap(), mode);
            assert_eq!(AccessMode::from_stored(mode.stored_mode(), mode.full_access()).unwrap(), mode);
        }
    }

    #[test]
    fn a1_policy_tokenizer_handles_quotes_and_redirects() {
        let segments = tokenize("echo \"a && b\" > 'out file.txt' 2>&1 && npm test");
        assert_eq!(segments.len(), 2);
        assert_eq!(segments[0].words, vec!["echo", "a && b"]);
        assert_eq!(segments[0].redirects, vec!["out file.txt"]);
        assert_eq!(segments[1].words, vec!["npm", "test"]);
        // A redirect out of the project is a path boundary.
        assert_eq!(command("run_command", "echo hi > ../escape.txt", AccessMode::Auto, &[]).class, ToolClass::PathBoundary);
        assert_eq!(verdict(&command("run_command", "echo hi > /dev/null", AccessMode::Auto, &[])), "allow");
        assert_eq!(verdict(&command("run_command", "curl https://example.com/a/b", AccessMode::Auto, &[])), "allow");
    }

    fn win_root() -> PathBuf {
        PathBuf::from("D:/fox-test/测试项目")
    }
    fn win_home() -> PathBuf {
        PathBuf::from("C:/Users/fox")
    }
    fn posix_root() -> PathBuf {
        PathBuf::from("/work/proj")
    }
    fn posix_home() -> PathBuf {
        PathBuf::from("/home/fox")
    }
    fn decide_at(project: &Path, home_dir: &Path, command: &str, mode: AccessMode) -> PolicyDecision {
        let subject = ToolSubject::Command { command: command.into(), cwd: project.to_path_buf(), model_risk: None };
        decide(&PolicyRequest { tool: "run_command", subject: &subject, mode, project_root: Some(project), home: Some(home_dir), rules: &[] })
    }
    fn assert_blocked_at(project: &Path, home_dir: &Path, command: &str) {
        for mode in MODES {
            let decision = decide_at(project, home_dir, command, mode);
            assert!(decision.is_block(), "{command} in {mode:?} should block, got {:?}", decision.verdict);
            assert_eq!(decision.class, ToolClass::Command, "{command} in {mode:?} stays Command");
            assert!(decision.findings.iter().any(|finding| finding == "builtin_block"), "{command} findings {:?}", decision.findings);
        }
    }

    /// 固定 Windows 盘符项目根，不依赖 cfg!(windows)，Linux CI 上也要拦住。
    #[test]
    fn builtin_block_stops_windows_project_root_deletes_in_every_mode() {
        let project = win_root();
        let home_dir = win_home();
        for command in [
            r"rmdir /s /q D:\fox-test\测试项目",
            r"RMDIR /S /Q d:\FOX-TEST\测试项目",
            r#"rd /s /q "D:\fox-test\测试项目\""#,
            "rd /s /q D:/fox-test/测试项目/",
            r"del /s /q D:\fox-test\测试项目",
            r"del /s /q D:\fox-test\测试项目\*",
            r"Remove-Item -Recurse -Force 'D:\fox-test\测试项目'",
            r#"Remove-Item -LiteralPath "D:\fox-test\测试项目" -Recurse"#,
            r#"powershell -Command "Remove-Item -Recurse -Force 'D:\fox-test\测试项目'""#,
            r"cmd /c rmdir /s /q D:\fox-test\测试项目",
            r#"cmd //c "rmdir /s /q D:\\fox-test\\测试项目""#,
            "cmd.exe //c rd //s //q D:/fox-test/测试项目",
            "rm -rf /d/fox-test/测试项目",
            "rm -rf /cygdrive/d/fox-test/测试项目",
            r#"rm -rf "D:/fox-test/测试项目""#,
            "rm -rf .",
            "rm -rf ./",
            "rm -rf ..",
            "rd /s /q .",
            "rd /s /q ..",
            "Remove-Item -Recurse .",
            "cd .. && rm -rf 测试项目",
            r"cd /d D:\fox-test && rd /s /q 测试项目",
            "cd - && rm -rf .",
            r"rd /s /q C:\Windows",
            r#"rd /s /q "C:\Program Files""#,
            r"Remove-Item -Recurse C:\Users\fox",
            "rm -rf /c/Windows/System32",
            "rm -rf /c",
        ] {
            assert_blocked_at(&project, &home_dir, command);
        }
        // 项目内部的删除不是 block：完全访问放行，自动档按高风险询问。
        for command in ["rd /s /q build", "rm -rf node_modules", r"del /s /q dist\*.tmp", r"Remove-Item -Recurse .\out"] {
            for (mode, want) in MODES.iter().zip(["deny", "ask", "ask", "allow"]) {
                let decision = decide_at(&project, &home_dir, command, *mode);
                assert!(!decision.is_block(), "{command} in {mode:?} must not be a block");
                assert_eq!(verdict(&decision), want, "{command} in {mode:?}");
            }
        }
        for command in ["dir", "dir /b", "dir /a /s src", "cmd /c dir", "cmd //c dir", "ls -la", "type README.md", "findstr /i foo *.txt"] {
            let decision = decide_at(&project, &home_dir, command, AccessMode::Auto);
            assert_eq!(verdict(&decision), "allow", "{command} should be a low-risk allow, class {:?} findings {:?}", decision.class, decision.findings);
            assert_eq!(decision.risk, Risk::Low, "{command}");
            assert_ne!(decision.class, ToolClass::PathBoundary, "{command}");
        }
        for command in ["ls /c", "cat /d/other/x.txt"] {
            let decision = decide_at(&project, &home_dir, command, AccessMode::Auto);
            assert_eq!(verdict(&decision), "ask", "{command}");
            assert_eq!(decision.class, ToolClass::PathBoundary, "{command}");
            assert!(!decision.is_block(), "{command}");
        }
        let encoded = decide_at(&project, &home_dir, "powershell -EncodedCommand ZQBjAGgAbwA=", AccessMode::Auto);
        assert_eq!(verdict(&encoded), "ask");
        assert_eq!(encoded.risk, Risk::High);
        assert!(encoded.findings.iter().any(|finding| finding == "encoded_command"));
        assert!(!encoded.is_block());
        // 盘符 / UNC 比较大小写不敏感。
        assert!(is_within(Path::new(r"D:\FOX-TEST\测试项目\a.txt"), Path::new(r"d:/fox-test/测试项目")));
        assert!(is_within(Path::new(r"\\SRV\Share\X"), Path::new(r"\\srv\share")));
    }

    #[test]
    fn builtin_block_stops_posix_roots_and_system_directories() {
        let project = posix_root();
        let home_dir = posix_home();
        for command in [
            "rm -rf /work/proj",
            r#"rm -rf "/work/proj/""#,
            "rm -rf /work",
            "rm -rf ~",
            "rm -rf $HOME",
            "rm -rf /usr/lib/x",
            "rm -rf /etc",
        ] {
            assert_blocked_at(&project, &home_dir, command);
        }
    }

    #[test]
    fn project_block_rules_are_marked_for_the_audit() {
        let rules = [rule("b", RuleEffect::Block, "*", "terraform destroy")];
        let decision = command("run_command", "terraform destroy -auto-approve", AccessMode::FullAccess, &rules);
        assert!(decision.is_block());
        assert!(decision.findings.iter().any(|finding| finding == "project_block_rule"));
        assert!(!decision.findings.iter().any(|finding| finding == "builtin_block"));
        assert_eq!(decision.class, ToolClass::Command);
        assert_eq!(decision.forcing_rule.as_ref().map(|rule| rule.id.as_str()), Some("b"));
    }
}
