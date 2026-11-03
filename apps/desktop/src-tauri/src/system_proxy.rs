//! 13.38 B5-2 / B5-3：系统代理设置。
//!
//! 来源按优先级：
//! 1. Fox 进程自己的环境变量（HTTP_PROXY / HTTPS_PROXY / NO_PROXY / ALL_PROXY，大小写都认）；
//! 2. Windows 用户级环境变量（HKCU\Environment，Fox 启动后才设置的也能读到）；
//! 3. Windows 系统代理（WinINET：HKCU\…\Internet Settings 的 ProxyEnable / ProxyServer / ProxyOverride），
//!    转换成同名环境变量。
//!
//! 同一个变量名，先找到的来源生效；后面的来源只补缺。
//! 用途：
//! * 「自动」「完全访问」档位下把代理变量带进 Fox 执行的命令（`command_environment`）；
//! * web_read / http_request 判断「是否在用代理」，以便放行代理 fake-ip 段（`proxy_in_use`）。

use std::collections::BTreeMap;

/// 四个代理变量的规范名（大写）。
pub const PROXY_VARIABLES: [&str; 4] = ["HTTP_PROXY", "HTTPS_PROXY", "NO_PROXY", "ALL_PROXY"];

/// 某个变量的值来自哪里，写进审计时用。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProxySource {
    ProcessEnvironment,
    UserEnvironment,
    SystemProxy,
}

impl ProxySource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ProcessEnvironment => "process_env",
            Self::UserEnvironment => "user_env",
            Self::SystemProxy => "wininet",
        }
    }
}

/// 汇总后的代理设置：规范名 → (值, 来源)。
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ProxySettings {
    values: BTreeMap<&'static str, (String, ProxySource)>,
}

impl ProxySettings {
    /// 按来源顺序合并：已经有值的变量不被后面的来源覆盖。
    pub fn merge(sources: &[(ProxySource, Vec<(String, String)>)]) -> Self {
        let mut values = BTreeMap::new();
        for (source, entries) in sources {
            for (name, value) in entries {
                let value = value.trim();
                if value.is_empty() {
                    continue;
                }
                let Some(canonical) = PROXY_VARIABLES.iter().copied().find(|known| known.eq_ignore_ascii_case(name.trim())) else {
                    continue;
                };
                values.entry(canonical).or_insert_with(|| (value.to_owned(), *source));
            }
        }
        Self { values }
    }

    pub fn get(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(key, _)| key.eq_ignore_ascii_case(name))
            .map(|(_, (value, _))| value.as_str())
    }

    /// 是否配置了真正的代理（只有 NO_PROXY 不算）。
    pub fn proxy_in_use(&self) -> bool {
        ["HTTP_PROXY", "HTTPS_PROXY", "ALL_PROXY"].iter().any(|name| self.values.contains_key(name))
    }

    /// 生效的来源（去重，按优先级），审计里写「来自哪里」。
    pub fn sources(&self) -> Vec<&'static str> {
        let mut seen: Vec<&'static str> = Vec::new();
        for source in [ProxySource::ProcessEnvironment, ProxySource::UserEnvironment, ProxySource::SystemProxy] {
            if self.values.values().any(|(_, from)| *from == source) && !seen.contains(&source.as_str()) {
                seen.push(source.as_str());
            }
        }
        seen
    }

    /// 带进命令的环境变量：每个变量同时给大写和小写两个名字。
    ///
    /// Windows 的环境变量名不区分大小写（`Command::env` 也按不区分大小写去重），
    /// 子进程里一条就同时能用 `%HTTP_PROXY%` 和 `%http_proxy%` 读到，所以 Windows
    /// 上只给一条；其他平台大小写是两个变量，两条都给（curl 只认小写 http_proxy）。
    pub fn command_environment(&self) -> Vec<(String, String)> {
        let mut entries = Vec::new();
        for (name, (value, _)) in &self.values {
            entries.push(((*name).to_owned(), value.clone()));
            if cfg!(not(windows)) {
                entries.push((name.to_ascii_lowercase(), value.clone()));
            }
        }
        entries
    }
}

/// 读取当前的代理设置（每次调用都重新读，系统代理随时可能被代理软件改掉）。
pub fn detect() -> ProxySettings {
    ProxySettings::merge(&[
        (ProxySource::ProcessEnvironment, process_environment()),
        (ProxySource::UserEnvironment, user_environment()),
        (ProxySource::SystemProxy, system_proxy_environment()),
    ])
}

