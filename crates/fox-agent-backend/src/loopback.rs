//! Loopback model gateway: derive the one loopback port a backend's own base
//! URL points at, so the sandbox allowlist can name exactly that port.
//!
//! Same mechanism as the scripted pi model of round 6 (docs/sandbox.md 6.5 /
//! 13.8): an IP-literal `127.0.0.1:<port>` entry in srt's
//! `network.allowedDomains` lets the srt proxy dial that one loopback port
//! (srt's resolved-address guard leaves allow-listed IP literals alone).
//! Nothing else on loopback is opened.
//!
//! Rules ([`derive_loopback_gateway`]):
//!
//! * only a base URL whose host is loopback (`127.0.0.0/8`, `localhost`,
//!   `*.localhost`, `::1`) is considered; a remote base URL adds nothing here
//!   (remote model APIs go through the ordinary domain allowlist);
//! * the port is the explicit port, else the scheme default (http 80,
//!   https 443);
//! * a base URL whose value cannot be read as a literal URL (YAML `!!js`, a
//!   block scalar, `${VAR}` / `$VAR` interpolation, an unknown scheme, a bad
//!   port) means the port cannot be determined: nothing is added and a
//!   diagnostic says why (fail closed);
//! * two different loopback endpoints are ambiguous: nothing is added, with a
//!   diagnostic.
//!
//! What this does NOT do: make a client use the proxy. A client that always
//! connects to loopback directly (dsh's `@deepseek-ai/dsh-http-proxy`
//! `proxyForUrl` returns `undefined` for every loopback host) never reaches
//! the srt proxy, so the allowlist entry alone does not get its request out
//! of the sandbox: on Windows the WFP fence blocks direct loopback outside
//! srt's proxy port range, on Linux the bwrap network namespace has its own
//! loopback. See docs/sandbox.md 13.10.

use serde::{Deserialize, Serialize};

/// The one loopback endpoint the sandbox allows for a backend's model gateway.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LoopbackGateway {
    /// Host as written in the base URL (`127.0.0.1`, `localhost`, `::1`).
    pub host: String,
    pub port: u16,
    /// The srt `allowedDomains` entry (`127.0.0.1:3425`, `[::1]:3425`).
    pub entry: String,
    /// Where the base URL came from (file and key, never the whole value's
    /// secrets: base URLs carry no credentials, userinfo is refused).
    pub source: String,
}

/// Result of [`derive_loopback_gateway`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LoopbackDerivation {
    pub gateway: Option<LoopbackGateway>,
    /// Why no port was allowed (or what was ignored), in plain words.
    pub diagnostics: Vec<String>,
}

/// One base URL as read from a configuration source.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BaseUrlCandidate {
    /// e.g. `<private HOME>/.dsh/cordis.patch.yml baseURL`.
    pub source: String,
    /// The raw value as written (quotes already removed).
    pub value: String,
}

/// What a base URL value amounts to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BaseUrlKind {
    Loopback { host: String, port: u16 },
    Remote,
    /// The value is not a literal http(s) URL with a usable port.
    Undetermined(String),
}

fn octet(text: &str) -> bool {
    !text.is_empty() && text.len() <= 3 && text.bytes().all(|byte| byte.is_ascii_digit()) && text.parse::<u16>().is_ok_and(|value| value <= 255)
}

/// Whether `host` (unbracketed, lower case) names this machine's loopback.
pub fn is_loopback_host(host: &str) -> bool {
    let host = host.trim_end_matches('.').to_ascii_lowercase();
    if host == "localhost" || host.ends_with(".localhost") || host == "::1" {
        return true;
    }
    let host = host.strip_prefix("::ffff:").unwrap_or(&host);
    let parts: Vec<&str> = host.split('.').collect();
    parts.len() == 4 && parts[0] == "127" && parts.iter().all(|part| octet(part))
}

