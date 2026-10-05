use crate::{
    config::Config,
    feedback::{Feedback, Severity},
    model::{Command, Status, StreamMessage, safe},
};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::{sync::Arc, time::Duration};
use tokio::{
    sync::mpsc,
    time::{Instant, timeout},
};
use tokio_tungstenite::{
    Connector, connect_async_tls_with_config,
    tungstenite::{Message, client::IntoClientRequest, protocol::WebSocketConfig},
};

#[derive(Clone, Copy, PartialEq)]
pub enum ClipboardTarget {
    Snapshot,
    Logs,
}

pub enum Event {
    Status(Box<Status>),
    Disconnected(String),
    Command(Command),
    Log(Value),
    LogPage(crate::logs::Request, Result<crate::logs::Page, String>),
    Notice(String),
    Finished(Feedback),
    Copied(bool, ClipboardTarget),
    Exit,
}

pub enum Intent {
    Output {
        output: &'static str,
        enabled: bool,
        snapshot: Box<Status>,
        key: String,
    },
    Retry,
    Connection(bool),
    Debug(bool),
}

pub struct Api {
    http: reqwest::Client,
    origin: reqwest::Url,
    token: Option<String>,
    tls: Arc<rustls::ClientConfig>,
    timeout: Duration,
}

impl Api {
    pub fn new(config: &Config) -> Result<Self, String> {
        let _ = rustls::crypto::ring::default_provider().install_default();
        let mut roots = rustls::RootCertStore::empty();
        let certs = if let Some(path) = &config.ca {
            let bytes = std::fs::read(path).map_err(|_| "Cannot read CA file.")?;
            let certs: Vec<_> = rustls_pemfile::certs(&mut bytes.as_slice())
                .collect::<Result<_, _>>()
                .map_err(|_| "Invalid CA bundle.")?;
            if certs.is_empty() {
                return Err("CA bundle contains no certificates.".into());
            }
            certs
        } else {
            rustls_native_certs::load_native_certs().certs
        };
        for cert in certs {
            roots
                .add(cert)
                .map_err(|_| "Invalid trusted certificate.")?;
        }
        let tls = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let mut headers = reqwest::header::HeaderMap::new();
        if let Some(token) = &config.token {
            let mut header = reqwest::header::HeaderValue::from_str(&format!("Bearer {token}"))
                .map_err(|_| "Invalid API token.")?;
            header.set_sensitive(true);
            headers.insert(reqwest::header::AUTHORIZATION, header);
        }
        let http = reqwest::Client::builder()
            .use_preconfigured_tls(tls.clone())
            .default_headers(headers)
            .no_proxy()
            .redirect(reqwest::redirect::Policy::none())
            .retry(reqwest::retry::never())
            .timeout(config.timeout)
            .build()
            .map_err(|_| "Cannot initialize HTTPS client.")?;
        Ok(Self {
            http,
            origin: config.server.clone(),
            token: config.token.clone(),
            tls: Arc::new(tls),
            timeout: config.timeout,
        })
    }

    pub async fn request(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        key: Option<&str>,
    ) -> Result<Value, String> {
        let url = self
            .origin
            .join(&format!("/api/v1{path}"))
            .map_err(|_| "Invalid API path.")?;
        let mut request = self.http.request(method, url);
        if let Some(body) = body {
            request = request.json(&body);
        }
        if let Some(key) = key {
            request = request.header("Idempotency-Key", key);
        }
        let mut response = request
            .send()
            .await
            .map_err(|_| "Cannot reach daemon or verify TLS.")?;
        let status = response.status();
        if status.is_redirection() {
            return Err("Daemon redirect refused.".into());
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| "Incomplete API response.")?
        {
            if bytes.len() + chunk.len() > 16 * 1024 * 1024 {
                return Err("API response exceeds client limit.".into());
            }
            bytes.extend_from_slice(&chunk);
        }
        let data: Value =
            serde_json::from_slice(&bytes).map_err(|_| "Daemon returned invalid JSON.")?;
        if !status.is_success() {
            // Show only the API error code; never echo a response body or secret-bearing URL.
            let code = data["error"]["code"].as_str().unwrap_or("http_error");
            return Err(format!("API {}: {}", status.as_u16(), safe(code)));
        }
        if !data.is_object() {
            return Err("Invalid API response object.".into());
        }
        Ok(data)
    }

    async fn command(&self, id: &str) -> Result<Command, String> {
        let value = self
            .request(reqwest::Method::GET, &format!("/commands/{id}"), None, None)
            .await?;
        let command: Command =
            serde_json::from_value(value).map_err(|_| "Invalid command response.")?;
        if !command.valid() || command.command_id != id {
            return Err("Invalid command response.".into());
        }
        Ok(command)
    }

