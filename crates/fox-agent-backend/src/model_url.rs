//! 模型服务地址 → 实际请求端点 / 交给 SDK 的根地址。
//!
//! 用户在「模型服务」里填的地址可能是基础地址（`https://host/v1`、
//! `https://api.deepseek.com/anthropic`），也可能直接是完整端点
//! （`https://token.sensenova.cn/v1/messages`、`https://host/v1/chat/completions`）。
//! 以前 Fox 一律在后面再拼一段路径，完整端点就会变成
//! `/v1/messages/v1/messages` 而 404。
//!
//! 规则（所有消费方共用，连接测试与运行时一致）：
//! - Anthropic Messages：路径以 `/messages`（含 `/v1/messages`）结尾即视为完整端点，
//!   原样请求；以 `/v1` 结尾补 `/messages`；否则补 `/v1/messages`。
//! - OpenAI Chat Completions：以 `/chat/completions` 结尾即完整端点；否则补
//!   `/chat/completions`。模型列表用根地址 + `/models`。
//! - OpenAI Responses：以 `/responses` 结尾即完整端点；否则补 `/responses`。
//!
//! 运行时（pi-ai / Anthropic、OpenAI SDK、dsh、Codex）只接受「根地址」并自己拼路径，
//! 所以另给出 [`sdk_base_url`]：从端点里去掉 SDK 会再拼的那一段。端点无法表示成
//! 「根地址 + SDK 固定路径」时（例如以 `/messages` 结尾但不是 `/v1/messages`）
//! 直接报错，不猜。

use url::Url;

pub const ANTHROPIC: &str = "anthropic-messages";
pub const OPENAI_COMPLETIONS: &str = "openai-completions";
pub const OPENAI_RESPONSES: &str = "openai-responses";

/// 错误码前缀：地址是完整端点，但运行时 SDK 无法按它请求。
pub const ENDPOINT_NOT_SDK_COMPATIBLE: &str = "model.endpoint_not_sdk_compatible";

fn parse(base: &str) -> Result<Url, String> {
    Url::parse(base.trim()).map_err(|_| "模型 API 地址格式无效".to_owned())
}

fn trimmed_path(url: &Url) -> String {
    url.path().trim_end_matches('/').to_owned()
}

fn ends_with_segments(path: &str, suffix: &str) -> bool {
    // 按路径段比较，大小写不敏感：`/foo/v1/messages` 以 `/messages` 结尾，
    // `/foo/v1/mymessages` 不算。
    // suffix 自带开头的 `/`，所以只要大小写不敏感地以它结尾就落在段边界上。
    path.len() >= suffix.len() && path.to_ascii_lowercase().ends_with(suffix)
}

fn with_path(mut url: Url, path: &str) -> String {
    url.set_path(if path.is_empty() { "/" } else { path });
    url.set_query(None);
    url.set_fragment(None);
    let text = url.to_string();
    if path.is_empty() { text.trim_end_matches('/').to_owned() } else { text }
}

fn strip_suffix_ci<'a>(path: &'a str, suffix: &str) -> Option<&'a str> {
    ends_with_segments(path, suffix).then(|| &path[..path.len() - suffix.len()])
}

/// Anthropic Messages 的实际请求地址。
pub fn anthropic_messages_url(base: &str) -> Result<String, String> {
    let url = parse(base)?;
    let path = trimmed_path(&url);
    let endpoint = if ends_with_segments(&path, "/messages") {
        path
    } else if ends_with_segments(&path, "/v1") {
        format!("{path}/messages")
    } else {
        format!("{path}/v1/messages")
    };
    Ok(with_path(url, &endpoint))
}

/// OpenAI Chat Completions 的实际请求地址。
pub fn openai_chat_url(base: &str) -> Result<String, String> {
    let url = parse(base)?;
    let path = trimmed_path(&url);
    let endpoint = if ends_with_segments(&path, "/chat/completions") { path } else { format!("{path}/chat/completions") };
    Ok(with_path(url, &endpoint))
}

