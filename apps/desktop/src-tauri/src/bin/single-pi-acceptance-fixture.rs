//! Single-Pi mainline acceptance fixture (2026-10-27), work package D2/D3.
//!
//! The acceptance run needs two experts with separate knowledge bases inside an
//! isolated data directory, created through Fox's real APIs. The desktop UI
//! buttons that create a knowledge base / expert are unreliable to drive from a
//! test, so this binary does it as a program:
//!
//! * `seed` finds-or-creates the two knowledge bases and their documents through
//!   `LocalKnowledgeStore::create_base` / `start_import`, the two experts through
//!   `Database::save_agent`, and one conversation per expert through
//!   `Database::create_conversation_with_backend` + `bind_conversation_expert`
//!   + `set_knowledge_reference_bindings`. Nothing is written with SQL.
//! * `dump` prints one JSON object that proves what exists: knowledge bases with
//!   their documents, experts with their bound bases, conversations with
//!   `agent_backend`, the `runtime_sessions` rows, and the result of the same
//!   retrieval calls the Host's `search_knowledge` tool makes.
//!
//! Every run is gated on an isolated `FOX_DATA_DIR` that either is empty or
//! carries the acceptance marker; a directory that already holds a Fox database
//! without a marker is refused, so the operator's real `%APPDATA%\com.fox.agent`
//! can never be seeded. The binary touches no credentials and prints no key.
//!
//! stdout carries exactly one JSON object; human-readable progress goes to stderr.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use serde_json::{json, Value};

use fox_desktop_lib::acceptance_api::{
    AgentRecord, ConversationExpertBinding, ConversationSummary, CreateLocalKnowledgeBaseRequest,
    Database, KnowledgeReference, KnowledgeReferenceBindingInput, KnowledgeReferenceBindingRecord,
    LocalKnowledgeBase, LocalKnowledgeDocumentsRequest, LocalKnowledgeImportFileRequest,
    LocalKnowledgeImportRequest, LocalKnowledgeStore, RuntimeSessionRecord, SaveAgentRequest,
    LOCAL_KNOWLEDGE_STORAGE_PATH_KEY,
};

/// Written into the isolated directory before anything else is created. A
/// directory that already holds a Fox database without it is refused.
const MARKER_FILE: &str = ".fox-single-pi-acceptance";
/// The Kernel acceptance launcher's marker: a directory it prepared has already
/// been proven not to be real user data.
const KERNEL_MARKER_FILE: &str = ".fox-kernel-acceptance";
/// Directory names of the operator's real data directory, never seeded.
const REAL_DATA_DIR_NAMES: [&str; 2] = ["com.fox.agent", "fox"];
/// Optional override of the fixture directory (defaults to the checked-in one).
const FIXTURE_DIR_ENV: &str = "FOX_SINGLE_PI_FIXTURE_DIR";
/// The values the expert editor in the UI writes by default.
const EXPERT_CATEGORY: &str = "general";
const EXPERT_ICON: &str = "/mascot/fox_magic.png";
/// `conversation_expert_bindings.activation_source` for an expert opened from the
/// expert centre, which is the flow the acceptance run drives.
const ACTIVATION_SOURCE: &str = "expert_center";
/// A local import runs on a worker thread; wait for its job rows to finish.
const IMPORT_TIMEOUT: Duration = Duration::from_secs(180);

fn main() {
    let mode = std::env::args().nth(1).unwrap_or_default();
    let outcome = match mode.as_str() {
        "seed" => seed(),
        "dump" => dump(),
        "" => Err(
            "usage: single-pi-acceptance-fixture <seed|dump> (FOX_DATA_DIR must name an isolated acceptance directory)"
                .to_owned(),
        ),
        other => Err(format!("unknown mode '{other}'; expected 'seed' or 'dump'")),
    };
    match outcome {
        Ok(value) => match serde_json::to_string_pretty(&value) {
            Ok(text) => println!("{text}"),
            Err(error) => {
                eprintln!("SINGLE_PI_FIXTURE_FAILED result is not serializable: {error}");
                std::process::exit(2);
            }
        },
        Err(message) => {
            eprintln!("SINGLE_PI_FIXTURE_FAILED {message}");
            std::process::exit(2);
        }
    }
}
// ---------------------------------------------------------------------------
// Isolation guard
// ---------------------------------------------------------------------------

/// The isolated acceptance directory, proven to be isolated.
struct AcceptanceDir {
    data_dir: PathBuf,
    database_path: PathBuf,
}