    async fn perform(&self, intent: Intent, events: &mpsc::Sender<Event>) -> Feedback {
        match intent {
            Intent::Output {
                output,
                enabled,
                snapshot,
                key,
            } => {
                let response = self
                    .request(
                        reqwest::Method::PUT,
                        &format!("/outputs/{output}"),
                        Some(json!({
                            "enabled": enabled, "server_instance_id": snapshot.server_instance_id,
                            "expected_outputs_revision": snapshot.controls.outputs_revision,
                        })),
                        Some(&key),
                    )
                    .await;
                let value = match response {
                    Ok(value) => value,
                    Err(error) if error.starts_with("API ") => {
                        return Feedback::request_error(&error);
                    }
                    Err(_) => return outcome_uncertain(),
                };
                let Ok(mut command) = serde_json::from_value::<Command>(value) else {
                    return outcome_uncertain();
                };
                if !command.valid()
                    || command.output != output
                    || command.requested_enabled != enabled
                {
                    return outcome_uncertain();
                }
                let _ = events.send(Event::Command(command.clone())).await;
                let deadline = Instant::now() + Duration::from_secs(25);
                while !command.terminal() {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                    let next = timeout(
                        deadline.saturating_duration_since(Instant::now()),
                        self.command(&command.command_id),
                    )
                    .await;
                    match next {
                        Ok(Ok(next)) => command = next,
                        _ => {
                            return outcome_uncertain();
                        }
                    }
                }
                Feedback::command(&command)
            }
            Intent::Retry => self
                .request(
                    reqwest::Method::POST,
                    "/connection/retry",
                    Some(json!({})),
                    None,
                )
                .await
                .map(|_| Feedback::new("Reconnecting to station", Severity::Info))
                .unwrap_or_else(|error| Feedback::request_error(&error)),
            Intent::Connection(running) => self
                .request(
                    reqwest::Method::PUT,
                    "/connection",
                    Some(json!({"desired": if running { "running" } else { "paused" }})),
                    None,
                )
                .await
                .map(|_| {
                    Feedback::new(
                        if running {
                            "Station connection resumed"
                        } else {
                            "Station connection paused"
                        },
                        Severity::Info,
                    )
                })
                .unwrap_or_else(|error| Feedback::request_error(&error)),
            Intent::Debug(enabled) => self
                .request(
                    if enabled {
                        reqwest::Method::PUT
                    } else {
                        reqwest::Method::DELETE
                    },
                    "/runtime/log-level",
                    enabled.then(|| json!({"level":"DEBUG"})),
                    None,
                )
                .await
                .map(|value| {
                    let level = value["effective_level"]
                        .as_str()
                        .filter(|level| ["DEBUG", "INFO", "WARNING", "ERROR"].contains(level));
                    let message = level
                        .map(|level| format!("Log level changed to {level}"))
                        .unwrap_or_else(|| "Log level updated".into());
                    Feedback::new(message, Severity::Info)
                })
                .unwrap_or_else(|error| Feedback::request_error(&error)),
        }
    }

    pub async fn operations(
        self: Arc<Self>,
        mut requests: mpsc::Receiver<Intent>,
        events: mpsc::Sender<Event>,
    ) {
        while let Some(intent) = requests.recv().await {
            let feedback = self.perform(intent, &events).await;
            if events.send(Event::Finished(feedback)).await.is_err() {
                return;
            }
        }
    }

    async fn log_page(&self, request: &crate::logs::Request) -> Result<crate::logs::Page, String> {
        let mut url = self.origin.join("/logs").map_err(|_| "Invalid logs URL.")?;
        url.query_pairs_mut()
            .append_pair("since", &request.since)
            .append_pair("until", &request.until)
            .append_pair("min_level", request.level)
            .append_pair("limit", &request.limit.to_string())
            .append_pair(
                "direction",
                if matches!(
                    request.kind,
                    crate::logs::Load::Latest | crate::logs::Load::Older
                ) {
                    "backward"
                } else {
                    "forward"
                },
            );
        if let Some(cursor) = &request.cursor {
            url.query_pairs_mut().append_pair("cursor", cursor);
        }
        let value = self
            .request(
                reqwest::Method::GET,
                &format!("/logs?{}", url.query().unwrap()),
                None,
                None,
            )
            .await?;
        let page: crate::logs::Page =
            serde_json::from_value(value).map_err(|_| "Invalid log page response.")?;
        if page.schema_version != 1
            || page.items.len() > request.limit
            || (page.has_more_before && page.previous_cursor.is_none())
            || (page.has_more_after && page.next_cursor.is_none())
            || page.items.iter().any(|record| !valid_log_record(record))
        {
            return Err("Invalid log page schema or pagination.".into());
        }
        Ok(page)
    }

    pub async fn log_pages(
        self: Arc<Self>,
        mut requests: tokio::sync::watch::Receiver<Option<crate::logs::Request>>,
        events: mpsc::Sender<Event>,
    ) {
        loop {
            if requests.changed().await.is_err() {
                return;
            }
            let Some(mut request) = requests.borrow_and_update().clone() else {
                continue;
            };
            loop {
                tokio::select! {
                    changed = requests.changed() => {
                        if changed.is_err() { return; }
                        if let Some(next) = requests.borrow_and_update().clone() { request = next; }
                    }
                    page = self.log_page(&request) => {
                        if events.send(Event::LogPage(request, page)).await.is_err() { return; }
                        break;
                    }
                }
            }
        }
    }