/// OpenAI Responses 的实际请求地址。
pub fn openai_responses_url(base: &str) -> Result<String, String> {
    let url = parse(base)?;
    let path = trimmed_path(&url);
    let endpoint = if ends_with_segments(&path, "/responses") { path } else { format!("{path}/responses") };
    Ok(with_path(url, &endpoint))
}

/// OpenAI 兼容模型列表地址：根地址 + `/models`（填了 `/chat/completions` 端点时先去掉它）。
pub fn openai_models_url(base: &str) -> Result<String, String> {
    let root = sdk_base_url(OPENAI_COMPLETIONS, base)?;
    let url = parse(&root)?;
    let path = trimmed_path(&url);
    Ok(with_path(url, &format!("{path}/models")))
}

/// 交给运行时 SDK 的根地址：SDK 在其后拼固定路径（Anthropic：`/v1/messages`；
/// OpenAI Chat：`/chat/completions`；Responses：`/responses`），拼出来恰好等于
/// 上面各 `*_url` 函数给出的端点。
///
/// dsh 的 `messagesApiRoot` 在根地址不以 `/v1` 结尾时补 `/v1` 再拼 `/messages`，
/// 与 Anthropic SDK 的结果一致，因此 dsh 也用这个根地址。
pub fn sdk_base_url(api_type: &str, base: &str) -> Result<String, String> {
    match api_type {
        ANTHROPIC => {
            let endpoint = parse(&anthropic_messages_url(base)?)?;
            let path = trimmed_path(&endpoint);
            match strip_suffix_ci(&path, "/v1/messages") {
                Some(root) => {
                    let root = root.to_owned();
                    Ok(with_path(endpoint, &root))
                }
                None => Err(format!(
                    "{ENDPOINT_NOT_SDK_COMPATIBLE}: 这个 Anthropic 地址以 /messages 结尾但不是 /v1/messages，Agent 运行时只能按「根地址 + /v1/messages」请求。请改填服务的根地址或 …/v1/messages 完整端点"
                )),
            }
        }
        OPENAI_COMPLETIONS => {
            let endpoint = parse(&openai_chat_url(base)?)?;
            let path = trimmed_path(&endpoint);
            let root = strip_suffix_ci(&path, "/chat/completions").expect("chat endpoint ends with /chat/completions").to_owned();
            Ok(with_path(endpoint, &root))
        }
        OPENAI_RESPONSES => {
            let endpoint = parse(&openai_responses_url(base)?)?;
            let path = trimmed_path(&endpoint);
            let root = strip_suffix_ci(&path, "/responses").expect("responses endpoint ends with /responses").to_owned();
            Ok(with_path(endpoint, &root))
        }
        // 其他协议（如测试用的 faux）不经 SDK 拼路径，原样交出。
        _ => Ok(base.trim().trim_end_matches('/').to_owned()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anthropic_full_endpoint_is_not_appended_again() {
        assert_eq!(anthropic_messages_url("https://token.sensenova.cn/v1/messages").unwrap(), "https://token.sensenova.cn/v1/messages");
        assert_eq!(anthropic_messages_url("https://token.sensenova.cn/v1/messages/").unwrap(), "https://token.sensenova.cn/v1/messages");
        assert_eq!(anthropic_messages_url("https://gw.example.com/anthropic/messages").unwrap(), "https://gw.example.com/anthropic/messages");
        assert_eq!(anthropic_messages_url("https://api.example.com/v1").unwrap(), "https://api.example.com/v1/messages");
        assert_eq!(anthropic_messages_url("https://api.minimaxi.com/anthropic").unwrap(), "https://api.minimaxi.com/anthropic/v1/messages");
        assert_eq!(anthropic_messages_url("http://127.0.0.1:3425").unwrap(), "http://127.0.0.1:3425/v1/messages");
        // 路径段边界：`mymessages` 不是 `/messages`。
        assert_eq!(anthropic_messages_url("https://x.example.com/mymessages").unwrap(), "https://x.example.com/mymessages/v1/messages");
    }

    #[test]
    fn anthropic_sdk_root_reproduces_the_endpoint() {
        for (input, root) in [
            ("https://token.sensenova.cn/v1/messages", "https://token.sensenova.cn"),
            ("https://token.sensenova.cn/v1/messages/", "https://token.sensenova.cn"),
            ("https://token.sensenova.cn", "https://token.sensenova.cn"),
            ("https://api.example.com/v1", "https://api.example.com"),
            ("https://api.deepseek.com/anthropic", "https://api.deepseek.com/anthropic"),
            ("https://gw.example.com/proxy/v1/messages", "https://gw.example.com/proxy"),
            ("http://127.0.0.1:3425/v1", "http://127.0.0.1:3425"),
        ] {
            let got = sdk_base_url(ANTHROPIC, input).unwrap();
            assert_eq!(got, root, "{input}");
            // Anthropic SDK 拼 /v1/messages 后必须等于连接测试请求的端点。
            assert_eq!(format!("{got}/v1/messages"), anthropic_messages_url(input).unwrap(), "{input}");
        }
    }

    #[test]
    fn anthropic_messages_endpoint_without_v1_is_refused_for_sdks() {
        let error = sdk_base_url(ANTHROPIC, "https://gw.example.com/anthropic/messages").unwrap_err();
        assert!(error.starts_with(ENDPOINT_NOT_SDK_COMPATIBLE), "{error}");
    }

    #[test]
    fn openai_chat_full_endpoint_is_not_appended_again() {
        assert_eq!(openai_chat_url("https://api.example.com/v1/chat/completions").unwrap(), "https://api.example.com/v1/chat/completions");
        assert_eq!(openai_chat_url("https://api.example.com/v1/chat/completions/").unwrap(), "https://api.example.com/v1/chat/completions");
        assert_eq!(openai_chat_url("https://api.example.com/v1").unwrap(), "https://api.example.com/v1/chat/completions");
        assert_eq!(sdk_base_url(OPENAI_COMPLETIONS, "https://api.example.com/v1/chat/completions").unwrap(), "https://api.example.com/v1");
        assert_eq!(sdk_base_url(OPENAI_COMPLETIONS, "https://api.example.com/v1").unwrap(), "https://api.example.com/v1");
        assert_eq!(sdk_base_url(OPENAI_COMPLETIONS, "https://dashscope.aliyuncs.com/compatible-mode/v1").unwrap(), "https://dashscope.aliyuncs.com/compatible-mode/v1");
    }

    #[test]
    fn openai_models_url_uses_the_root_even_for_a_full_chat_endpoint() {
        assert_eq!(openai_models_url("https://api.example.com/v1/chat/completions").unwrap(), "https://api.example.com/v1/models");
        assert_eq!(openai_models_url("https://api.example.com/v1").unwrap(), "https://api.example.com/v1/models");
        assert_eq!(openai_models_url("http://127.0.0.1:11434/v1/").unwrap(), "http://127.0.0.1:11434/v1/models");
    }

    #[test]
    fn openai_responses_full_endpoint_is_not_appended_again() {
        assert_eq!(openai_responses_url("https://api.openai.com/v1/responses").unwrap(), "https://api.openai.com/v1/responses");
        assert_eq!(openai_responses_url("https://api.openai.com/v1").unwrap(), "https://api.openai.com/v1/responses");
        assert_eq!(sdk_base_url(OPENAI_RESPONSES, "https://api.openai.com/v1/responses").unwrap(), "https://api.openai.com/v1");
        assert_eq!(sdk_base_url(OPENAI_RESPONSES, "https://api.deepseek.com").unwrap(), "https://api.deepseek.com");
    }

    #[test]
    fn unknown_protocols_pass_the_base_through() {
        assert_eq!(sdk_base_url("faux", "https://example.com/v1/").unwrap(), "https://example.com/v1");
    }
}