fn resolve_acceptance_dir() -> Result<AcceptanceDir, String> {
    let raw = std::env::var_os("FOX_DATA_DIR")
        .filter(|value| !value.is_empty())
        .ok_or_else(|| {
            "FOX_DATA_DIR is not set: this fixture only runs against an isolated acceptance data directory"
                .to_owned()
        })?;
    let data_dir = PathBuf::from(raw);
    if !data_dir.is_absolute() {
        return Err(format!(
            "FOX_DATA_DIR must be an absolute path, got '{}'",
            data_dir.display()
        ));
    }

    // Belt and braces: the real per-user data directory is refused by name, so a
    // mistyped path can never reach the operator's own database.
    if let Some(app_data) = std::env::var_os("APPDATA").filter(|value| !value.is_empty()) {
        for name in REAL_DATA_DIR_NAMES {
            let real = PathBuf::from(&app_data).join(name);
            if same_path(&data_dir, &real) {
                return Err(format!(
                    "refusing the real Fox data directory {}: FOX_DATA_DIR must name an isolated acceptance directory",
                    real.display()
                ));
            }
        }
    }

    // A directory that already holds a Fox database must carry an acceptance
    // marker, otherwise it may be a real user database.
    let database_path = data_dir.join("fox.db");
    let marker = data_dir.join(MARKER_FILE);
    if database_path.exists() && !marker.is_file() && !data_dir.join(KERNEL_MARKER_FILE).is_file() {
        return Err(format!(
            "refusing '{}': it already contains a Fox database ({}) but no acceptance marker ({}); point FOX_DATA_DIR at an empty directory or at one this fixture created",
            data_dir.display(),
            database_path.display(),
            MARKER_FILE
        ));
    }

    std::fs::create_dir_all(&data_dir).map_err(|error| {
        format!(
            "cannot create the acceptance data directory {}: {error}",
            data_dir.display()
        )
    })?;
    std::fs::write(
        &marker,
        "Single-Pi acceptance fixture (2026-10-27).\n\
         This directory holds acceptance data only; it is not a real Fox data directory.\n",
    )
    .map_err(|error| {
        format!(
            "cannot write the acceptance marker {}: {error}",
            marker.display()
        )
    })?;

    Ok(AcceptanceDir {
        data_dir,
        database_path,
    })
}

/// Drop a Windows `\\?\` verbatim prefix (incl. `\\?\UNC\`), which the local
/// knowledge importer refuses as an unsafe source path.
fn strip_verbatim(path: PathBuf) -> PathBuf {
    let text = path.to_string_lossy();
    if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{rest}"));
    }
    match text.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => path,
    }
}
/// Case-insensitive on Windows, exact elsewhere, tolerant of a trailing separator.
fn same_path(left: &Path, right: &Path) -> bool {
    let fold = |path: &Path| {
        path.to_string_lossy()
            .trim_end_matches(|character| character == '\\' || character == '/')
            .to_lowercase()
    };
    let left = fold(left);
    let right = fold(right);
    !left.is_empty() && left == right
}

/// Where the app itself opens the local knowledge store: the configured storage
/// path when one is set, otherwise `<data>/local-knowledge`.
fn resolve_knowledge_root(data_dir: &Path, database: &Database) -> Result<PathBuf, String> {
    match database
        .app_setting(LOCAL_KNOWLEDGE_STORAGE_PATH_KEY)
        .map_err(|error| format!("cannot read {LOCAL_KNOWLEDGE_STORAGE_PATH_KEY}: {error}"))?
    {
        Some(configured) if !configured.trim().is_empty() => {
            let configured = PathBuf::from(configured.trim());
            if !configured.is_absolute() {
                return Err(format!(
                    "the configured local knowledge path '{}' is not absolute",
                    configured.display()
                ));
            }
            Ok(configured)
        }
        _ => Ok(data_dir.join("local-knowledge")),
    }
}
// ---------------------------------------------------------------------------
// Fixture description
// ---------------------------------------------------------------------------

struct Fixture {
    root: PathBuf,
    json: Value,
}

impl Fixture {
    fn load() -> Result<Self, String> {
        let root = match std::env::var_os(FIXTURE_DIR_ENV).filter(|value| !value.is_empty()) {
            Some(value) => PathBuf::from(value),
            None => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                // apps/desktop/src-tauri -> apps/desktop/tests/acceptance/...
                .join("..")
                .join("tests")
                .join("acceptance")
                .join("single-pi-20261027"),
        };
        let root = std::fs::canonicalize(&root).map_err(|error| {
            format!(
                "cannot resolve the acceptance fixture directory {}: {error} (set {FIXTURE_DIR_ENV})",
                root.display()
            )
        })?;
        // `canonicalize` yields a `\\?\` verbatim path on Windows, which the
        // knowledge importer refuses; only the plain form is accepted.
        let root = strip_verbatim(root);
        let path = root.join("fixture.json");
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("cannot read {}: {error}", path.display()))?;
        let json: Value = serde_json::from_str(&text)
            .map_err(|error| format!("cannot parse {}: {error}", path.display()))?;
        Ok(Self { root, json })
    }

    fn path(&self) -> String {
        self.root.join("fixture.json").display().to_string()
    }

    fn name(&self) -> String {
        self.json
            .get("name")
            .and_then(Value::as_str)
            .unwrap_or("single-pi-acceptance")
            .to_owned()
    }

    fn summary(&self) -> Value {
        json!({
            "name": self.name(),
            "schemaVersion": self.json.get("schemaVersion").cloned().unwrap_or(Value::Null),
            "source": self.path(),
            "root": self.root.display().to_string(),
        })
    }

    fn experts(&self) -> Result<&Vec<Value>, String> {
        self.json
            .get("experts")
            .and_then(Value::as_array)
            .ok_or_else(|| format!("{} has no experts[]", self.path()))
    }

    fn questions(&self) -> Vec<Value> {
        self.json
            .get("questions")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    fn isolation_probes(&self) -> Vec<Value> {
        self.json
            .get("isolationProbes")
            .and_then(Value::as_array)
            .cloned()
            .unwrap_or_default()
    }

    /// The tools an expert created from this fixture may use. Kept narrow on
    /// purpose: the expert editor's own default is an empty `allowedTools`, so a
    /// knowledge expert gets exactly what it needs and no filesystem search that
    /// could reach the fixture files directly.
    fn required_tools(&self) -> Vec<String> {
        string_list(&self.json, "requiredTools")
    }
}