fn process_environment() -> Vec<(String, String)> {
    std::env::vars_os()
        .filter_map(|(key, value)| Some((key.into_string().ok()?, value.into_string().ok()?)))
        .filter(|(key, _)| PROXY_VARIABLES.iter().any(|known| known.eq_ignore_ascii_case(key)))
        .collect()
}

#[cfg(windows)]
fn user_environment() -> Vec<(String, String)> {
    PROXY_VARIABLES
        .iter()
        .filter_map(|name| registry::read_string(registry::USER_ENVIRONMENT, name).map(|value| ((*name).to_owned(), value)))
        .collect()
}

#[cfg(not(windows))]
fn user_environment() -> Vec<(String, String)> {
    Vec::new()
}

#[cfg(windows)]
fn system_proxy_environment() -> Vec<(String, String)> {
    let enabled = registry::read_dword(registry::INTERNET_SETTINGS, "ProxyEnable").unwrap_or(0) != 0;
    if !enabled {
        return Vec::new();
    }
    let Some(server) = registry::read_string(registry::INTERNET_SETTINGS, "ProxyServer") else {
        return Vec::new();
    };
    let bypass = registry::read_string(registry::INTERNET_SETTINGS, "ProxyOverride").unwrap_or_default();
    wininet_environment(&server, &bypass)
}

#[cfg(not(windows))]
fn system_proxy_environment() -> Vec<(String, String)> {
    Vec::new()
}

fn with_scheme(address: &str, scheme: &str) -> String {
    let address = address.trim();
    if address.contains("://") {
        address.to_owned()
    } else {
        format!("{scheme}://{address}")
    }
}

/// WinINET 的 ProxyServer / ProxyOverride → 环境变量。
///
/// * `host:port`：HTTP 和 HTTPS 都走它；
/// * `http=a:1;https=b:2;socks=c:3`：分别给 HTTP_PROXY / HTTPS_PROXY / ALL_PROXY（socks5）。
///   只有 socks 时 ALL_PROXY 就是唯一的代理；
/// * ProxyOverride：`<local>` → localhost，`*.corp` → `.corp`，`10.*` → `10.0.0.0/8`，
///   其余带通配符的写法环境变量表达不了，跳过。本机地址总是加进 NO_PROXY。
pub fn wininet_environment(server: &str, bypass: &str) -> Vec<(String, String)> {
    let server = server.trim();
    if server.is_empty() {
        return Vec::new();
    }
    let mut entries = Vec::new();
    if server.contains('=') {
        for part in server.split(';') {
            let Some((protocol, address)) = part.split_once('=') else { continue };
            let address = address.trim();
            if address.is_empty() {
                continue;
            }
            match protocol.trim().to_ascii_lowercase().as_str() {
                "http" => entries.push(("HTTP_PROXY".to_owned(), with_scheme(address, "http"))),
                "https" => entries.push(("HTTPS_PROXY".to_owned(), with_scheme(address, "http"))),
                "socks" | "socks5" => entries.push(("ALL_PROXY".to_owned(), with_scheme(address, "socks5"))),
                _ => {}
            }
        }
    } else {
        let address = server.split(';').next().unwrap_or_default().trim();
        if !address.is_empty() {
            entries.push(("HTTP_PROXY".to_owned(), with_scheme(address, "http")));
            entries.push(("HTTPS_PROXY".to_owned(), with_scheme(address, "http")));
        }
    }
    if entries.is_empty() {
        return entries;
    }
    let mut no_proxy: Vec<String> = vec!["localhost".into(), "127.0.0.1".into(), "::1".into()];
    for item in bypass.split(';').map(str::trim).filter(|item| !item.is_empty()) {
        let converted = if item.eq_ignore_ascii_case("<local>") {
            None
        } else if let Some(suffix) = item.strip_prefix("*.") {
            (!suffix.contains('*')).then(|| format!(".{suffix}"))
        } else if item.ends_with(".*") {
            wildcard_ipv4_cidr(item)
        } else if item.contains('*') {
            None
        } else {
            Some(item.to_owned())
        };
        if let Some(value) = converted {
            if !no_proxy.iter().any(|known| known.eq_ignore_ascii_case(&value)) {
                no_proxy.push(value);
            }
        }
    }
    entries.push(("NO_PROXY".to_owned(), no_proxy.join(",")));
    entries
}

