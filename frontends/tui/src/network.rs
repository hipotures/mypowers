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
            || page.items.iter().any(|record| {
                record["timestamp"]
                    .as_str()
                    .is_none_or(|stamp| chrono::DateTime::parse_from_rfc3339(stamp).is_err())
                    || record["sequence"].as_u64().is_none()
                    || record["server_instance_id"]
                        .as_str()
                        .is_none_or(|id| uuid::Uuid::parse_str(id).is_err())
                    || record["message"].as_str().is_none()
            })
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
                Message::Ping(data) => {
                    socket
                        .send(Message::Pong(data))
                        .await
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
                    events
                        .send(Event::Log(message.data.ok_or("Missing log record.")?))
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

    #[tokio::test]
    async fn uncertain_admission_is_one_put_with_captured_revision_and_key() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let config = Config {
            server: format!("http://{}", listener.local_addr().unwrap())
                .parse()
                .unwrap(),
            token: None,
            ca: None,
            timeout: Duration::from_secs(2),
            no_color: false,
            no_mouse: true,
            timezone: None,
        };
        let api = Api::new(&config).unwrap();
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