/// One expert + knowledge base + conversation triple declared by `fixture.json`.
struct ExpertPlan {
    key: String,
    display_name: String,
    prompt: String,
    description: String,
    knowledge_base_key: String,
    knowledge_base_name: String,
    documents: Vec<String>,
}

fn expert_plans(fixture: &Fixture) -> Result<Vec<ExpertPlan>, String> {
    let mut plans = Vec::new();
    for expert in fixture.experts()? {
        let key = required_str(expert, "key")?;
        let knowledge_base = expert
            .get("knowledgeBase")
            .ok_or_else(|| format!("fixture expert '{key}' has no knowledgeBase"))?;
        let display_name = required_str(expert, "displayName")?;
        let plan = ExpertPlan {
            key: key.to_owned(),
            display_name: display_name.to_owned(),
            prompt: required_str(expert, "prompt")?.to_owned(),
            description: format!("single-pi acceptance fixture expert ({key})"),
            knowledge_base_key: required_str(knowledge_base, "key")?.to_owned(),
            knowledge_base_name: required_str(knowledge_base, "name")?.to_owned(),
            documents: string_list(knowledge_base, "documents"),
        };
        if plan.documents.is_empty() {
            return Err(format!(
                "fixture expert '{key}' declares no knowledge documents"
            ));
        }
        plans.push(plan);
    }
    if plans.is_empty() {
        return Err(format!("{} declares no experts", fixture.path()));
    }
    Ok(plans)
}

/// The title of the fixture conversation for one expert. Stable, so re-running
/// the seed finds the conversation it created instead of adding another one.
fn conversation_title(plan: &ExpertPlan) -> String {
    format!("{} @ {}", plan.display_name, plan.knowledge_base_name)
}

/// The expert manifest the acceptance run needs: the bound knowledge reference
/// plus the fixture's `requiredTools` as the tool scope.
fn expert_manifest(knowledge_base_id: &str, allowed_tools: &[String]) -> Value {
    json!({
        "version": "1.0.0",
        "manifestSchemaVersion": 2,
        "skills": [],
        "knowledgeReferences": [{
            "source": "local",
            "providerKey": "local",
            "id": knowledge_base_id,
        }],
        "mcpServers": [],
        "allowedTools": allowed_tools,
    })
}
// ---------------------------------------------------------------------------
// seed
// ---------------------------------------------------------------------------

