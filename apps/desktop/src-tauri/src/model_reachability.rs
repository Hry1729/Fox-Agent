//! 13.38 B4-5：发送前确认模型服务地址连得上。
//!
//! 模型服务地址改成一个连不上的地址（比如 http://127.0.0.1:9/v1）时，以前会先建出 Run，
//! 1 秒内以「处理失败」结束，草稿也丢了。现在在建 Run 之前做一次 TCP 连接预检：
//! 拒绝连接、域名解析失败、连接超时都按发送被拒处理（错误码 `model.connection_unreachable`），
//! 界面保留草稿，弹「连不上模型服务」并带「去设置模型服务」。
//!
//! 只判断「连不连得上」，不发请求、不验证 Key（那是连接测试和 Run 自己的事）。
//! 检测到代理（系统代理或代理环境变量）且目标不是本机/内网地址时跳过预检：
//! 这时模型请求可能经代理出去，直连失败不代表连不上。

use std::net::{IpAddr, SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::time::Duration;

pub const UNREACHABLE_CODE: &str = "model.connection_unreachable";

const RESOLVE_TIMEOUT: Duration = Duration::from_secs(3);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);
const MAX_ADDRESSES: usize = 3;

/// 连不上的原因（用于开发者模式的原文和日志；普通界面只显示一句人话）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Unreachable {
    InvalidAddress,
    DnsFailed,
    Refused,
    TimedOut,
}

impl Unreachable {
    fn sentence(&self) -> &'static str {
        match self {
            Self::InvalidAddress => "模型服务地址不是有效的网址",
            Self::DnsFailed => "找不到这个地址（域名解析失败）",
            Self::Refused => "这个地址拒绝了连接，服务可能没有启动",
            Self::TimedOut => "连接这个地址超时",
        }
    }
}

/// 发送被拒时的说明：一句人话 + 去哪里改。地址本身只出现在开发者模式的原文里（界面层负责）。
pub fn refusal_message(endpoint: &str, reason: &Unreachable) -> String {
    format!(
        "连不上模型服务（{endpoint}）：{}。请到「设置 → 模型服务」检查地址，或确认模型服务已经启动，再发送；草稿已保留。",
        reason.sentence()
    )
}

fn is_local_or_private(host: &str) -> bool {
    if host.eq_ignore_ascii_case("localhost") || host.to_ascii_lowercase().ends_with(".localhost") {
        return true;
    }
    match host.trim_start_matches('[').trim_end_matches(']').parse::<IpAddr>() {
        Ok(IpAddr::V4(address)) => address.is_loopback() || address.is_private() || address.is_link_local() || address.is_unspecified(),
        Ok(IpAddr::V6(address)) => address.is_loopback() || address.is_unspecified() || (address.segments()[0] & 0xfe00) == 0xfc00,
        Err(_) => false,
    }
}

/// 预检的输入：地址和「是否在用代理」。分开是为了测试不依赖本机代理设置。
pub fn check_with(base_url: &str, proxy_in_use: bool) -> Result<(), Unreachable> {
    let url = url::Url::parse(base_url.trim()).map_err(|_| Unreachable::InvalidAddress)?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(Unreachable::InvalidAddress);
    }
    let host = url.host_str().ok_or(Unreachable::InvalidAddress)?.to_owned();
    let port = url.port_or_known_default().ok_or(Unreachable::InvalidAddress)?;
    if proxy_in_use && !is_local_or_private(&host) {
        return Ok(());
    }
    let addresses = resolve(&host, port)?;
    let mut timed_out = false;
    for address in addresses.into_iter().take(MAX_ADDRESSES) {
        match TcpStream::connect_timeout(&address, CONNECT_TIMEOUT) {
            Ok(_) => return Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut || error.kind() == std::io::ErrorKind::WouldBlock => timed_out = true,
            Err(_) => {}
        }
    }
    Err(if timed_out { Unreachable::TimedOut } else { Unreachable::Refused })
}

/// 系统 DNS 没有超时参数，放到线程里等；超时按「解析失败」报（线程自己结束后退出）。
fn resolve(host: &str, port: u16) -> Result<Vec<SocketAddr>, Unreachable> {
    let bare = host.trim_start_matches('[').trim_end_matches(']');
    let target = if bare.contains(':') { format!("[{bare}]:{port}") } else { format!("{bare}:{port}") };
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(target.to_socket_addrs().map(|addresses| addresses.collect::<Vec<_>>()));
    });
    match receiver.recv_timeout(RESOLVE_TIMEOUT) {
        Ok(Ok(addresses)) if !addresses.is_empty() => Ok(addresses),
        Ok(_) => Err(Unreachable::DnsFailed),
        Err(_) => Err(Unreachable::DnsFailed),
    }
}

/// 发送前调用：`Some((code, message))` 表示拒绝发送。
pub fn send_refusal(base_url: &str) -> Option<(&'static str, String)> {
    let proxy_in_use = crate::system_proxy::detect().proxy_in_use();
    match check_with(base_url, proxy_in_use) {
        Ok(()) => None,
        Err(reason) => Some((UNREACHABLE_CODE, refusal_message(base_url.trim(), &reason))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::TcpListener;

    #[test]
    fn a_listening_local_port_is_reachable() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        assert_eq!(check_with(&format!("http://127.0.0.1:{port}/v1"), false), Ok(()));
        assert_eq!(check_with(&format!("http://localhost:{port}/v1"), true), Ok(()), "本机地址即使有代理也照样检查");
    }

    /// 实测 B4-5：http://127.0.0.1:9/v1（端口没有服务）在建 Run 之前就被拒。
    #[test]
    fn a_closed_local_port_is_refused_before_any_run() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        drop(listener);
        let reason = check_with(&format!("http://127.0.0.1:{port}/v1"), true).unwrap_err();
        assert!(matches!(reason, Unreachable::Refused | Unreachable::TimedOut), "{reason:?}");
        let (code, message) = (UNREACHABLE_CODE, refusal_message("http://127.0.0.1:9/v1", &reason));
        assert_eq!(code, "model.connection_unreachable");
        assert!(message.starts_with("连不上模型服务") && message.contains("设置 → 模型服务"), "{message}");
    }

    #[test]
    fn unresolvable_hosts_and_bad_addresses() {
        assert_eq!(check_with("https://fox-unreachable.invalid/v1", false), Err(Unreachable::DnsFailed));
        assert_eq!(check_with("not a url", false), Err(Unreachable::InvalidAddress));
        assert_eq!(check_with("ftp://example.com", false), Err(Unreachable::InvalidAddress));
    }

    #[test]
    fn remote_hosts_are_not_probed_through_a_proxy() {
        // 代理环境下远程地址直接放行（不做 DNS，也不连接）。
        assert_eq!(check_with("https://fox-unreachable.invalid/v1", true), Ok(()));
        assert!(is_local_or_private("127.0.0.1") && is_local_or_private("192.168.1.2") && is_local_or_private("[::1]") && is_local_or_private("localhost"));
        assert!(!is_local_or_private("api.deepseek.com") && !is_local_or_private("8.8.8.8"));
    }
}
