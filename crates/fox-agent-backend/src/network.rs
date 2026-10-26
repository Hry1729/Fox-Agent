use futures::future::BoxFuture;
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NetworkTarget {
    pub host: String,
    pub port: u16,
}

impl NetworkTarget {
    pub fn validated(self) -> Option<Self> {
        if self.port == 0 || self.host.is_empty() || self.host.len() > 253 {
            return None;
        }
        let parsed = url::Host::parse(&self.host).ok()?;
        let host = parsed
            .to_string()
            .trim_end_matches('.')
            .to_ascii_lowercase();
        if host.is_empty() || host.contains('*') {
            return None;
        }
        Some(Self {
            host,
            port: self.port,
        })
    }

    pub fn key(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

pub type NetworkCallback = Arc<dyn Fn(NetworkTarget) -> BoxFuture<'static, bool> + Send + Sync>;

type ProjectRules = std::collections::BTreeMap<String, Vec<NetworkTarget>>;

pub fn project_rules(root: &std::path::Path, project: &str) -> Result<Vec<NetworkTarget>, String> {
    let rules: ProjectRules =
        crate::statefile::read_json(&root.join("project-network-rules.json"))?;
    Ok(rules.get(project).cloned().unwrap_or_default())
}

pub fn set_project_rule(
    root: &std::path::Path,
    project: &str,
    target: NetworkTarget,
    allowed: bool,
) -> Result<(), String> {
    if project.is_empty() {
        return Err("项目不能为空".into());
    }
    let target = target.validated().ok_or("网络目标无效")?;
    let path = root.join("project-network-rules.json");
    let _lock = crate::statefile::StateLock::acquire(&crate::statefile::lock_path_for(&path))
        .map_err(|error| error.to_string())?;
    let mut rules: ProjectRules = crate::statefile::read_json(&path)?;
    let targets = rules.entry(project.to_owned()).or_default();
    targets.retain(|existing| existing != &target);
    if allowed {
        if targets.len() >= 256 {
            return Err("此项目的网络规则已达到上限".into());
        }
        targets.push(target);
    }
    crate::statefile::write_json(&path, &rules)
}

/// Owned by the ACP connection. Dropping it closes the independent Host channel.
pub struct NetworkBroker(tokio::task::JoinHandle<()>);
impl Drop for NetworkBroker {
    fn drop(&mut self) {
        self.0.abort();
    }
}

impl NetworkBroker {
    pub fn attach(
        prepared: &mut crate::sandbox::PreparedLaunch,
        callback: NetworkCallback,
    ) -> Result<Self, String> {
        use crate::launch::BridgeEndpoint;
        #[cfg(windows)]
        let sid = {
            use crate::winacl::WindowsAclProbe;
            Some(crate::winacl::RealWindowsAcl.current_user_sid()?)
        };
        #[cfg(not(windows))]
        let sid = None;
        // Unlike the ACP bridge, this endpoint admits only the Host account.
        let endpoint = BridgeEndpoint::new_local(sid)?;
        let listener =
            crate::bridge::BridgeListener::bind(&endpoint).map_err(|error| error.to_string())?;
        let session = prepared
            .sandbox
            .as_ref()
            .ok_or("network broker requires sandbox settings")?;
        let folder = session
            .settings_path()
            .parent()
            .ok_or("missing sandbox settings directory")?;
        let wrapper = folder.join("network-host.mjs");
        let control = folder.join("network-host.json");
        let original = prepared.spec.args.clone();
        let cli = std::path::Path::new(original.first().ok_or("missing srt entry")?);
        let library = cli
            .parent()
            .ok_or("missing srt directory")?
            .join("index.js");
        crate::onboarding::write_private(&wrapper, include_bytes!("network-host.mjs"))
            .map_err(|error| error.to_string())?;
        let config = serde_json::json!({ "address": endpoint.address, "token": endpoint.token, "library": library, "args": &original[1..] });
        crate::onboarding::write_private(
            &control,
            &serde_json::to_vec(&config).map_err(|error| error.to_string())?,
        )
        .map_err(|error| error.to_string())?;
        prepared.spec.args = vec![
            wrapper.to_string_lossy().into_owned(),
            control.to_string_lossy().into_owned(),
        ];
        let task = tokio::spawn(async move {
            let Ok(Ok(stream)) =
                tokio::time::timeout(std::time::Duration::from_secs(30), listener.accept()).await
            else {
                return;
            };
            let (read, mut write) = tokio::io::split(stream);
            let mut read = BufReader::new(read);
            let (tx, mut rx) = tokio::sync::mpsc::channel::<(u64, BoxFuture<'static, bool>)>(8);
            let receive = async move {
                loop {
                    let mut bytes = Vec::new();
                    let Ok(count) = (&mut read).take(2049).read_until(b'\n', &mut bytes).await
                    else {
                        break;
                    };
                    if count == 0 || count > 2048 || bytes.last() != Some(&b'\n') {
                        break;
                    }
                    #[derive(Deserialize)]
                    struct Request {
                        id: u64,
                        host: String,
                        port: u16,
                    }
                    let Ok(request) = serde_json::from_slice::<Request>(&bytes) else {
                        break;
                    };
                    let decision = match (NetworkTarget {
                        host: request.host,
                        port: request.port,
                    })
                    .validated()
                    {
                        Some(target) => callback(target),
                        None => Box::pin(async { false }),
                    };
                    if tx.send((request.id, decision)).await.is_err() {
                        break;
                    }
                }
            };
            let respond = async move {
                use futures::{stream::FuturesUnordered, StreamExt};
                let mut pending: FuturesUnordered<BoxFuture<'static, (u64, bool)>> =
                    FuturesUnordered::new();
                loop {
                    tokio::select! {
                        request = rx.recv(), if pending.len() < 8 => {
                            let Some((id, decision)) = request else { break; };
                            pending.push(Box::pin(async move { (id, decision.await) }));
                        }
                        result = pending.next(), if !pending.is_empty() => {
                            let Some((id, allowed)) = result else { continue; };
                            let response = format!("{{\"id\":{id},\"allow\":{allowed}}}\n");
                            if write.write_all(response.as_bytes()).await.is_err() { break; }
                        }
                    }
                }
            };
            tokio::select! { _ = receive => {}, _ = respond => {} }
        });
        Ok(Self(task))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn network_targets_are_exact_and_canonical() {
        assert_eq!(
            NetworkTarget {
                host: "EXAMPLE.COM.".into(),
                port: 443
            }
            .validated()
            .unwrap()
            .key(),
            "example.com:443"
        );
        assert_eq!(
            NetworkTarget {
                host: "127.1".into(),
                port: 8080
            }
            .validated()
            .unwrap()
            .key(),
            "127.0.0.1:8080"
        );
        for host in ["", "*.example.com", "a/b", "a\0b", "https://example.com"] {
            assert!(
                NetworkTarget {
                    host: host.into(),
                    port: 443
                }
                .validated()
                .is_none(),
                "{host:?}"
            );
        }
        assert!(NetworkTarget {
            host: "localhost".into(),
            port: 0
        }
        .validated()
        .is_none());
    }

    #[test]
    fn project_rules_do_not_cross_projects_or_ports_and_can_be_removed() {
        let folder = tempfile::tempdir().unwrap();
        let target = NetworkTarget {
            host: "registry.npmjs.org".into(),
            port: 443,
        };
        set_project_rule(folder.path(), "project-a", target.clone(), true).unwrap();
        assert_eq!(
            project_rules(folder.path(), "project-a").unwrap(),
            [target.clone()]
        );
        assert!(project_rules(folder.path(), "project-b")
            .unwrap()
            .is_empty());
        assert!(!project_rules(folder.path(), "project-a")
            .unwrap()
            .contains(&NetworkTarget {
                port: 80,
                ..target.clone()
            }));
        set_project_rule(folder.path(), "project-a", target.clone(), false).unwrap();
        assert!(project_rules(folder.path(), "project-a")
            .unwrap()
            .is_empty());
        std::fs::write(folder.path().join("project-network-rules.json"), "broken").unwrap();
        assert!(project_rules(folder.path(), "project-a").is_err());
        assert!(set_project_rule(folder.path(), "project-a", target, true).is_err());
    }
}