fn seed() -> Result<Value, String> {
    let directory = resolve_acceptance_dir()?;
    let fixture = Fixture::load()?;
    let plans = expert_plans(&fixture)?;

    eprintln!(
        "single-pi fixture: seeding {} expert(s) into {}",
        plans.len(),
        directory.data_dir.display()
    );

    let database = Database::open(directory.database_path.clone())
        .map_err(|error| format!("cannot open {}: {error}", directory.database_path.display()))?;
    let knowledge_root = resolve_knowledge_root(&directory.data_dir, &database)?;
    let store = LocalKnowledgeStore::open(&knowledge_root).map_err(|error| {
        format!(
            "cannot open the local knowledge store at {}: {error}",
            knowledge_root.display()
        )
    })?;
    let allowed_tools = fixture.required_tools();

    let mut actions: Vec<Value> = Vec::new();
    for plan in &plans {
        // 1. Knowledge base, find-or-create by the fixture's stable name.
        let (base, created) = find_or_create_base(&store, plan)?;
        actions.push(json!({
            "kind": "knowledgeBase",
            "key": plan.knowledge_base_key.clone(),
            "name": base.name.clone(),
            "id": base.id.clone(),
            "action": if created { "created" } else { "reused" },
        }));
        eprintln!(
            "single-pi fixture: knowledge base {} ({})",
            base.id,
            if created { "created" } else { "already present" }
        );

        // 2. Documents through the production import pipeline. A document that is
        //    already there under the same file name is never imported twice.
        for document in &plan.documents {
            let source = fixture.root.join(document);
            if !source.is_file() {
                return Err(format!(
                    "fixture knowledge document is missing: {}",
                    source.display()
                ));
            }
            let file_name = source
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or_else(|| format!("{} has no UTF-8 file name", source.display()))?
                .to_owned();
            let existing = store
                .list_documents(LocalKnowledgeDocumentsRequest {
                    knowledge_base_id: base.id.clone(),
                    query: None,
                })
                .map_err(|error| format!("cannot list documents of {}: {error}", base.id))?;
            if existing.iter().any(|item| item.display_name == file_name) {
                actions.push(json!({
                    "kind": "document",
                    "key": format!("{}:{}", plan.knowledge_base_key, file_name),
                    "knowledgeBaseId": base.id.clone(),
                    "action": "reused",
                }));
                eprintln!("single-pi fixture: document {file_name} already imported");
                continue;
            }
            let accepted = store
                .start_import(LocalKnowledgeImportRequest {
                    knowledge_base_id: base.id.clone(),
                    files: vec![LocalKnowledgeImportFileRequest {
                        source_path: source.to_string_lossy().into_owned(),
                        relative_path: None,
                    }],
                    parser_version: None,
                    chunk_config_hash: None,
                })
                .map_err(|error| format!("cannot import {}: {error}", source.display()))?;
            wait_for_import(&store, &base.id, &accepted.operation_id, &file_name)?;
            actions.push(json!({
                "kind": "document",
                "key": format!("{}:{}", plan.knowledge_base_key, file_name),
                "knowledgeBaseId": base.id.clone(),
                "action": "created",
            }));
            eprintln!("single-pi fixture: document {file_name} imported");
        }

        // 3. Expert: `agents` row with runtime_type 'pi', agent_kind 'expert', a
        //    distinct system prompt and the knowledge reference in its package
        //    manifest -- exactly what the expert editor writes.
        let manifest = expert_manifest(&base.id, &allowed_tools);
        let existing_expert = database
            .list_agents()
            .map_err(|error| format!("cannot list agents: {error}"))?
            .into_iter()
            .find(|agent| is_fixture_expert(agent, &plan.display_name));
        let request = SaveAgentRequest {
            id: existing_expert.as_ref().map(|agent| agent.id.clone()),
            name: plan.display_name.clone(),
            description: plan.description.clone(),
            icon: Some(EXPERT_ICON.to_owned()),
            category: EXPERT_CATEGORY.to_owned(),
            system_prompt: plan.prompt.clone(),
            // The sentinel every seeded agent carries: resolve the model from the
            // configured model service instead of a per-agent model id.
            default_model: "configured-model".to_owned(),
            opening_suggestions: Vec::new(),
            package_manifest: manifest,
        };
        let expert = database
            .save_agent(&request)
            .map_err(|error| format!("cannot save expert '{}': {error}", plan.display_name))?;
        actions.push(json!({
            "kind": "expert",
            "key": plan.key.clone(),
            "name": expert.name.clone(),
            "id": expert.id.clone(),
            "knowledgeBaseId": base.id.clone(),
            "action": if existing_expert.is_some() { "reused" } else { "created" },
        }));
        eprintln!(
            "single-pi fixture: expert {} ({})",
            expert.id,
            if existing_expert.is_some() { "updated in place" } else { "created" }
        );

        // 4. One conversation per expert, `agent_backend = 'fox'`, with the expert
        //    as the active binding and the knowledge base bound to it. A
        //    conversation can only be created for an assistant, so the expert is
        //    attached with the same binding API the UI uses when an expert avatar
        //    is opened.
        let title = conversation_title(plan);
        let existing_conversation = database
            .list_conversations()
            .map_err(|error| format!("cannot list conversations: {error}"))?
            .into_iter()
            .find(|conversation| conversation.title == title && !conversation.archived);
        let conversation = match &existing_conversation {
            Some(conversation) => conversation.clone(),
            None => database
                .create_conversation_with_backend(
                    database.default_agent_id(),
                    Some(title.as_str()),
                    // Knowledge chats are project-free, as the acceptance run drives them.
                    None,
                    Some("read_only"),
                    "fox",
                )
                .map_err(|error| format!("cannot create conversation '{title}': {error}"))?,
        };
        actions.push(json!({
            "kind": "conversation",
            "key": plan.key.clone(),
            "title": conversation.title.clone(),
            "id": conversation.id.clone(),
            "agentBackend": conversation.agent_backend.clone(),
            "action": if existing_conversation.is_some() { "reused" } else { "created" },
        }));

        let active = database
            .current_conversation_expert_binding(&conversation.id)
            .map_err(|error| format!("cannot read the expert binding: {error}"))?;
        let expert_binding_action = if active
            .as_ref()
            .is_some_and(|binding| binding.expert_id == expert.id)
        {
            "reused"
        } else {
            database
                .bind_conversation_expert(&conversation.id, &expert.id, ACTIVATION_SOURCE)
                .map_err(|error| format!("cannot bind expert '{}': {error:?}", expert.id))?;
            "created"
        };
        actions.push(json!({
            "kind": "expertBinding",
            "key": plan.key.clone(),
            "conversationId": conversation.id.clone(),
            "expertId": expert.id.clone(),
            "action": expert_binding_action,
        }));

        let bindings = database
            .conversation_knowledge_reference_bindings(&conversation.id)
            .map_err(|error| format!("cannot read the knowledge bindings: {error}"))?;
        let already_bound = bindings.iter().any(|binding| {
            binding.enabled && binding.reference.source == "local" && binding.reference.id == base.id
        });
        if !already_bound {
            let input = KnowledgeReferenceBindingInput {
                reference: KnowledgeReference::local(base.id.clone()),
                name: base.name.clone(),
            };
            database
                .set_knowledge_reference_bindings(&conversation.id, std::slice::from_ref(&input))
                .map_err(|error| {
                    format!(
                        "cannot bind knowledge base {} to {}: {error}",
                        base.id, conversation.id
                    )
                })?;
        }
        actions.push(json!({
            "kind": "knowledgeBinding",
            "key": plan.knowledge_base_key.clone(),
            "conversationId": conversation.id.clone(),
            "knowledgeBaseId": base.id.clone(),
            "action": if already_bound { "reused" } else { "created" },
        }));
        eprintln!(
            "single-pi fixture: conversation {} for expert {}",
            conversation.id,
            if existing_conversation.is_some() { "already present" } else { "created" }
        );
    }

    // Prove the end state from the same read-only path `dump` uses, so a seed
    // that silently produced nothing also fails visibly.
    let evidence = collect_evidence(&directory, &fixture, &knowledge_root, &store, &database)?;
    Ok(json!({
        "mode": "seed",
        "fixture": fixture.summary(),
        "dataDir": directory.data_dir.display().to_string(),
        "databasePath": directory.database_path.display().to_string(),
        "knowledgeRoot": knowledge_root.display().to_string(),
        "marker": MARKER_FILE,
        "actions": actions,
        "counts": evidence.get("counts").cloned().unwrap_or(Value::Null),
        "searchProbes": evidence.get("searchProbes").cloned().unwrap_or(Value::Null),
    }))
}