    async fn stream_once(&self, logs: bool, events: &mpsc::Sender<Event>) -> Result<(), String> {
        let mut url = self.origin.clone();
        let scheme = if url.scheme() == "https" { "wss" } else { "ws" };
        url.set_scheme(scheme)
            .map_err(|_| "Invalid stream origin.")?;
        url.set_path(if logs {
            "/api/v1/logs/stream"
        } else {
            "/api/v1/events"
        });
        if logs {
            url.set_query(Some("min_level=DEBUG"));
        }
        let mut request = url
            .as_str()
            .into_client_request()
            .map_err(|_| "Cannot create stream request.")?;
        if let Some(token) = &self.token {
            let mut header = format!("Bearer {token}")
                .parse::<reqwest::header::HeaderValue>()
                .map_err(|_| "Invalid API token.")?;
            header.set_sensitive(true);
            request.headers_mut().insert("Authorization", header);
        }
        let config = WebSocketConfig::default()
            .max_message_size(Some(16384))
            .max_frame_size(Some(16384));
        let connection = connect_async_tls_with_config(
            request,
            Some(config),
            false,
            Some(Connector::Rustls(self.tls.clone())),
        );
        let (mut socket, _) = timeout(self.timeout, connection)
            .await
            .map_err(|_| "Daemon stream connection timed out.")?
            .map_err(|_| "Daemon stream unavailable: check authentication, URL and TLS.")?;
        let (mut sequence, mut instance, mut valid_at) = (0, None::<String>, Instant::now());
        loop {
            let remaining = Duration::from_secs(15).saturating_sub(valid_at.elapsed());
            let frame = timeout(remaining, socket.next())
                .await
                .map_err(|_| "Daemon stream timed out. Reconnecting...")?
                .ok_or("Daemon stream closed. Reconnecting...")?
                .map_err(|_| "Daemon stream lost. Reconnecting...")?;
            let text = match frame {
                Message::Text(text) => text,
                Message::Ping(_) => {
                    // Tungstenite queues the matching Pong; writes share the receive deadline.
                    timeout(
                        Duration::from_secs(15).saturating_sub(valid_at.elapsed()),
                        socket.flush(),
                    )
                        .await
                        .map_err(|_| "Daemon stream timed out. Reconnecting...")?
                        .map_err(|_| "Stream ping failed.")?;
                    continue;
                }
                Message::Pong(_) => continue,
                Message::Close(Some(close)) if close.code == tokio_tungstenite::tungstenite::protocol::frame::coding::CloseCode::Policy => {
                    return Err("Daemon rejected stream authentication or policy.".into());
                }
                _ => return Err("Daemon stream closed or invalid. Reconnecting...".into()),
            };
            let message: StreamMessage =
                serde_json::from_str(&text).map_err(|_| "Invalid API stream message.")?;
            if message.schema_version != 1
                || message.stream_sequence <= sequence
                || uuid::Uuid::parse_str(&message.server_instance_id).is_err()
                || chrono::DateTime::parse_from_rfc3339(&message.server_time).is_err()
            {
                return Err("Invalid API stream sequence or schema.".into());
            }
            if let Some(expected) = &instance {
                if expected != &message.server_instance_id {
                    return Err("Daemon instance changed within stream.".into());
                }
            } else {
                if message.kind != "snapshot" {
                    return Err("Missing initial daemon snapshot.".into());
                }
                instance = Some(message.server_instance_id.clone());
            }
            sequence = message.stream_sequence;
            match message.kind.as_str() {
                "snapshot" | "state" => {
                    let status: Status =
                        serde_json::from_value(message.data.ok_or("Missing status.")?)
                            .map_err(|_| "Invalid API status.")?;
                    if !status.valid() || status.server_instance_id != message.server_instance_id {
                        return Err("Invalid API status schema.".into());
                    }
                    if !logs {
                        events
                            .send(Event::Status(Box::new(status)))
                            .await
                            .map_err(|_| "UI closed.")?;
                    }
                }
                "command" if !logs => {
                    let command: Command =
                        serde_json::from_value(message.data.ok_or("Missing command.")?)
                            .map_err(|_| "Invalid command event.")?;
                    if !command.valid() {
                        return Err("Invalid command event.".into());
                    }
                    events
                        .send(Event::Command(command))
                        .await
                        .map_err(|_| "UI closed.")?;
                }
                "log" if logs => {
                    let record = message.data.ok_or("Missing log record.")?;
                    if !valid_log_record(&record) {
                        return Err("Invalid log record.".into());
                    }
                    events
                        .send(Event::Log(record))
                        .await
                        .map_err(|_| "UI closed.")?;
                }
                "gap" if logs => {
                    events
                        .send(Event::Notice(
                            "Log history gap; showing available records.".into(),
                        ))
                        .await
                        .map_err(|_| "UI closed.")?;
                }
                "heartbeat" => {}
                _ => return Err("Unsupported API stream event.".into()),
            }
            valid_at = Instant::now();
        }
    }