/// Classify one base URL value.
pub fn classify_base_url(value: &str) -> BaseUrlKind {
    let value = value.trim();
    if value.is_empty() {
        return BaseUrlKind::Undetermined("empty value".into());
    }
    if value.starts_with("!!") || value.starts_with('>') || value.starts_with('|') || value.contains("${") || value.contains('$') || value.contains("process.env") {
        return BaseUrlKind::Undetermined("computed value (expression or variable), not a literal URL".into());
    }
    let lower = value.to_ascii_lowercase();
    let (rest, default_port) = if let Some(rest) = lower.strip_prefix("http://") {
        (rest, 80u16)
    } else if let Some(rest) = lower.strip_prefix("https://") {
        (rest, 443u16)
    } else {
        return BaseUrlKind::Undetermined("not an http(s) URL".into());
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
    if authority.contains('@') {
        return BaseUrlKind::Undetermined("base URL carries credentials (userinfo)".into());
    }
    let (host, port_text) = if let Some(bracketed) = authority.strip_prefix('[') {
        let Some((host, after)) = bracketed.split_once(']') else {
            return BaseUrlKind::Undetermined("unterminated IPv6 literal".into());
        };
        match after {
            "" => (host.to_owned(), None),
            _ => match after.strip_prefix(':') {
                Some(port) => (host.to_owned(), Some(port.to_owned())),
                None => return BaseUrlKind::Undetermined("garbage after IPv6 literal".into()),
            },
        }
    } else {
        match authority.rsplit_once(':') {
            Some((host, port)) => (host.to_owned(), Some(port.to_owned())),
            None => (authority.to_owned(), None),
        }
    };
    if host.is_empty() {
        return BaseUrlKind::Undetermined("no host".into());
    }
    if !is_loopback_host(&host) {
        return BaseUrlKind::Remote;
    }
    let port = match port_text.as_deref() {
        None | Some("") => default_port,
        Some(text) => match text.parse::<u16>() {
            Ok(port) if port != 0 && text.bytes().all(|byte| byte.is_ascii_digit()) => port,
            _ => return BaseUrlKind::Undetermined(format!("invalid port {text:?}")),
        },
    };
    BaseUrlKind::Loopback { host: host.trim_end_matches('.').to_owned(), port }
}

/// The srt `allowedDomains` entry for one loopback endpoint.
pub fn allow_entry(host: &str, port: u16) -> String {
    if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

/// Pick the one loopback gateway, or none with diagnostics (see the module
/// docs for the rules).
pub fn derive_loopback_gateway(candidates: &[BaseUrlCandidate]) -> LoopbackDerivation {
    let mut out = LoopbackDerivation::default();
    let mut loopbacks: Vec<LoopbackGateway> = Vec::new();
    let mut undetermined = false;
    for candidate in candidates {
        match classify_base_url(&candidate.value) {
            BaseUrlKind::Remote => {}
            BaseUrlKind::Undetermined(reason) => {
                undetermined = true;
                out.diagnostics.push(format!("{}: 无法确定端口（{reason}），不放行本机回环端口", candidate.source));
            }
            BaseUrlKind::Loopback { host, port } => {
                if !loopbacks.iter().any(|known| known.host == host && known.port == port) {
                    loopbacks.push(LoopbackGateway { entry: allow_entry(&host, port), host, port, source: candidate.source.clone() });
                }
            }
        }
    }
    if undetermined {
        if !loopbacks.is_empty() {
            out.diagnostics.push(format!(
                "另有本机回环地址 {}，但存在无法确定的 base URL，为免放错端口，本次不放行任何本机回环端口",
                loopbacks.iter().map(|gateway| gateway.entry.as_str()).collect::<Vec<_>>().join("、")
            ));
        }
        return out;
    }
    match loopbacks.len() {
        0 => {}
        1 => out.gateway = loopbacks.pop(),
        _ => out.diagnostics.push(format!(
            "base URL 指向多个本机回环端点（{}），无法确定后端实际使用哪一个，不放行本机回环端口",
            loopbacks.iter().map(|gateway| format!("{} 来自 {}", gateway.entry, gateway.source)).collect::<Vec<_>>().join("；")
        )),
    }
    out
}

/// Strip one level of matching quotes and a trailing ` #` comment.
fn scalar(value: &str) -> String {
    let value = value.trim();
    for quote in ['"', '\''] {
        if let Some(inner) = value.strip_prefix(quote) {
            if let Some(end) = inner.find(quote) {
                return inner[..end].to_owned();
            }
        }
    }
    let value = match value.find(" #") {
        Some(index) => &value[..index],
        None => value,
    };
    value.trim().to_owned()
}

/// `baseURL:` / `baseUrl:` keys of a YAML config text (any nesting). Values
/// that are not plain scalars come back as written, so they classify as
/// undetermined.
pub fn yaml_base_urls(text: &str, source: &str) -> Vec<BaseUrlCandidate> {
    text.lines()
        .filter_map(|line| {
            let trimmed = line.trim_start().trim_start_matches("- ").trim_start();
            if trimmed.starts_with('#') {
                return None;
            }
            let (key, value) = trimmed.split_once(':')?;
            let key = key.trim().trim_matches(['"', '\'']);
            matches!(key, "baseURL" | "baseUrl" | "base_url").then(|| BaseUrlCandidate { source: format!("{source} {key}"), value: scalar(value) })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(value: &str) -> BaseUrlCandidate {
        BaseUrlCandidate { source: "test".into(), value: value.into() }
    }

    #[test]
    fn loopback_base_urls_give_their_port() {
        assert_eq!(classify_base_url("http://127.0.0.1:3425/v1"), BaseUrlKind::Loopback { host: "127.0.0.1".into(), port: 3425 });
        assert_eq!(classify_base_url("HTTP://LOCALHOST:8080"), BaseUrlKind::Loopback { host: "localhost".into(), port: 8080 });
        assert_eq!(classify_base_url("http://[::1]:9000/anthropic"), BaseUrlKind::Loopback { host: "::1".into(), port: 9000 });
        assert_eq!(classify_base_url("http://127.0.0.2"), BaseUrlKind::Loopback { host: "127.0.0.2".into(), port: 80 });
        assert_eq!(classify_base_url("https://gw.localhost/v1"), BaseUrlKind::Loopback { host: "gw.localhost".into(), port: 443 });
    }

    #[test]
    fn remote_and_undetermined_base_urls() {
        assert_eq!(classify_base_url("https://api.deepseek.com/anthropic"), BaseUrlKind::Remote);
        assert_eq!(classify_base_url("http://10.0.0.1:3425"), BaseUrlKind::Remote);
        assert_eq!(classify_base_url("http://127.999.0.1:3425"), BaseUrlKind::Remote, "not an IPv4 loopback");
        for value in ["", "!!js process.env.X", "http://127.0.0.1:${PORT}", "$GATEWAY", "ws://127.0.0.1:1", "http://127.0.0.1:99999", "http://127.0.0.1:0", "http://u:p@127.0.0.1:1", "http://[::1"] {
            assert!(matches!(classify_base_url(value), BaseUrlKind::Undetermined(_)), "{value}");
        }
    }

    #[test]
    fn exactly_one_loopback_port_is_allowed() {
        let derived = derive_loopback_gateway(&[candidate("https://api.deepseek.com"), candidate("http://127.0.0.1:3425/v1"), candidate("http://127.0.0.1:3425/anthropic")]);
        let gateway = derived.gateway.expect("one gateway");
        assert_eq!((gateway.port, gateway.entry.as_str()), (3425, "127.0.0.1:3425"));
        assert!(derived.diagnostics.is_empty());
        assert_eq!(allow_entry("::1", 1), "[::1]:1");
    }

    #[test]
    fn ambiguous_or_undetermined_allows_nothing_with_a_diagnostic() {
        let none = derive_loopback_gateway(&[candidate("https://api.deepseek.com")]);
        assert_eq!(none, LoopbackDerivation::default(), "remote only: nothing, no noise");
        let two = derive_loopback_gateway(&[candidate("http://127.0.0.1:3425"), candidate("http://127.0.0.1:4000")]);
        assert!(two.gateway.is_none());
        assert!(two.diagnostics[0].contains("127.0.0.1:3425") && two.diagnostics[0].contains("127.0.0.1:4000"), "{:?}", two.diagnostics);
        let unknown = derive_loopback_gateway(&[candidate("http://127.0.0.1:3425"), candidate("!!js process.env.GW")]);
        assert!(unknown.gateway.is_none(), "fail closed when any base URL is undetermined");
        assert!(unknown.diagnostics.iter().any(|line| line.contains("无法确定端口")), "{:?}", unknown.diagnostics);
    }

    #[test]
    fn yaml_base_urls_are_read() {
        let yaml = "- id: llm-pi-ai\n  config:\n    providers:\n      magpie:\n        baseURL: 'http://127.0.0.1:3425/v1' # gateway\n        # baseURL: http://127.0.0.1:1\n      other:\n        baseURL: !!js process.env.X\n";
        let values = yaml_base_urls(yaml, "patch");
        assert_eq!(values.len(), 2, "{values:?}");
        assert_eq!(values[0].value, "http://127.0.0.1:3425/v1");
        assert!(matches!(classify_base_url(&values[1].value), BaseUrlKind::Undetermined(_)));
    }
}