fn is_fixture_expert(agent: &AgentRecord, display_name: &str) -> bool {
    agent.name == display_name
        && agent.runtime_type == "pi"
        && agent.agent_kind == "expert"
        && agent.package_source == "local"
}

fn find_or_create_base(
    store: &LocalKnowledgeStore,
    plan: &ExpertPlan,
) -> Result<(LocalKnowledgeBase, bool), String> {
    let bases = store
        .list_bases()
        .map_err(|error| format!("cannot list knowledge bases: {error}"))?;
    if let Some(base) = bases
        .into_iter()
        .find(|base| base.name == plan.knowledge_base_name)
    {
        return Ok((base, false));
    }
    let created = store
        .create_base(CreateLocalKnowledgeBaseRequest {
            name: plan.knowledge_base_name.clone(),
            description: Some(format!(
                "single-pi acceptance fixture: {}",
                plan.knowledge_base_key
            )),
            embedding_model_id: None,
            chunk_size: None,
            chunk_overlap: None,
            // The production default; local lexical retrieval needs no model.
            search_mode: None,
        })
        .map_err(|error| {
            format!(
                "cannot create knowledge base '{}': {error}",
                plan.knowledge_base_name
            )
        })?;
    Ok((created, true))
}

/// Wait for one import operation's job rows to leave `queued`/`running`.
fn wait_for_import(
    store: &LocalKnowledgeStore,
    knowledge_base_id: &str,
    operation_id: &str,
    document: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + IMPORT_TIMEOUT;
    loop {
        let jobs: Vec<_> = store
            .list_jobs(Some(knowledge_base_id))
            .map_err(|error| format!("cannot list import jobs: {error}"))?
            .into_iter()
            .filter(|job| job.parent_operation_id.as_deref() == Some(operation_id))
            .collect();
        if jobs.is_empty() {
            return Err(format!(
                "import of {document} reported no job rows for operation {operation_id}"
            ));
        }
        let mut pending = 0usize;
        for job in &jobs {
            match job.status.as_str() {
                "completed" => {}
                "failed" | "cancelled" => {
                    return Err(format!(
                        "import of {document} {}: {} {}",
                        job.status,
                        job.error_code.as_deref().unwrap_or("-"),
                        job.error_message.as_deref().unwrap_or("-"),
                    ));
                }
                _ => pending += 1,
            }
        }
        if pending == 0 {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(format!(
                "import of {document} did not finish within {}s ({pending} job(s) still pending)",
                IMPORT_TIMEOUT.as_secs()
            ));
        }
        std::thread::sleep(Duration::from_millis(150));
    }
}
// ---------------------------------------------------------------------------
// dump
// ---------------------------------------------------------------------------

fn dump() -> Result<Value, String> {
    let directory = resolve_acceptance_dir()?;
    let fixture = Fixture::load()?;
    if !directory.database_path.is_file() {
        return Err(format!(
            "no Fox database at {}: run 'seed' first",
            directory.database_path.display()
        ));
    }
    let database = Database::open(directory.database_path.clone())
        .map_err(|error| format!("cannot open {}: {error}", directory.database_path.display()))?;
    let knowledge_root = resolve_knowledge_root(&directory.data_dir, &database)?;
    let store = LocalKnowledgeStore::open(&knowledge_root).map_err(|error| {
        format!(
            "cannot open the local knowledge store at {}: {error}",
            knowledge_root.display()
        )
    })?;
    collect_evidence(&directory, &fixture, &knowledge_root, &store, &database)
}