    pub async fn stream(self: Arc<Self>, logs: bool, events: mpsc::Sender<Event>) {
        loop {
            if let Err(reason) = self.stream_once(logs, &events).await {
                let event = if logs {
                    Event::Notice(format!("Logs: {reason}"))
                } else {
                    Event::Disconnected(reason)
                };
                if events.send(event).await.is_err() {
                    return;
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
        }
    }
}

fn valid_log_record(record: &Value) -> bool {
    record["timestamp"]
        .as_str()
        .is_some_and(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).is_ok())
        && record["sequence"].as_u64().is_some()
        && record["server_instance_id"]
            .as_str()
            .is_some_and(|id| uuid::Uuid::parse_str(id).is_ok())
        && record["message"].as_str().is_some()
        && record["level"].as_str().is_some()
}

fn outcome_uncertain() -> Feedback {
    Feedback::new(
        "Command outcome uncertain; check station before retrying",
        Severity::Warning,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn api_for(listener: &tokio::net::TcpListener) -> Api {
        Api::new(&Config {
            server: format!("http://{}", listener.local_addr().unwrap())
                .parse()
                .unwrap(),
            token: None,
            ca: None,
            timeout: Duration::from_secs(2),
            no_color: false,
            no_mouse: true,
            timezone: None,
        })
        .unwrap()
    }

    #[tokio::test]
    async fn http_body_limit_applies_to_the_complete_response_for_both_framing_modes() {
        const LIMIT: usize = 16 * 1024 * 1024;
        for chunked in [false, true] {
            for extra in [0, 1] {
                let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
                let api = api_for(&listener);
                let server = tokio::spawn(async move {
                    let (mut socket, request) = accept_http_request(&listener).await;
                    assert!(request.starts_with("GET /api/v1/logs HTTP/1.1"));
                    let length = LIMIT + extra;
                    let framing = if chunked {
                        "Transfer-Encoding: chunked".to_owned()
                    } else {
                        format!("Content-Length: {length}")
                    };
                    socket
                        .write_all(
                            format!("HTTP/1.1 200 OK\r\n{framing}\r\nConnection: close\r\n\r\n")
                                .as_bytes(),
                        )
                        .await?;
                    // Valid JSON on both sides of the boundary, divided into small writes.
                    let mut body = vec![b' '; length];
                    body[..2].copy_from_slice(b"{}");
                    for chunk in body.chunks(64 * 1024) {
                        if chunked {
                            socket
                                .write_all(format!("{:x}\r\n", chunk.len()).as_bytes())
                                .await?;
                        }
                        socket.write_all(chunk).await?;
                        if chunked {
                            socket.write_all(b"\r\n").await?;
                        }
                    }
                    if chunked {
                        socket.write_all(b"0\r\n\r\n").await?;
                    }
                    Ok::<_, std::io::Error>(())
                });
                let result = api.request(reqwest::Method::GET, "/logs", None, None).await;
                if extra == 0 {
                    assert_eq!(result.unwrap(), json!({}), "chunked={chunked}");
                } else {
                    assert_eq!(
                        result.unwrap_err(),
                        "API response exceeds client limit.",
                        "chunked={chunked}"
                    );
                }
                let sent = timeout(Duration::from_secs(3), server)
                    .await
                    .unwrap()
                    .unwrap();
                if extra == 0 {
                    sent.unwrap();
                }
            }
        }
    }

    async fn receive_stream_record(record: Value) -> (Result<(), String>, Option<Value>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = api_for(&listener);
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            let status = crate::tests::status();
            for (sequence, kind, data) in [
                (1, "snapshot", serde_json::to_value(&status).unwrap()),
                (2, "log", record),
            ] {
                socket
                    .send(Message::Text(
                        json!({
                            "schema_version": 1, "server_instance_id": status.server_instance_id,
                            "server_time": status.server_time,
                            "stream_sequence": sequence, "type": kind, "data": data
                        })
                        .to_string()
                        .into(),
                    ))
                    .await
                    .unwrap();
            }
            let _ = socket.close(None).await;
        });
        let (events, mut incoming) = mpsc::channel(4);
        let result = timeout(Duration::from_secs(3), api.stream_once(true, &events))
            .await
            .unwrap();
        server.await.unwrap();
        let received = match incoming.try_recv() {
            Ok(Event::Log(record)) => Some(record),
            Err(mpsc::error::TryRecvError::Empty) => None,
            _ => panic!("Unexpected event on log-only stream"),
        };
        assert!(incoming.try_recv().is_err());
        (result, received)
    }

    fn status_envelope(sequence: u64, kind: &str) -> Value {
        let status = crate::tests::status();
        json!({
            "schema_version": 1, "server_instance_id": status.server_instance_id,
            "server_time": status.server_time, "stream_sequence": sequence,
            "type": kind, "data": status
        })
    }