/// `10.*` → `10.0.0.0/8`，`192.168.*` → `192.168.0.0/16`，`172.16.1.*` → `172.16.1.0/24`。
fn wildcard_ipv4_cidr(pattern: &str) -> Option<String> {
    let prefix = pattern.strip_suffix(".*")?;
    let octets = prefix.split('.').map(|part| part.parse::<u8>().ok()).collect::<Option<Vec<_>>>()?;
    if octets.is_empty() || octets.len() > 3 {
        return None;
    }
    let mut full = octets.clone();
    full.resize(4, 0);
    Some(format!("{}.{}.{}.{}/{}", full[0], full[1], full[2], full[3], octets.len() * 8))
}

#[cfg(windows)]
mod registry {
    use windows::core::PCWSTR;
    use windows::Win32::Foundation::ERROR_SUCCESS;
    use windows::Win32::System::Registry::{RegGetValueW, HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RRF_RT_REG_EXPAND_SZ, RRF_RT_REG_SZ};

    pub const USER_ENVIRONMENT: &str = "Environment";
    pub const INTERNET_SETTINGS: &str = r"Software\Microsoft\Windows\CurrentVersion\Internet Settings";

    fn wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }

    /// REG_SZ / REG_EXPAND_SZ（展开后）。读不到返回 None。
    pub fn read_string(subkey: &str, value: &str) -> Option<String> {
        let subkey = wide(subkey);
        let value = wide(value);
        let flags = RRF_RT_REG_SZ | RRF_RT_REG_EXPAND_SZ;
        let mut size: u32 = 0;
        // 先问长度，再读。
        let status = unsafe {
            RegGetValueW(HKEY_CURRENT_USER, PCWSTR(subkey.as_ptr()), PCWSTR(value.as_ptr()), flags, None, None, Some(&mut size))
        };
        if status != ERROR_SUCCESS || size == 0 {
            return None;
        }
        let mut buffer = vec![0u16; (size as usize).div_ceil(2) + 1];
        let mut bytes = (buffer.len() * 2) as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                PCWSTR(value.as_ptr()),
                flags,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut bytes),
            )
        };
        if status != ERROR_SUCCESS {
            return None;
        }
        let units = (bytes as usize / 2).min(buffer.len());
        let text = String::from_utf16_lossy(&buffer[..units]);
        let text = text.trim_end_matches('\0').trim().to_owned();
        (!text.is_empty()).then_some(text)
    }

    pub fn read_dword(subkey: &str, value: &str) -> Option<u32> {
        let subkey = wide(subkey);
        let value = wide(value);
        let mut data: u32 = 0;
        let mut size = std::mem::size_of::<u32>() as u32;
        let status = unsafe {
            RegGetValueW(
                HKEY_CURRENT_USER,
                PCWSTR(subkey.as_ptr()),
                PCWSTR(value.as_ptr()),
                RRF_RT_REG_DWORD,
                None,
                Some((&mut data as *mut u32).cast()),
                Some(&mut size),
            )
        };
        (status == ERROR_SUCCESS).then_some(data)
    }
}