fn collect_evidence(
    directory: &AcceptanceDir,
    fixture: &Fixture,
    knowledge_root: &Path,
    store: &LocalKnowledgeStore,
    database: &Database,
) -> Result<Value, String> {
    let plans = expert_plans(fixture)?;

    // Knowledge bases, straight from the store the Host's knowledge tools use.
    let bases = store
        .list_bases()
        .map_err(|error| format!("cannot list knowledge bases: {error}"))?;
    let mut knowledge_bases = Vec::with_capacity(bases.len());
    let mut document_total = 0i64;
    for base in &bases {
        let documents = store
            .list_documents(LocalKnowledgeDocumentsRequest {
                knowledge_base_id: base.id.clone(),
                query: None,
            })
            .map_err(|error| format!("cannot list documents of {}: {error}", base.id))?;
        document_total += documents.len() as i64;
        knowledge_bases.push(json!({
            "id": base.id.clone(),
            "name": base.name.clone(),
            "description": base.description.clone(),
            "documentCount": base.document_count,
            "searchMode": base.search_mode.clone(),
            "textIndexReady": base.text_index_ready,
            "vectorIndexReady": base.vector_index_ready,
            "configuredEmbeddingModelId": base.configured_embedding_model_id.clone(),
            "activeIndexGeneration": base.active_index_generation.clone(),
            "documents": documents.iter().map(|document| json!({
                "id": document.id.clone(),
                "displayName": document.display_name.clone(),
                "relativePath": document.relative_path.clone(),
                "parseStatus": document.parse_status.clone(),
                "indexStatus": document.index_status.clone(),
                "chunkCount": document.chunk_count,
                "fileSize": document.file_size,
                "mimeType": document.mime_type.clone(),
            })).collect::<Vec<_>>(),
        }));
    }

    // Experts: only the ones this fixture owns, keyed by the fixture id.
    let agents = database
        .list_agents()
        .map_err(|error| format!("cannot list agents: {error}"))?;
    let mut experts = Vec::with_capacity(plans.len());
    let mut expert_total = 0usize;
    for plan in &plans {
        let base = bases
            .iter()
            .find(|base| base.name == plan.knowledge_base_name);
        let agent = agents
            .iter()
            .find(|agent| is_fixture_expert(agent, &plan.display_name));
        if agent.is_some() {
            expert_total += 1;
        }
        experts.push(json!({
            "key": plan.key.clone(),
            "displayName": plan.display_name.clone(),
            "present": agent.is_some(),
            "id": agent.map(|agent| agent.id.clone()),
            "runtimeType": agent.map(|agent| agent.runtime_type.clone()),
            "agentKind": agent.map(|agent| agent.agent_kind.clone()),
            "invocationMode": agent.map(|agent| agent.invocation_mode.clone()),
            "visibility": agent.map(|agent| agent.visibility.clone()),
            "packageSource": agent.map(|agent| agent.package_source.clone()),
            "defaultModel": agent.map(|agent| agent.default_model.clone()),
            "systemPrompt": agent.map(|agent| agent.system_prompt.clone()),
            "systemPromptSha256": agent.map(|agent| sha256_hex(&agent.system_prompt)),
            "declaredKnowledgeReferences": agent
                .map(|agent| agent.package_manifest.get("knowledgeReferences").cloned().unwrap_or(Value::Null))
                .unwrap_or(Value::Null),
            "allowedTools": agent
                .map(|agent| agent.package_manifest.get("allowedTools").cloned().unwrap_or(Value::Null))
                .unwrap_or(Value::Null),
            "knowledgeBaseKey": plan.knowledge_base_key.clone(),
            "knowledgeBaseName": plan.knowledge_base_name.clone(),
            "knowledgeBaseId": base.map(|base| base.id.clone()),
        }));
    }

    // Conversations with their frozen backend, expert binding and knowledge
    // references. `runtime_sessions` is keyed per conversation, so this scan
    // covers every row of that table.
    let conversations = database
        .list_conversations()
        .map_err(|error| format!("cannot list conversations: {error}"))?;
    let mut conversation_rows = Vec::with_capacity(conversations.len());
    let mut runtime_sessions = Vec::new();
    let mut seen_sessions: Vec<String> = Vec::new();
    for conversation in &conversations {
        let expert_binding: Option<ConversationExpertBinding> = database
            .current_conversation_expert_binding(&conversation.id)
            .map_err(|error| format!("cannot read the expert binding: {error}"))?;
        let knowledge: Vec<KnowledgeReferenceBindingRecord> = database
            .conversation_knowledge_reference_bindings(&conversation.id)
            .map_err(|error| format!("cannot read the knowledge bindings: {error}"))?;
        let session: Option<RuntimeSessionRecord> = database
            .get_runtime_session(&conversation.id)
            .map_err(|error| format!("cannot read the runtime session: {error}"))?;
        if let Some(session) = &session {
            if !seen_sessions.contains(&session.runtime_session_id) {
                seen_sessions.push(session.runtime_session_id.clone());
                runtime_sessions.push(runtime_session_row(&conversation.id, session));
            }
        }
        conversation_rows.push(json!({
            "id": conversation.id.clone(),
            "title": conversation.title.clone(),
            "agentId": conversation.agent_id.clone(),
            "agentName": conversation.agent_name.clone(),
            "agentBackend": conversation.agent_backend.clone(),
            "status": conversation.status.clone(),
            "projectRoot": conversation.project_root.clone(),
            "permissionMode": conversation.permission_mode.clone(),
            "fixtureKey": conversation_fixture_key(&plans, conversation),
            "expertBinding": expert_binding.as_ref().map(expert_binding_row),
            "knowledgeReferences": knowledge.iter().map(knowledge_reference_row).collect::<Vec<_>>(),
            "runtimeSession": session.as_ref().map(|session| runtime_session_row(&conversation.id, session)),
        }));
    }

    let search_probes = search_probes(fixture, store, database, &bases, &conversation_rows)?;

    Ok(json!({
        "schema": "single-pi-acceptance-fixture/dump/v1",
        "fixture": fixture.summary(),
        "dataDir": directory.data_dir.display().to_string(),
        "databasePath": directory.database_path.display().to_string(),
        "knowledgeRoot": knowledge_root.display().to_string(),
        "marker": MARKER_FILE,
        "counts": {
            "knowledgeBases": knowledge_bases.len(),
            "documents": document_total,
            "experts": expert_total,
            "conversations": conversation_rows.len(),
            "runtimeSessions": runtime_sessions.len(),
        },
        "knowledgeBases": knowledge_bases,
        "experts": experts,
        "conversations": conversation_rows,
        "runtimeSessions": runtime_sessions,
        "searchProbes": search_probes,
    }))
}

