use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
};
use uuid::Uuid;

#[derive(Deserialize, Serialize)]
pub(crate) struct Header {
    pub(crate) title: String,
    #[serde(default)]
    pub(crate) aliases: Vec<String>,
}

pub(crate) fn render(title: &str, aliases: &[String], body: &str) -> String {
    format!(
        "---\ntitle: {}\naliases: {}\n---\n{}",
        serde_json::json!(title),
        serde_json::json!(aliases),
        body
    )
}

pub(crate) fn parse(text: &str) -> (Option<Header>, String) {
    let normalized = text.replace("\r\n", "\n");
    if let Some((header, body)) = normalized
        .strip_prefix("---\n")
        .and_then(|rest| rest.split_once("\n---\n"))
    {
        if let Ok(header) = serde_yaml::from_str(header) {
            return (Some(header), body.to_owned());
        }
    }
    (None, normalized)
}

pub(crate) fn sha(text: &str) -> String {
    hex::encode(Sha256::digest(text.as_bytes()))
}

pub(crate) fn topics_dir(data_dir: &Path, owner: &str) -> io::Result<PathBuf> {
    if Path::new(owner).file_name().and_then(|x| x.to_str()) != Some(owner) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "联系人目录无效",
        ));
    }
    let root = data_dir.canonicalize()?;
    let dir = crate::data_paths::companion_notes_dir(&root, owner).join("topics");
    if dir.canonicalize()? != dir {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "笔记目录已指向其他位置",
        ));
    }
    Ok(dir)
}

pub(crate) fn topic_path(data_dir: &Path, owner: &str, file_name: &str) -> io::Result<PathBuf> {
    let invalid = || io::Error::new(io::ErrorKind::InvalidInput, "专题文件位于笔记目录外");
    if Path::new(file_name).file_name().and_then(|x| x.to_str()) != Some(file_name)
        || !file_name.starts_with("ntp_")
        || !file_name.ends_with(".md")
    {
        return Err(invalid());
    }
    let dir = topics_dir(data_dir, owner)?;
    let path = dir.join(file_name);
    match fs::symlink_metadata(&path) {
        Ok(metadata) if !metadata.is_file() || path.canonicalize()? != path => Err(invalid()),
        Ok(_) => Ok(path),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(path),
        Err(err) => Err(err),
    }
}

pub(crate) fn replace(path: &Path, text: &str) -> io::Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", Uuid::new_v4()));
    fs::write(&tmp, text)?;
    if let Err(error) = fs::rename(&tmp, path) {
        let _ = fs::remove_file(tmp);
        return Err(error);
    }
    Ok(())
}

pub(crate) fn tokens(text: &str) -> BTreeSet<String> {
    let mut result = BTreeSet::new();
    let mut previous = None;
    let mut word = String::new();
    for ch in text.chars().chain(std::iter::once(' ')) {
        let cjk =
            matches!(ch,'\u{3400}'..='\u{9fff}'|'\u{f900}'..='\u{faff}'|'\u{20000}'..='\u{2fa1f}');
        if cjk {
            if let Some(first) = previous {
                result.insert(format!("{first}{ch}"));
            }
            previous = Some(ch);
        } else {
            previous = None;
        }
        if ch.is_ascii_alphanumeric() {
            word.push(ch.to_ascii_lowercase());
        } else if !word.is_empty() {
            result.insert(std::mem::take(&mut word));
        }
    }
    result
}

pub(crate) fn chunks(body: &str) -> Vec<(String, String)> {
    let mut chunks = Vec::new();
    let mut heading = String::new();
    let mut lines = Vec::new();
    for line in body.lines().chain(std::iter::once("")) {
        if line.trim().is_empty() || line.starts_with('#') {
            let paragraph = lines.join("\n");
            lines.clear();
            let mut chars = paragraph.chars().peekable();
            while chars.peek().is_some() {
                chunks.push((heading.clone(), chars.by_ref().take(800).collect()));
            }
            if line.starts_with('#') {
                heading = line.trim_start_matches('#').trim().to_owned();
            }
        } else if !line.starts_with("<!-- fox-") {
            lines.push(line);
        }
    }
    chunks
}