/// 13.38 B5-2: the proxy entries a command gets in an access mode. 自动 and
/// 完全访问 get the detected proxy; 只读 and 询问 keep the plain allowlisted
/// environment (and do not even detect).
pub(crate) fn command_environment_for_mode(
    mode: crate::runtime_host::approval_policy::AccessMode,
    settings: impl FnOnce() -> ProxySettings,
) -> Vec<(String, String)> {
    use crate::runtime_host::approval_policy::AccessMode;
    match mode {
        AccessMode::Auto | AccessMode::FullAccess => settings().command_environment(),
        AccessMode::ReadOnly | AccessMode::Ask => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pairs(entries: &[(&str, &str)]) -> Vec<(String, String)> {
        entries.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())).collect()
    }

    #[test]
    fn earlier_sources_win_and_names_are_case_insensitive() {
        let settings = ProxySettings::merge(&[
            (ProxySource::ProcessEnvironment, pairs(&[("https_proxy", "http://127.0.0.1:1"), ("PATH", "x")])),
            (ProxySource::UserEnvironment, pairs(&[("HTTPS_PROXY", "http://127.0.0.1:2"), ("HTTP_PROXY", "http://127.0.0.1:2")])),
            (ProxySource::SystemProxy, pairs(&[("HTTP_PROXY", "http://127.0.0.1:3"), ("NO_PROXY", "localhost")])),
        ]);
        assert_eq!(settings.get("HTTPS_PROXY"), Some("http://127.0.0.1:1"), "进程环境优先");
        assert_eq!(settings.get("http_proxy"), Some("http://127.0.0.1:2"), "用户级环境补缺");
        assert_eq!(settings.get("NO_PROXY"), Some("localhost"), "系统代理再补缺");
        assert!(settings.get("PATH").is_none(), "只认四个代理变量");
        assert!(settings.proxy_in_use());
        assert_eq!(settings.sources(), vec!["process_env", "user_env", "wininet"]);
    }

    #[test]
    fn no_proxy_alone_is_not_a_proxy_and_blank_values_are_ignored() {
        let settings = ProxySettings::merge(&[(ProxySource::ProcessEnvironment, pairs(&[("NO_PROXY", "localhost"), ("HTTP_PROXY", "  ")]))]);
        assert!(!settings.proxy_in_use());
        assert!(settings.get("HTTP_PROXY").is_none());
        assert!(!ProxySettings::default().proxy_in_use());
    }

    #[test]
    fn command_environment_passes_both_cases() {
        let settings = ProxySettings::merge(&[(ProxySource::SystemProxy, pairs(&[("HTTP_PROXY", "http://127.0.0.1:7890"), ("NO_PROXY", "localhost")]))]);
        let env = settings.command_environment();
        assert!(env.contains(&("HTTP_PROXY".to_owned(), "http://127.0.0.1:7890".to_owned())));
        assert!(env.contains(&("NO_PROXY".to_owned(), "localhost".to_owned())));
        if cfg!(windows) {
            // 不区分大小写：一条就够，子进程里 %http_proxy% 同样能读到（见 tool_host 的 Windows 测试）。
            assert_eq!(env.len(), 2);
        } else {
            assert!(env.contains(&("http_proxy".to_owned(), "http://127.0.0.1:7890".to_owned())));
            assert!(env.contains(&("no_proxy".to_owned(), "localhost".to_owned())));
            assert_eq!(env.len(), 4);
        }
    }

    #[test]
    fn wininet_single_server_covers_http_and_https() {
        let env = wininet_environment("127.0.0.1:7890", "localhost;127.*;10.*;192.168.*;*.corp.example;<local>;intranet;a*b");
        let get = |name: &str| env.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
        assert_eq!(get("HTTP_PROXY").as_deref(), Some("http://127.0.0.1:7890"));
        assert_eq!(get("HTTPS_PROXY").as_deref(), Some("http://127.0.0.1:7890"));
        assert!(get("ALL_PROXY").is_none());
        assert_eq!(
            get("NO_PROXY").as_deref(),
            Some("localhost,127.0.0.1,::1,127.0.0.0/8,10.0.0.0/8,192.168.0.0/16,.corp.example,intranet")
        );
    }

    #[test]
    fn wininet_per_protocol_and_socks() {
        let env = wininet_environment("http=127.0.0.1:8080;https=127.0.0.1:8443;socks=127.0.0.1:1080", "");
        let get = |name: &str| env.iter().find(|(key, _)| key == name).map(|(_, value)| value.clone());
        assert_eq!(get("HTTP_PROXY").as_deref(), Some("http://127.0.0.1:8080"));
        assert_eq!(get("HTTPS_PROXY").as_deref(), Some("http://127.0.0.1:8443"));
        assert_eq!(get("ALL_PROXY").as_deref(), Some("socks5://127.0.0.1:1080"));
        let socks_only = wininet_environment("socks=127.0.0.1:1080", "");
        assert_eq!(socks_only.iter().filter(|(key, _)| key.ends_with("_PROXY") && key != "NO_PROXY").count(), 1);
        assert!(wininet_environment("", "localhost").is_empty(), "没有代理地址时什么都不给");
        assert_eq!(wininet_environment("http://proxy.corp:3128", "")[0].1, "http://proxy.corp:3128", "已带协议的保持原样");
    }

    /// 13.38 B5-2：只有「自动」和「完全访问」把代理变量带进命令；「只读」「询问」不带，也不去检测。
    #[test]
    fn proxy_reaches_commands_only_in_auto_and_full_access() {
        use crate::runtime_host::approval_policy::AccessMode;
        let settings = || ProxySettings::merge(&[(ProxySource::SystemProxy, pairs(&[("HTTPS_PROXY", "http://127.0.0.1:7890")]))]);
        for mode in [AccessMode::Auto, AccessMode::FullAccess] {
            let entries = command_environment_for_mode(mode, settings);
            assert!(entries.iter().any(|(name, value)| name == "HTTPS_PROXY" && value == "http://127.0.0.1:7890"), "{entries:?}");
        }
        for mode in [AccessMode::ReadOnly, AccessMode::Ask] {
            let entries = command_environment_for_mode(mode, || -> ProxySettings { panic!("只读 / 询问不检测代理") });
            assert!(entries.is_empty());
        }
    }
}