fn conversation_fixture_key(plans: &[ExpertPlan], conversation: &ConversationSummary) -> Value {
    match plans
        .iter()
        .find(|plan| conversation_title(plan) == conversation.title)
    {
        Some(plan) => Value::String(plan.key.clone()),
        None => Value::Null,
    }
}

fn expert_binding_row(binding: &ConversationExpertBinding) -> Value {
    json!({
        "id": binding.id.clone(),
        "expertId": binding.expert_id.clone(),
        "state": binding.state.clone(),
        "activationSource": binding.activation_source.clone(),
        "expertVersion": binding.expert_version.clone(),
        "packageHash": binding.package_hash.clone(),
        "displaySnapshot": binding.display_snapshot_json.clone(),
    })
}

fn knowledge_reference_row(binding: &KnowledgeReferenceBindingRecord) -> Value {
    json!({
        "source": binding.reference.source.clone(),
        "providerKey": binding.reference.provider_key.clone(),
        "connectionId": binding.reference.connection_id.clone(),
        "id": binding.reference.id.clone(),
        "name": binding.knowledge_base_name.clone(),
        "enabled": binding.enabled,
    })
}

fn runtime_session_row(conversation_id: &str, session: &RuntimeSessionRecord) -> Value {
    json!({
        "conversationId": conversation_id,
        "id": session.runtime_session_id.clone(),
        "formatVersion": session.format_version,
        "nativeSessionPath": session.native_session_path.clone(),
        "nativeSessionRef": session.native_session_ref.clone(),
        "sessionPath": session.session_path.clone(),
        "status": session.status.clone(),
        "contextGeneration": session.context_generation,
        "runtimeVersion": session.runtime_version.clone(),
    })
}
// ---------------------------------------------------------------------------
// Search probes
// ---------------------------------------------------------------------------

