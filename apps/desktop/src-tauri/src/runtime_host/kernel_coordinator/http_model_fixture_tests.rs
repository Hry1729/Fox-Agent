use super::*;
use serde_json::json;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpStream};
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

fn send_request(address: SocketAddr, body: &serde_json::Value) -> String {
    let payload = body.to_string();
    let mut stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    write!(
        stream,
        "POST /v1/chat/completions HTTP/1.1\r\nHost: localhost\r\nContent-Length: {}\r\n\r\n{}",
        payload.len(),
        payload
    )
    .unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    response
}

fn response_hook_count() -> (Arc<AtomicUsize>, Option<(usize, Box<dyn FnOnce() + Send>)>) {
    let count = Arc::new(AtomicUsize::new(0));
    let hook_count = count.clone();
    let hook = Some((
        0,
        Box::new(move || {
            hook_count.fetch_add(1, Ordering::SeqCst);
        }) as Box<dyn FnOnce() + Send>,
    ));
    (count, hook)
}

#[test]
fn empty_disconnected_probe_does_not_consume_the_scripted_reply() {
    let (hook_count, hook) = response_hook_count();
    let (address, server) = start_http_model_fixture_with_response_hook(
        vec![json!({"role":"assistant","content":"scripted first response"})],
        hook,
    );

    let mut probe = TcpStream::connect(address).unwrap();
    probe.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
    probe.shutdown(Shutdown::Write).unwrap();
    let mut buffer = [0u8; 1];
    match probe.read(&mut buffer) {
        Ok(0) => {}
        Err(error) if matches!(error.kind(),
            std::io::ErrorKind::ConnectionReset | std::io::ErrorKind::ConnectionAborted) => {}
        other => panic!("empty probe was not closed by the fixture: {other:?}"),
    }
    assert_eq!(hook_count.load(Ordering::SeqCst), 0);

    let request = json!({"probe":"the only model request"});
    let response = send_request(address, &request);
    assert!(response.contains("scripted first response"));
    assert_eq!(server.join().unwrap(), vec![request]);
    assert_eq!(hook_count.load(Ordering::SeqCst), 1);
}

fn assert_partial_request_is_not_ignored(bytes: &[u8], expected_phase: &str) {
    let (hook_count, hook) = response_hook_count();
    let (address, server) = start_http_model_fixture_with_response_hook(
        vec![json!({"role":"assistant","content":"must not be sent"})],
        hook,
    );
    let mut client = TcpStream::connect(address).unwrap();
    client.write_all(bytes).unwrap();
    client.shutdown(Shutdown::Write).unwrap();

    let payload = server.join().expect_err("a partial request must fail the fixture");
    let message = payload
        .downcast_ref::<String>()
        .cloned()
        .or_else(|| payload.downcast_ref::<&str>().map(|text| (*text).to_owned()))
        .expect("fixture panic text");
    assert!(message.contains(expected_phase), "{message}");
    assert!(message.contains(&format!("after {} bytes", bytes.len())), "{message}");
    assert_eq!(hook_count.load(Ordering::SeqCst), 0);
}

#[test]
fn partial_header_disconnect_fails_without_consuming_a_reply() {
    assert_partial_request_is_not_ignored(
        b"POST /v1/chat/completions HTTP/1.1\r\nContent-Length: 2\r\n",
        "model request ended",
    );
}

#[test]
fn partial_body_disconnect_fails_without_consuming_a_reply() {
    let body = b"{\"x\":1}";
    let header = format!(
        "POST /v1/chat/completions HTTP/1.1\r\nContent-Length: {}\r\n\r\n",
        body.len()
    );
    let mut partial = header.into_bytes();
    partial.extend_from_slice(&body[..body.len() - 1]);
    assert_partial_request_is_not_ignored(&partial, "model request body ended");
}

#[test]
fn only_zero_byte_eof_or_reset_is_an_ignorable_probe() {
    for kind in [
        std::io::ErrorKind::ConnectionReset,
        std::io::ErrorKind::ConnectionAborted,
    ] {
        let reset = Err(std::io::Error::from(kind));
        assert!(is_empty_http_model_probe_disconnect(&reset, 0));
        assert!(!is_empty_http_model_probe_disconnect(&reset, 1));
    }
    assert!(is_empty_http_model_probe_disconnect(&Ok(0), 0));
    assert!(!is_empty_http_model_probe_disconnect(&Ok(0), 1));
    assert!(!is_empty_http_model_probe_disconnect(
        &Err(std::io::Error::from(std::io::ErrorKind::TimedOut)),
        0
    ));
    assert!(!is_empty_http_model_probe_disconnect(&Ok(1), 0));
}
