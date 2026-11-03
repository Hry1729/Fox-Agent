//! 13.38（R1-3 / B4-2）：run.failed 投影被拒、随后 Pi 释放 Run 时，终态不再被写成
//! `runtime.released_without_terminal`。

use super::{released_without_terminal_outcome, rejected_terminal_outcome};
use serde_json::json;

#[test]
fn rejected_run_failed_keeps_its_own_code_and_message() {
    let payload = json!({ "type": "run.failed", "code": "provider.request_failed", "message": "socket hang up" });
    assert_eq!(rejected_terminal_outcome(&payload), Some(("provider.request_failed".to_owned(), "socket hang up".to_owned())));
    let interrupted = json!({ "type": "run.interrupted", "code": "runtime.crashed" });
    assert_eq!(rejected_terminal_outcome(&interrupted), Some(("runtime.crashed".to_owned(), "runtime.crashed".to_owned())));
    assert_eq!(rejected_terminal_outcome(&json!({ "type": "message.delta", "delta": "x" })), None, "不是终态事件");
    assert_eq!(rejected_terminal_outcome(&json!({ "type": "run.failed", "code": " " })), None, "没有 code 时沿用投影错误");
}

#[test]
fn released_run_after_provider_trouble_is_a_retryable_provider_failure() {
    let (code, message, failed) = released_without_terminal_outcome(true);
    assert_eq!(code, "provider.request_failed");
    assert_eq!(message, "模型服务连接中断，可重试。");
    assert!(failed);
    let (code, _, failed) = released_without_terminal_outcome(false);
    assert_eq!(code, "runtime.released_without_terminal");
    assert!(!failed);
}