/// Exercise the retrieval the Host's `search_knowledge` / `read_knowledge_document`
/// tools perform for a `source: local` target, plus the binding gate that decides
/// whether that target is reachable from the conversation at all.
fn search_probes(
    fixture: &Fixture,
    store: &LocalKnowledgeStore,
    database: &Database,
    bases: &[LocalKnowledgeBase],
    conversations: &[Value],
) -> Result<Value, String> {
    let plans = expert_plans(fixture)?;
    let mut questions = Vec::new();
    let mut isolation = Vec::new();
    let mut all_ok = true;

    for question in fixture.questions() {
        let expert_key = required_str(&question, "expert")?;
        let base_key = required_str(&question, "knowledgeBase")?;
        let expected_fact = required_str(&question, "expectedFact")?;
        let expected_citation = required_str(&question, "expectedCitation")?;
        if !plans.iter().any(|plan| plan.key == expert_key) {
            return Err(format!("question names unknown expert '{expert_key}'"));
        }
        let base = base_for_key(&plans, bases, base_key)
            .ok_or_else(|| format!("knowledge base '{base_key}' is not present"))?;
        let conversation = conversations
            .iter()
            .find(|row| row["fixtureKey"] == json!(expert_key));
        // The Host's own instruction is to retry a long question with a few short
        // domain keywords; the fixture carries that query per question.
        let query = question
            .get("searchQuery")
            .and_then(Value::as_str)
            .unwrap_or(expected_citation);

        let conversation_id = conversation.and_then(|row| row["id"].as_str());
        let bound = match conversation_id {
            Some(conversation_id) => database
                .conversation_knowledge_reference_bindings(conversation_id)
                .map_err(|error| format!("cannot read the knowledge bindings: {error}"))?
                .iter()
                .any(|binding| {
                    binding.enabled
                        && binding.reference.source == "local"
                        && binding.reference.id == base.id
                }),
            None => false,
        };
        let found = store
            .search_lexical(&base.id, query, None, None)
            .map_err(|error| format!("search_knowledge on {} failed: {error}", base.id))?;
        let content = joined_content(&found);
        let fact_found = content.contains(expected_fact);
        let citation_found = content.contains(expected_citation);
        let item_count = found
            .get("items")
            .and_then(Value::as_array)
            .map(|items| items.len())
            .unwrap_or(0);
        let document_ids: Vec<String> = found
            .get("items")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|item| item.get("documentId").and_then(Value::as_str))
            .map(str::to_owned)
            .collect();

        let (read_id, read_content) = read_first_document(store, &base.id)?;
        let read_fact = read_content
            .as_deref()
            .is_some_and(|text| text.contains(expected_fact));
        let read_citation = read_content
            .as_deref()
            .is_some_and(|text| text.contains(expected_citation));

        let ok = bound && fact_found && citation_found && read_fact && read_citation;
        all_ok &= ok;
        questions.push(json!({
            "expert": expert_key,
            "knowledgeBase": base_key,
            "knowledgeBaseId": base.id.clone(),
            "question": question.get("question").cloned().unwrap_or(Value::Null),
            "searchQuery": query,
            "bindingGate": {
                "conversationId": conversation_id,
                "boundToConversation": bound,
            },
            "search": {
                "itemCount": item_count,
                "documentIds": document_ids,
                "expectedFact": expected_fact,
                "expectedFactFound": fact_found,
                "expectedCitation": expected_citation,
                "expectedCitationFound": citation_found,
            },
            "read": {
                "documentId": read_id,
                "expectedFactFound": read_fact,
                "expectedCitationFound": read_citation,
            },
            "ok": ok,
        }));
    }

    for probe in fixture.isolation_probes() {
        let expert_key = required_str(&probe, "expert")?;
        let forbidden_fact = required_str(&probe, "forbiddenFact")?;
        let plan = plans
            .iter()
            .find(|plan| plan.key == expert_key)
            .ok_or_else(|| format!("isolation probe names unknown expert '{expert_key}'"))?;
        let own_base = base_for_key(&plans, bases, &plan.knowledge_base_key)
            .ok_or_else(|| format!("knowledge base '{}' is not present", plan.knowledge_base_name))?;
        let other_base = plans
            .iter()
            .find(|other| other.knowledge_base_key != plan.knowledge_base_key)
            .and_then(|other| base_for_key(&plans, bases, &other.knowledge_base_key));
        let conversation = conversations
            .iter()
            .find(|row| row["fixtureKey"] == json!(expert_key));
        let query = probe
            .get("searchQuery")
            .and_then(Value::as_str)
            .unwrap_or(forbidden_fact);

        let bound_ids: Vec<String> = conversation
            .and_then(|row| row["knowledgeReferences"].as_array())
            .into_iter()
            .flatten()
            .filter_map(|reference| reference["id"].as_str())
            .map(str::to_owned)
            .collect();
        let other_bound = other_base
            .map(|other| bound_ids.contains(&other.id))
            .unwrap_or(false);

        let found = store
            .search_lexical(&own_base.id, query, None, None)
            .map_err(|error| format!("search_knowledge on {} failed: {error}", own_base.id))?;
        let leaked = joined_content(&found).contains(forbidden_fact);
        let ok = !leaked && !other_bound;
        all_ok &= ok;
        isolation.push(json!({
            "expert": expert_key,
            "question": probe.get("question").cloned().unwrap_or(Value::Null),
            "searchQuery": query,
            "searchedKnowledgeBaseId": own_base.id.clone(),
            "forbiddenFact": forbidden_fact,
            "forbiddenFactLeaked": leaked,
            "otherKnowledgeBaseId": other_base.map(|base| base.id.clone()),
            "otherKnowledgeBaseBoundToThisConversation": other_bound,
            "otherBindingRefused": !other_bound,
            "ok": ok,
        }));
    }

    Ok(json!({
        "method": "LocalKnowledgeStore::search_lexical and ::read_document -- the exact calls runtime_host::execute_bound_knowledge_tool makes for a source:'local' target of search_knowledge / read_knowledge_document",
        "bindingGate": "replicated from runtime_host::execute_bound_knowledge_tool's ensure_bound using Database::conversation_knowledge_reference_bindings; the composed Host tool (tool guard, approval, Run record) is NOT exercised by this binary",
        "allOk": all_ok,
        "questions": questions,
        "isolation": isolation,
    }))
}

fn base_for_key<'a>(
    plans: &[ExpertPlan],
    bases: &'a [LocalKnowledgeBase],
    base_key: &str,
) -> Option<&'a LocalKnowledgeBase> {
    let plan = plans
        .iter()
        .find(|plan| plan.knowledge_base_key == base_key)?;
    bases
        .iter()
        .find(|base| base.name == plan.knowledge_base_name)
}

/// `read_knowledge_document` on the base's first document, as the Host tool does.
fn read_first_document(
    store: &LocalKnowledgeStore,
    knowledge_base_id: &str,
) -> Result<(Option<String>, Option<String>), String> {
    let documents = store
        .list_documents(LocalKnowledgeDocumentsRequest {
            knowledge_base_id: knowledge_base_id.to_owned(),
            query: None,
        })
        .map_err(|error| format!("cannot list documents of {knowledge_base_id}: {error}"))?;
    let Some(document) = documents.first() else {
        return Ok((None, None));
    };
    let value = store
        .read_document(knowledge_base_id, &document.id, None, None)
        .map_err(|error| format!("read_knowledge_document on {} failed: {error}", document.id))?;
    Ok((
        Some(document.id.clone()),
        Some(
            value
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
    ))
}

fn joined_content(value: &Value) -> String {
    value
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|item| item.get("content").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n")
}

fn sha256_hex(value: &str) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(value.as_bytes()))
}

// ---------------------------------------------------------------------------
// Small JSON helpers
// ---------------------------------------------------------------------------

fn required_str<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| format!("fixture entry is missing the string field '{key}'"))
}

fn string_list(value: &Value, key: &str) -> Vec<String> {
    value
        .get(key)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(str::to_owned)
        .collect()
}