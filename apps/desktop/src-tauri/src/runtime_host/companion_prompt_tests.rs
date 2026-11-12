use super::{conversation_enabled_skill_ids, conversation_prompt_context, PromptScopeOverrides};
use crate::database::Database;
use serde_json::{json, Value};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Stdio},
};

const SKILLS: [&str; 3] = ["a24-s1", "a24-s2", "a24-s3"];
const EXPERT: &str = "fox-debugger";

fn publish(db: &Database, version: &str, skills: &[&str]) {
    db.with_connection(|c| {
        c.execute(
            "UPDATE agents SET system_prompt=?1,package_version=?2,
            package_manifest_json=json_set(package_manifest_json,'$.skills',json(?3)) WHERE id=?4",
            rusqlite::params![
                format!("expert-instructions-{version}"),
                version,
                json!(skills).to_string(),
                EXPERT
            ],
        )?;
        Ok(())
    })
    .unwrap();
}

fn check_stage(
    db: &Database,
    root: &Path,
    chat: &str,
    stage: &str,
    version: &str,
    expected: &[&str],
) {
    let context = conversation_prompt_context(
        db,
        &root.join("skills"),
        chat,
        "选用合适技能",
        "primary",
        PromptScopeOverrides {
            tool_allowlist: None,
            mcp_server_scope: None,
        },
    )
    .unwrap();
    let expert = context.expert_package.as_ref().unwrap();
    assert_eq!(
        expert["enabledSkills"],
        json!(expected),
        "{stage} enabled skills"
    );
    assert_eq!(
        expert["packageManifest"]["skills"],
        json!(expected),
        "{stage} runtime manifest"
    );
    assert_eq!(
        expert["systemPrompt"],
        format!("expert-instructions-{version}")
    );
    let data = json!({
        "systemPrompt":context.system_prompt,
        "context":{"assistantPackage":context.assistant_package,"expertPackage":expert,
            "skillPrompt":context.skill_prompt,"expertBinding":context.expert_binding_payload,
            "companionContext":db.companion_runtime_notes(chat,"选用合适技能").unwrap().unwrap()},
    });
    let runtime = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../services/agent-runtime");
    let mut command = Command::new("node");
    command.current_dir(runtime).args(["--input-type=module","--eval",
        "import {readFileSync} from 'node:fs'; import {composeRuntimePrompt} from './src/runtime-instructions.mjs'; process.stdout.write(composeRuntimePrompt(JSON.parse(readFileSync(0,'utf8'))).prompt)"])
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(&crate::canonical_json::to_vec(&data).unwrap())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let prompt = String::from_utf8(output.stdout).unwrap();
    if let Some(dir) = std::env::var_os("FOX_TEST_COMPANION_PROMPT_CAPTURE_DIR") {
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            Path::new(&dir).join(format!("{stage}-host.json")),
            crate::canonical_json::to_vec_pretty(&data).unwrap(),
        )
        .unwrap();
        fs::write(Path::new(&dir).join(format!("{stage}-prompt.txt")), &prompt).unwrap();
    }
    assert!(prompt.contains(&format!("expert-instructions-{version}")));
    for skill in SKILLS {
        assert_eq!(
            context.skill_prompt.contains(skill),
            expected.contains(&skill),
            "{stage} Host skill prompt: {skill}"
        );
        assert_eq!(
            prompt.contains(skill),
            expected.contains(&skill),
            "{stage} JavaScript prompt: {skill}"
        );
    }
}

#[test]
fn companion_source_updates_do_not_change_frozen_skills_or_prompt() {
    let root = std::env::temp_dir().join(format!("fox-companion-a24-{}", uuid::Uuid::new_v4()));
    fs::create_dir_all(&root).unwrap();
    let db = Database::open(root.join("fox.db")).unwrap();
    for skill in SKILLS {
        let dir = root.join("skills").join(skill);
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("SKILL.md"),
            format!(
                "---\nid: {skill}\nname: {skill}\nversion: 1.0\n---\nInstructions for {skill}."
            ),
        )
        .unwrap();
        db.set_agent_skill_enabled(EXPERT, skill, true).unwrap();
    }
    // The general assistant can enable a skill independently of this expert.
    db.set_agent_skill_enabled("fox-general", SKILLS[2], true)
        .unwrap();
    db.set_agent_skill_enabled("fox-general", SKILLS[1], true)
        .unwrap();
    publish(&db, "v1", &SKILLS[..2]);
    let contact = db.add_companion(&root, EXPERT).unwrap();
    let id = &contact.profile.id;
    let chat = &contact.conversation_id;
    publish(&db, "v2", &[SKILLS[0], SKILLS[2]]);
    check_stage(&db, &root, chat, "before-adoption", "v1", &SKILLS[..2]);
    db.set_agent_skill_enabled(EXPERT, SKILLS[1], false)
        .unwrap();
    check_stage(&db, &root, chat, "disable-s2-source-stays-frozen", "v1", &SKILLS[..2]);
    assert!(conversation_enabled_skill_ids(&db, &root.join("skills"), chat)
        .unwrap()
        .contains(&SKILLS[1].to_owned()));
    check_stage(&db, &root, chat, "source-v2-stays-frozen", "v1", &SKILLS[..2]);
    fs::remove_dir_all(root.join("skills").join(SKILLS[0])).unwrap();
    check_stage(&db, &root, chat, "uninstall-s1", "v1", &SKILLS[1..2]);
    drop(db);
    fs::remove_dir_all(root).unwrap();
}
