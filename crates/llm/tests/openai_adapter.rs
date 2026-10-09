//! 可选适配器的真实 HTTP 测试：仅连回环假服务，无真实 Key，不调用付费 API。
#![cfg(feature = "openai")]
use llm::*;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    io::{BufRead, BufReader, Read, Write},
    net::TcpListener,
    sync::mpsc,
    thread,
    time::Duration,
};

fn input() -> NarrationInput {
    NarrationInput {
        facts: BTreeMap::from([("quality".into(), "质量：90%".into())]),
        interpretation: "仅描述 EEG".into(),
        limitations: vec!["无诊断".into()],
    }
}
fn response(content: Option<&str>, finish: &str, refusal: Option<&str>) -> Value {
    json!({"id": "test", "object": "chat.completion", "created": 1, "model": "test-model",
        "choices": [{"index": 0, "message": {"role": "assistant", "content": content, "refusal": refusal}, "finish_reason": finish}]})
}
// 简单单请求服务器：读取 Content-Length，不假设请求头或 JSON 一次 read 就到齐。
fn server(
    body: String,
    status: &str,
    delay: Duration,
) -> (String, mpsc::Receiver<Value>, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let status = status.to_owned();
    let (tx, rx) = mpsc::channel();
    let handle = thread::spawn(move || {
        let (stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut reader = BufReader::new(stream);
        let mut line = String::new();
        reader.read_line(&mut line).unwrap();
        assert!(line.starts_with("POST /v1/chat/completions "));
        let mut length = None;
        loop {
            line.clear();
            reader.read_line(&mut line).unwrap();
            if line == "\r\n" {
                break;
            }
            if let Some(value) = line.to_ascii_lowercase().strip_prefix("content-length:") {
                length = Some(value.trim().parse::<usize>().unwrap());
            }
        }
        let mut data = vec![0; length.unwrap()];
        reader.read_exact(&mut data).unwrap();
        tx.send(serde_json::from_slice(&data).unwrap()).unwrap();
        thread::sleep(delay);
        let mut stream = reader.into_inner();
        // 超时用例会提前关连接，服务端写失败是预期而不是测试失败。
        let _ = write!(
            stream,
            "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
    });
    (base, rx, handle)
}
fn narrator(base: String, timeout: Duration) -> OpenAiReportNarrator {
    OpenAiReportNarrator::new(
        OpenAiConfig::new("dummy-test-key", "test-model")
            .unwrap()
            .with_api_base(base)
            .unwrap()
            .with_timeout(timeout)
            .unwrap(),
    )
    .unwrap()
}

#[test]
fn sends_structured_schema_and_returns_only_untrusted_candidate() {
    let (base, requests, handle) = server(
        response(Some("{\"candidate\":true}"), "stop", None).to_string(),
        "200 OK",
        Duration::ZERO,
    );
    let provider = narrator(base, Duration::from_secs(5));
    assert_eq!(provider.narrate(&input()).unwrap(), "{\"candidate\":true}");
    let request = requests.recv_timeout(Duration::from_secs(5)).unwrap();
    handle.join().unwrap();
    assert_eq!(request["model"], "test-model");
    assert_eq!(request["store"], false);
    assert_eq!(request["response_format"]["type"], "json_schema");
    assert_eq!(request["response_format"]["json_schema"]["strict"], true);
    assert_eq!(
        request["response_format"]["json_schema"]["schema"],
        narrative_schema()
    );
    assert_eq!(request["messages"][0]["role"], "system");
    let payload: Value =
        serde_json::from_str(request["messages"][1]["content"].as_str().unwrap()).unwrap();
    assert_eq!(payload["data"]["facts"]["quality"], "质量：90%");
    assert!(!request.to_string().contains("dummy-test-key"));
}

#[test]
fn refusal_truncation_empty_and_invalid_responses_are_typed_failures() {
    let cases = [
        (response(None, "stop", Some("refused")), LlmError::Refused),
        (response(Some("{}"), "length", None), LlmError::Incomplete),
        (
            response(Some("{}"), "content_filter", None),
            LlmError::Incomplete,
        ),
        (response(Some(" "), "stop", None), LlmError::EmptyResponse),
        (response(None, "stop", None), LlmError::EmptyResponse),
        (
            json!({"id":"test", "object":"chat.completion", "created":1, "model":"test-model", "choices":[]}),
            LlmError::EmptyResponse,
        ),
    ];
    for (body, expected) in cases {
        let (base, requests, handle) = server(body.to_string(), "200 OK", Duration::ZERO);
        assert_eq!(
            narrator(base, Duration::from_secs(5)).narrate(&input()),
            Err(expected)
        );
        requests.recv_timeout(Duration::from_secs(5)).unwrap();
        handle.join().unwrap();
    }
}

#[test]
fn timeout_and_http_errors_do_not_expose_server_text_or_credentials() {
    let (base, requests, handle) = server(
        response(Some("{}"), "stop", None).to_string(),
        "200 OK",
        Duration::from_millis(200),
    );
    assert_eq!(
        narrator(base, Duration::from_millis(50)).narrate(&input()),
        Err(LlmError::Timeout)
    );
    requests.recv_timeout(Duration::from_secs(5)).unwrap();
    handle.join().unwrap();
    let (base, requests, handle) = server(
        json!({"error": {"message": "SECRET echoed data", "type": "invalid_api_key"}}).to_string(),
        "401 Unauthorized",
        Duration::ZERO,
    );
    let error = narrator(base, Duration::from_secs(5))
        .narrate(&input())
        .unwrap_err();
    requests.recv_timeout(Duration::from_secs(5)).unwrap();
    handle.join().unwrap();
    assert_eq!(error, LlmError::Request);
    assert!(!format!("{error:?} {error}").contains("SECRET"));
}

#[test]
fn async_host_can_create_await_and_drop_provider_without_nested_runtime() {
    let (base, requests, handle) = server(
        response(Some("{}"), "stop", None).to_string(),
        "200 OK",
        Duration::ZERO,
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let provider = narrator(base, Duration::from_secs(5));
        assert_eq!(provider.narrate(&input()), Err(LlmError::Configuration));
        assert_eq!(provider.narrate_async(&input()).await.unwrap(), "{}");
        // provider 在 Tokio task 作用域内销毁；不能持有同步入口的已启动 Runtime。
    });
    requests.recv_timeout(Duration::from_secs(5)).unwrap();
    handle.join().unwrap();
}

#[test]
fn configuration_is_redacted_and_rejects_missing_secrets_remote_plaintext_or_bad_timeouts() {
    assert!(OpenAiConfig::new("", "model").is_err());
    assert!(OpenAiConfig::new("key", " ").is_err());
    let config = OpenAiConfig::new("super-secret", "model").unwrap();
    assert!(!format!("{config:?}").contains("super-secret"));
    for base in [
        "http://example.com/v1",
        "http://localhost.evil/v1",
        "http://localhost:80@evil/v1",
        "https://",
        "https://user@host/v1",
        "https://host/?key=secret",
    ] {
        assert!(
            OpenAiConfig::new("key", "model")
                .unwrap()
                .with_api_base(base)
                .is_err(),
            "{base}"
        );
    }
    for timeout in [Duration::ZERO, Duration::from_secs(301)] {
        assert!(
            OpenAiConfig::new("key", "model")
                .unwrap()
                .with_timeout(timeout)
                .is_err()
        );
    }
}