    async fn assert_status_stream(frames: Vec<Message>, count: usize, expected_error: &str) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = Arc::new(api_for(&listener));
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            for frame in frames {
                if socket.send(frame).await.is_err() {
                    break;
                }
            }
            let _ = socket.close(None).await;
        });
        let (events, mut incoming) = mpsc::channel(4);
        let worker = tokio::spawn(api.stream(false, events));
        let mut app = crate::app::App::new(false, Some(chrono_tz::UTC));
        for _ in 0..count {
            let event = timeout(Duration::from_secs(3), incoming.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(event, Event::Status(_)));
            app.update(event);
            assert!(app.allowed());
        }
        let event = timeout(Duration::from_secs(3), incoming.recv())
            .await
            .unwrap()
            .unwrap();
        let Event::Disconnected(error) = &event else {
            panic!("Expected stream rejection before another UI update");
        };
        assert_eq!(error, expected_error);
        app.update(event);
        assert!(!app.connected && !app.allowed());
        assert!(matches!(app.toggle(0), crate::app::Effect::None));
        assert!(incoming.try_recv().is_err());
        worker.abort();
        assert!(worker.await.unwrap_err().is_cancelled());
        timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn rejected_stream_frames_disconnect_before_later_state_can_enable_controls() {
        let text = |value: Value| Message::Text(value.to_string().into());
        for (path, value, error) in [
            (
                "/stream_sequence",
                json!(1),
                "Invalid API stream sequence or schema.",
            ),
            (
                "/stream_sequence",
                json!(0),
                "Invalid API stream sequence or schema.",
            ),
            (
                "/schema_version",
                json!(2),
                "Invalid API stream sequence or schema.",
            ),
            (
                "/server_instance_id",
                json!("invalid"),
                "Invalid API stream sequence or schema.",
            ),
            (
                "/server_instance_id",
                json!("6147f85c-53ef-42f4-b3f2-c15b071320a5"),
                "Daemon instance changed within stream.",
            ),
            (
                "/data/server_instance_id",
                json!("6147f85c-53ef-42f4-b3f2-c15b071320a5"),
                "Invalid API status schema.",
            ),
            (
                "/data/telemetry/sample/battery_percent",
                json!(101),
                "Invalid API status schema.",
            ),
            (
                "/data/telemetry/sample/input_power_w",
                json!(65536),
                "Invalid API status schema.",
            ),
            (
                "/data/telemetry/sample",
                Value::Null,
                "Invalid API status schema.",
            ),
            (
                "/data/telemetry/age_seconds",
                json!(-1),
                "Invalid API status schema.",
            ),
            ("/type", json!("unknown"), "Unsupported API stream event."),
        ] {
            let mut bad = status_envelope(2, "state");
            *bad.pointer_mut(path).unwrap() = value;
            assert_status_stream(
                vec![
                    text(status_envelope(1, "snapshot")),
                    text(bad),
                    text(status_envelope(3, "state")),
                ],
                1,
                error,
            )
            .await;
        }
        assert_status_stream(
            vec![
                text(status_envelope(1, "heartbeat")),
                text(status_envelope(2, "state")),
            ],
            0,
            "Missing initial daemon snapshot.",
        )
        .await;
        assert_status_stream(
            vec![
                text(status_envelope(1, "snapshot")),
                Message::Binary(status_envelope(2, "state").to_string().into_bytes().into()),
                text(status_envelope(3, "state")),
            ],
            1,
            "Daemon stream closed or invalid. Reconnecting...",
        )
        .await;
        // Sequence gaps are valid: the contract requires increasing, not consecutive IDs.
        assert_status_stream(
            vec![
                text(status_envelope(1, "snapshot")),
                text(status_envelope(10, "state")),
            ],
            2,
            "Daemon stream closed or invalid. Reconnecting...",
        )
        .await;
    }

    #[tokio::test]
    async fn websocket_limits_cover_single_frames_and_reassembled_messages() {
        use tokio_tungstenite::tungstenite::protocol::frame::{
            Frame,
            coding::{Data, OpCode},
        };
        for (size, fragmented) in [(16384, false), (16385, false), (16384, true), (16385, true)] {
            let mut envelope = status_envelope(2, "state");
            envelope["padding"] = json!("");
            let padding = size - envelope.to_string().len();
            envelope["padding"] = json!("x".repeat(padding));
            let payload = envelope.to_string();
            assert_eq!(payload.len(), size);
            let mut frames = vec![Message::Text(
                status_envelope(1, "snapshot").to_string().into(),
            )];
            if fragmented {
                let middle = payload.len() / 2;
                for (bytes, opcode, final_frame) in [
                    (&payload.as_bytes()[..middle], Data::Text, false),
                    (&payload.as_bytes()[middle..], Data::Continue, true),
                ] {
                    assert!(bytes.len() < 16384);
                    frames.push(Message::Frame(Frame::message(
                        bytes.to_vec(),
                        OpCode::Data(opcode),
                        final_frame,
                    )));
                }
            } else {
                frames.push(Message::Text(payload.into()));
            }
            assert_status_stream(
                frames,
                if size == 16384 { 2 } else { 1 },
                if size == 16384 {
                    "Daemon stream closed or invalid. Reconnecting..."
                } else {
                    "Daemon stream lost. Reconnecting..."
                },
            )
            .await;
        }
    }

    #[tokio::test]
    async fn heartbeats_require_valid_server_timestamps_before_the_next_state() {
        for (timestamp, valid) in [
            (Some(json!("2026-10-05T12:00:00Z")), true),
            (Some(json!("invalid")), false),
            (Some(Value::Null), false),
            (None, false),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = api_for(&listener);
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                let status = crate::tests::status();
                for (sequence, kind) in [(1, "snapshot"), (2, "heartbeat"), (3, "state")] {
                    let mut message = json!({
                        "schema_version": 1, "server_instance_id": status.server_instance_id,
                        "server_time": status.server_time, "stream_sequence": sequence,
                        "type": kind, "data": if kind == "heartbeat" { Value::Null } else { serde_json::to_value(&status).unwrap() }
                    });
                    if kind == "heartbeat" {
                        if let Some(timestamp) = &timestamp {
                            message["server_time"] = timestamp.clone();
                        } else {
                            message.as_object_mut().unwrap().remove("server_time");
                        }
                    }
                    socket
                        .send(Message::Text(message.to_string().into()))
                        .await
                        .unwrap();
                }
                let _ = socket.close(None).await;
            });
            let (events, mut incoming) = mpsc::channel(4);
            let result = timeout(Duration::from_secs(3), api.stream_once(false, &events))
                .await
                .unwrap();
            server.await.unwrap();
            let error = result.unwrap_err();
            assert_eq!(error.starts_with("Invalid API stream"), !valid);
            assert!(matches!(incoming.try_recv(), Ok(Event::Status(_))));
            if valid {
                assert!(matches!(incoming.try_recv(), Ok(Event::Status(_))));
            }
            assert!(
                incoming.try_recv().is_err(),
                "Unexpected state reached the UI"
            );
        }
    }

    #[tokio::test]
    async fn blocked_pong_writes_cannot_escape_the_valid_message_deadline() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let socket = tokio::net::TcpSocket::new_v4().unwrap();
        socket.set_recv_buffer_size(1024).unwrap();
        socket.bind("127.0.0.1:0".parse().unwrap()).unwrap();
        let listener = socket.listen(1).unwrap();
        let api = api_for(&listener);
        let sent = Arc::new(AtomicUsize::new(0));
        let count = sent.clone();
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            socket
                .send(Message::Text(
                    status_envelope(1, "snapshot").to_string().into(),
                ))
                .await
                .unwrap();
            // Never read the client's Pong replies; eventually its writes must block.
            let ping = Message::Ping(vec![b'p'; 125].into());
            for _ in 0..100_000 {
                if socket.send(ping.clone()).await.is_err() {
                    break;
                }
                count.fetch_add(1, Ordering::Relaxed);
            }
            std::future::pending::<()>().await;
        });
        let (events, mut incoming) = mpsc::channel(4);
        let result = timeout(Duration::from_secs(16), api.stream_once(false, &events)).await;
        server.abort();
        let _ = server.await;
        assert!(
            sent.load(Ordering::Relaxed) > 1000,
            "The server did not create backpressure"
        );
        let result = result.expect("A blocked Pong write escaped the 15-second stream deadline");
        assert_eq!(
            result.unwrap_err(),
            "Daemon stream timed out. Reconnecting..."
        );
        assert!(matches!(incoming.try_recv(), Ok(Event::Status(_))));
        assert!(
            incoming.try_recv().is_err(),
            "Pings became application updates"
        );
    }

    #[tokio::test]
    async fn heartbeats_extend_api_liveness_but_pings_do_not_refresh_telemetry_or_the_deadline() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = api_for(&listener);
        let server = tokio::spawn(async move {
            let (socket, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
            let status = crate::tests::status();
            socket
                .send(Message::Text(
                    json!({
                        "schema_version": 1, "server_instance_id": status.server_instance_id,
                        "server_time": status.server_time, "stream_sequence": 1,
                        "type": "snapshot", "data": status
                    })
                    .to_string()
                    .into(),
                ))
                .await
                .unwrap();
            let mut interval = tokio::time::interval(Duration::from_secs(1));
            interval.tick().await;
            let (mut ticks, mut sequence, mut heartbeats, mut pongs) = (0u8, 1, 0, 0);
            let mut pending_pings = std::collections::VecDeque::new();
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        ticks += 1;
                        if ticks <= 20 && ticks % 5 == 0 {
                            sequence += 1;
                            let message = json!({
                                "schema_version": 1, "server_instance_id": status.server_instance_id,
                                "server_time": chrono::Utc::now().to_rfc3339(),
                                "stream_sequence": sequence, "type": "heartbeat", "data": null
                            });
                            if socket.send(Message::Text(message.to_string().into())).await.is_err() {
                                break;
                            }
                            heartbeats += 1;
                        }
                        if socket.send(Message::Ping(vec![ticks].into())).await.is_err() {
                            break;
                        }
                        pending_pings.push_back(ticks);
                    }
                    frame = socket.next() => {
                        match frame {
                            Some(Ok(Message::Pong(payload))) => {
                                assert_eq!(payload.as_ref(), &[pending_pings.pop_front().unwrap()]);
                                pongs += 1;
                            }
                            _ => break,
                        }
                    }
                }
            }
            (heartbeats, pongs)
        });
        let (events, mut incoming) = mpsc::channel(4);
        let started = Instant::now();
        let worker = tokio::spawn(async move { api.stream_once(false, &events).await });
        let first = timeout(Duration::from_secs(2), incoming.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(first, Event::Status(_)));
        let mut app = crate::app::App::new(false, Some(chrono_tz::UTC));
        app.update(first);
        assert!(app.allowed());
        tokio::time::sleep(Duration::from_secs(4)).await;
        assert!(!worker.is_finished());
        assert!(app.connected);
        assert!(!app.live() && !app.allowed());
        assert!(matches!(app.toggle(0), crate::app::Effect::None));
        let result = timeout(Duration::from_secs(36), worker)
            .await
            .unwrap()
            .unwrap();
        let error = result.unwrap_err();
        assert_eq!(error, "Daemon stream timed out. Reconnecting...");
        assert!(started.elapsed() >= Duration::from_secs(35));
        assert!(started.elapsed() < Duration::from_secs(40));
        let (heartbeats, pongs) = timeout(Duration::from_secs(2), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(heartbeats, 4);
        assert!(pongs >= 25);
        assert!(
            incoming.try_recv().is_err(),
            "Heartbeat or Ping became a telemetry update"
        );
        app.update(Event::Disconnected(error));
        assert!(!app.connected && !app.allowed());
    }

    #[tokio::test]
    async fn log_stream_rejects_malformed_records_before_queueing() {
        let valid = json!({
            "timestamp": "2026-10-05T12:00:00Z", "sequence": 4,
            "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
            "level": "INFO", "message": "Station connected"
        });
        let mut records = vec![Value::Null, json!([]), json!("not a record")];
        for (field, value) in [
            ("timestamp", json!("invalid")),
            ("sequence", json!(-1)),
            ("server_instance_id", json!("invalid")),
            ("level", json!(42)),
            ("message", json!(42)),
        ] {
            let mut record = valid.clone();
            record[field] = value;
            records.push(record);
            let mut record = valid.clone();
            record.as_object_mut().unwrap().remove(field);
            records.push(record);
        }
        for record in records {
            let expected = if record.is_null() {
                "Missing log record."
            } else {
                "Invalid log record."
            };
            let (result, received) = receive_stream_record(record).await;
            assert_eq!(result.unwrap_err(), expected);
            assert!(received.is_none(), "Malformed data reached the UI");
        }
    }

    #[tokio::test]
    async fn log_stream_keeps_retained_records_from_previous_daemon_instances() {
        let record = json!({
            "timestamp": "2026-10-04T12:00:00Z", "sequence": 4,
            "server_instance_id": "6147f85c-53ef-42f4-b3f2-c15b071320a5",
            "level": "INFO", "message": "Previous daemon stopped"
        });
        let (_, received) = receive_stream_record(record.clone()).await;
        assert_eq!(received, Some(record));
    }

    #[tokio::test]
    async fn log_pages_require_text_levels_and_preserve_named_levels() {
        for level in [
            None,
            Some(json!(null)),
            Some(json!(42)),
            Some(json!([])),
            Some(json!({})),
            Some(json!("DEBUG")),
            Some(json!("INFO")),
            Some(json!("WARNING")),
            Some(json!("ERROR")),
            Some(json!("CRITICAL")),
        ] {
            let mut record = json!({
                "timestamp": "2026-10-05T12:00:00Z", "sequence": 4,
                "server_instance_id": "88767477-2a2a-481f-843b-30d56a5e3f10",
                "message": "Diagnostic record"
            });
            let valid = level.as_ref().is_some_and(Value::is_string);
            if let Some(level) = level {
                record["level"] = level;
            }
            let page = json!({
                "schema_version": 1, "items": [record.clone()],
                "previous_cursor": null, "next_cursor": null,
                "has_more_before": false, "has_more_after": false,
                "source": "files", "gap": false, "skipped_lines": 0
            });
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = api_for(&listener);
            let server = tokio::spawn(async move {
                let (mut socket, request) = accept_http_request(&listener).await;
                assert!(request.starts_with("GET /api/v1/logs?"));
                let body = page.to_string();
                socket.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ).as_bytes()).await.unwrap();
            });
            let request = crate::logs::Logs::new(Some(chrono_tz::UTC)).open().unwrap();
            let result = api.log_page(&request).await;
            if valid {
                assert_eq!(result.unwrap().items, vec![record]);
            } else {
                assert_eq!(
                    result.unwrap_err(),
                    "Invalid log page schema or pagination."
                );
            }
            timeout(Duration::from_secs(3), server)
                .await
                .unwrap()
                .unwrap();
        }
    }

    async fn accept_http_request(
        listener: &tokio::net::TcpListener,
    ) -> (tokio::net::TcpStream, String) {
        timeout(Duration::from_secs(3), async {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0u8; 4096];
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
                if bytes.windows(4).any(|window| window == b"\r\n\r\n") {
                    break;
                }
            }
            (socket, String::from_utf8(bytes).unwrap())
        })
        .await
        .unwrap()
    }

    #[tokio::test]
    async fn log_queries_replace_a_stalled_request_with_the_latest_navigation() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = Arc::new(api_for(&listener));
        let (requests, operations) = tokio::sync::watch::channel(None);
        let (events, mut incoming) = mpsc::channel(4);
        let worker = tokio::spawn(api.log_pages(operations, events));
        let mut logs = crate::logs::Logs::new(Some(chrono_tz::UTC));
        let first = logs.open().unwrap();
        requests.send(Some(first)).unwrap();
        // Keep the first connection open without a response throughout the next query.
        let (_stalled, _) = accept_http_request(&listener).await;
        requests.send(logs.navigate(false)).unwrap();
        let newest = logs.navigate(false).unwrap();
        requests.send(Some(newest.clone())).unwrap();
        let (mut socket, headers) = accept_http_request(&listener).await;
        let path = headers
            .lines()
            .next()
            .unwrap()
            .split_whitespace()
            .nth(1)
            .unwrap();
        let url = reqwest::Url::parse(&format!("http://localhost{path}")).unwrap();
        assert_eq!(url.path(), "/api/v1/logs");
        assert_eq!(
            url.query_pairs().find(|(key, _)| key == "since").unwrap().1,
            newest.since
        );
        let body = json!({
            "schema_version": 1, "items": [], "previous_cursor": null, "next_cursor": null,
            "has_more_before": false, "has_more_after": false, "source": "files",
            "gap": false, "skipped_lines": 0
        })
        .to_string();
        socket
            .write_all(
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
                .as_bytes(),
            )
            .await
            .unwrap();
        let Event::LogPage(request, page) = timeout(Duration::from_secs(1), incoming.recv())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("Expected the latest log page");
        };
        assert_eq!(request.generation, newest.generation);
        assert!(page.unwrap().items.is_empty());
        assert!(incoming.try_recv().is_err());
        drop(requests);
        timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn closing_log_requests_cancels_an_in_flight_query() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = Arc::new(api_for(&listener));
        let (requests, operations) = tokio::sync::watch::channel(None);
        let (events, mut incoming) = mpsc::channel(4);
        let worker = tokio::spawn(api.log_pages(operations, events));
        let mut logs = crate::logs::Logs::new(Some(chrono_tz::UTC));
        requests.send(logs.open()).unwrap();
        let (_stalled, _) = accept_http_request(&listener).await;
        drop(requests);
        timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap();
        assert!(incoming.recv().await.is_none());
    }

    #[tokio::test]
    async fn slow_admission_body_times_out_without_replay_and_the_next_read_still_works() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = api_for(&listener);
        let mut app = crate::app::App::new(false, Some(chrono_tz::UTC));
        app.update(Event::Status(Box::new(crate::tests::status())));
        let crate::app::Effect::Request(intent) = app.toggle(0) else {
            panic!("Expected an AC command");
        };
        let Intent::Output { key, .. } = &intent else {
            panic!("Expected an output intention");
        };
        let expected_key = key.clone();
        let (done, mut stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, request) = accept_http_request(&listener).await;
            let (headers, initial_body) = request.split_once("\r\n\r\n").unwrap();
            assert!(headers.starts_with("PUT /api/v1/outputs/ac HTTP/1.1"));
            assert!(
                headers
                    .to_ascii_lowercase()
                    .contains(&format!("idempotency-key: {expected_key}"))
            );
            let length: usize = headers
                .lines()
                .find_map(|line| {
                    let (name, value) = line.split_once(':')?;
                    name.eq_ignore_ascii_case("content-length")
                        .then(|| value.trim().parse().unwrap())
                })
                .unwrap();
            let mut body = initial_body.as_bytes().to_vec();
            while body.len() < length {
                let mut chunk = [0; 4096];
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                body.extend_from_slice(&chunk[..count]);
            }
            let body: Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(body["enabled"], true);
            assert_eq!(body["expected_outputs_revision"], 2);
            socket
                .write_all(b"HTTP/1.1 202 Accepted\r\nTransfer-Encoding: chunked\r\n\r\n1\r\n{\r\n")
                .await
                .unwrap();
            let mut interval = tokio::time::interval(Duration::from_millis(250));
            interval.tick().await;
            let mut chunks = 0;
            loop {
                tokio::select! {
                    _ = &mut stopped => break,
                    _ = interval.tick() => {
                        if socket.write_all(b"1\r\n \r\n").await.is_err() { break; }
                        chunks += 1;
                    }
                }
            }
            // Only the caller's later read may open another connection, never a replayed PUT.
            let (mut next, request) = accept_http_request(&listener).await;
            assert!(request.starts_with("GET /api/v1/status HTTP/1.1"));
            next.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}")
                .await
                .unwrap();
            assert!(
                timeout(Duration::from_millis(200), listener.accept())
                    .await
                    .is_err()
            );
            chunks
        });
        let (events, mut incoming) = mpsc::channel(4);
        let started = Instant::now();
        let feedback = timeout(Duration::from_secs(4), api.perform(intent, &events))
            .await
            .unwrap();
        assert!(started.elapsed() >= Duration::from_secs(2));
        assert_eq!(
            feedback.message,
            "Command outcome uncertain; check station before retrying"
        );
        assert!(matches!(feedback.severity, Severity::Warning));
        assert!(
            incoming.try_recv().is_err(),
            "Partial admission must not become a command event"
        );
        app.update(Event::Finished(feedback));
        assert!(app.pending.is_none());
        assert!(
            !app.status
                .as_ref()
                .unwrap()
                .telemetry
                .sample
                .as_ref()
                .unwrap()
                .ac_enabled
        );
        let _ = done.send(());
        assert_eq!(
            api.request(reqwest::Method::GET, "/status", None, None)
                .await
                .unwrap(),
            json!({})
        );
        let chunks = timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
        assert!(
            chunks >= 2,
            "The response must keep delivering data before the total timeout"
        );
    }

    #[tokio::test]
    async fn uncertain_admission_is_one_put_with_captured_revision_and_key() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = api_for(&listener);
        let key = uuid::Uuid::new_v4().to_string();
        let expected_key = key.clone();
        let server = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let mut bytes = Vec::new();
            loop {
                let mut chunk = [0u8; 4096];
                let count = socket.read(&mut chunk).await.unwrap();
                assert!(count > 0);
                bytes.extend_from_slice(&chunk[..count]);
                let text = String::from_utf8_lossy(&bytes);
                if let Some((headers, body)) = text.split_once("\r\n\r\n") {
                    let length: usize = headers
                        .lines()
                        .find_map(|line| {
                            line.to_ascii_lowercase()
                                .strip_prefix("content-length: ")
                                .map(|value| value.parse().unwrap())
                        })
                        .unwrap();
                    if body.len() >= length {
                        assert!(headers.starts_with("PUT /api/v1/outputs/ac HTTP/1.1"));
                        assert!(
                            headers
                                .to_ascii_lowercase()
                                .contains(&format!("idempotency-key: {expected_key}"))
                        );
                        let value: Value = serde_json::from_str(body).unwrap();
                        assert_eq!(value["enabled"], true);
                        assert_eq!(value["expected_outputs_revision"], 2);
                        break;
                    }
                }
            }
            // Simulate a lost/invalid admission response after the PUT has arrived.
            socket
                .write_all(
                    b"HTTP/1.1 202 Accepted\r\nContent-Length: 4\r\nConnection: close\r\n\r\noops",
                )
                .await
                .unwrap();
            drop(socket);
            assert!(
                timeout(Duration::from_millis(300), listener.accept())
                    .await
                    .is_err(),
                "PUT must not be replayed"
            );
        });
        let (events, _incoming) = mpsc::channel(4);
        let result = api
            .perform(
                Intent::Output {
                    output: "ac",
                    enabled: true,
                    snapshot: Box::new(crate::tests::status()),
                    key: key.clone(),
                },
                &events,
            )
            .await;
        assert!(result.message.contains("Command outcome uncertain"));
        assert!(!result.message.contains(&key));
        assert_eq!(result.severity, Severity::Warning);
        server.await.unwrap();
    }
}
