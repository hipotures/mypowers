use crate::{
    config::Config,
    feedback::{Feedback, Severity},
    model::{Command, Connection, Status, StreamMessage, safe},
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

// Outbound log records need space for their stream envelope as well as their text.
const STREAM_MESSAGE_LIMIT: usize = 32 * 1024;

#[derive(Clone, Copy, PartialEq)]
pub enum ClipboardTarget {
    Snapshot,
    Logs,
}

pub enum Event {
    Settings(crate::settings::Settings),
    SettingsUnavailable,
    LogStreamReady,
    Status(Box<Status>),
    Disconnected(String),
    Command(Command),
    Log(Value),
    LogPage(crate::logs::Request, Result<crate::logs::Page, String>),
    History(
        crate::history::Request,
        Result<Vec<crate::history::Point>, String>,
    ),
    Notice(String),
    Finished(Feedback),
    Copied(bool, ClipboardTarget),
    Exit,
}

pub enum Intent {
    SaveSettings(crate::settings::Settings),
    TestTelegram,
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

#[derive(Debug)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    pub status: u16,
}

impl From<&str> for ApiError {
    fn from(message: &str) -> Self {
        Self {
            code: "incompatible_response".into(),
            message: message.into(),
            status: 0,
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
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
        self.request_detailed(method, path, body, key)
            .await
            .map_err(|e| e.to_string())
    }

    pub async fn request_detailed(
        &self,
        method: reqwest::Method,
        path: &str,
        body: Option<Value>,
        key: Option<&str>,
    ) -> Result<Value, ApiError> {
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
        let mut response = request.send().await.map_err(|_| ApiError {
            code: "server_unreachable".into(),
            message: "Cannot reach daemon or verify TLS.".into(),
            status: 0,
        })?;
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
            return Err(ApiError {
                code: safe(code).into_owned(),
                message: format!("API {}: {}", status.as_u16(), safe(code)),
                status: status.as_u16(),
            });
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

    fn connection_response(value: Value) -> Result<Connection, String> {
        let connection: Connection =
            serde_json::from_value(value).map_err(|_| "Invalid connection response.")?;
        if !connection.valid() {
            return Err("Invalid connection response.".into());
        }
        Ok(connection)
    }

    async fn perform(&self, intent: Intent, events: &mpsc::Sender<Event>) -> Feedback {
        match intent {
            Intent::TestTelegram => {
                match self
                    .request(
                        reqwest::Method::POST,
                        "/notifications/telegram/test",
                        Some(json!({})),
                        None,
                    )
                    .await
                {
                    Ok(value) if value["status"].as_str() == Some("sent") => {
                        Feedback::new("Telegram test sent", Severity::Success)
                    }
                    _ => Feedback::new(
                        "Telegram test failed; see server logs/configuration",
                        Severity::Error,
                    ),
                }
            }
            Intent::SaveSettings(draft) => {
                let result = self
                    .request(
                        reqwest::Method::PUT,
                        "/settings",
                        Some(json!({"graph_interval_seconds": draft.graph_interval_seconds, "graph_visualization": draft.graph_visualization, "graph_base_scale_w": draft.graph_base_scale_w, "timezone": draft.timezone, "logs_page_size": draft.logs_page_size, "battery_alert": draft.battery_alert})),
                        None,
                    )
                    .await
                    .and_then(Self::settings_response);
                match result {
                    Ok(settings) if settings == draft => {
                        let _ = events.send(Event::Settings(settings)).await;
                        Feedback::new("Settings saved", Severity::Success)
                    }
                    _ => Feedback::new("Could not save settings; retrying", Severity::Error),
                }
            }

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
                        Ok(Ok(next))
                            if next.output == output && next.requested_enabled == enabled =>
                        {
                            command = next;
                        }
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
                .and_then(Self::connection_response)
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
                .and_then(Self::connection_response)
                .and_then(|connection| {
                    if (connection.desired == "running") != running {
                        return Err("Invalid connection response.".into());
                    }
                    Ok(Feedback::new(
                        if running {
                            "Station connection resumed"
                        } else {
                            "Station connection paused"
                        },
                        Severity::Info,
                    ))
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
                .and_then(|value| {
                    let configured = value["configured_level"]
                        .as_str()
                        .filter(|level| crate::logs::LEVELS.contains(level))
                        .ok_or("Invalid log level response.")?;
                    let level = value["effective_level"]
                        .as_str()
                        .filter(|level| crate::logs::LEVELS.contains(level))
                        .ok_or("Invalid log level response.")?;
                    if level != if enabled { "DEBUG" } else { configured }
                        || !value.get("override_expires_at").is_some_and(Value::is_null)
                    {
                        return Err("Invalid log level response.".into());
                    }
                    Ok(Feedback::new(
                        format!("Log level changed to {level}"),
                        Severity::Info,
                    ))
                })
                .unwrap_or_else(|error| Feedback::request_error(&error)),
        }
    }

    fn settings_response(value: Value) -> Result<crate::settings::Settings, String> {
        let settings: crate::settings::Settings =
            serde_json::from_value(value).map_err(|_| "Invalid settings response.")?;
        if !settings.valid() {
            return Err("Invalid settings interval or schema.".into());
        }
        Ok(settings)
    }

    pub async fn load_settings(self: Arc<Self>, events: mpsc::Sender<Event>) {
        loop {
            match self
                .request(reqwest::Method::GET, "/settings", None, None)
                .await
                .and_then(Self::settings_response)
            {
                Ok(settings) => {
                    let _ = events.send(Event::Settings(settings)).await;
                    return;
                }
                Err(_) => {
                    if events.send(Event::SettingsUnavailable).await.is_err() {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_secs(2)).await;
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

    pub async fn history(
        &self,
        request: &crate::history::Request,
    ) -> Result<Vec<crate::history::Point>, String> {
        let mut url = self
            .origin
            .join("/history/aggregates")
            .map_err(|_| "Invalid history URL.")?;
        url.query_pairs_mut()
            .append_pair("since", &request.since.to_rfc3339())
            .append_pair("until", &request.until.to_rfc3339())
            .append_pair("bucket_seconds", &request.resolution.seconds().to_string())
            .append_pair("limit", &request.width.to_string());
        let value = self
            .request(
                reqwest::Method::GET,
                &format!("/history/aggregates?{}", url.query().unwrap()),
                None,
                None,
            )
            .await?;
        let response: crate::history::Response =
            serde_json::from_value(value).map_err(|_| "Invalid history response.")?;
        if response.schema_version != 1
            || response.source != "database"
            || response.bucket_seconds != request.resolution.seconds()
            || response.since_ms != request.since.timestamp_millis()
            || response.until_ms != request.until.timestamp_millis()
            || response.items.len() > usize::from(request.width)
        {
            return Err("Invalid or oversized history response.".into());
        }
        let mut previous = None;
        for point in &response.items {
            if !point.valid(request) || previous.is_some_and(|last| point.bucket_start_ms <= last) {
                return Err("Invalid history bucket or order.".into());
            }
            previous = Some(point.bucket_start_ms);
        }
        Ok(response.items)
    }

    pub async fn history_aggregates(
        self: Arc<Self>,
        mut requests: tokio::sync::watch::Receiver<Option<crate::history::Request>>,
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
                        let Some(next) = requests.borrow_and_update().clone() else { break; };
                        request = next;
                    }
                    result = timeout(self.timeout, self.history(&request)) => {
                        let result = result.unwrap_or_else(|_| Err("History request timed out.".into()));
                        if events.send(Event::History(request, result)).await.is_err() { return; }
                        break;
                    }
                }
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
        let mut page: crate::logs::Page =
            serde_json::from_value(value).map_err(|_| "Invalid log page response.")?;
        if page.schema_version != 1
            || page.items.len() > request.limit
            || (page.has_more_before && page.previous_cursor.is_none())
            || (page.has_more_after && page.next_cursor.is_none())
            || page.items.iter().any(|record| !valid_log_record(record))
        {
            return Err("Invalid log page schema or pagination.".into());
        }
        if request.discover_oldest {
            let value = self
                .request(
                    reqwest::Method::GET,
                    "/logs?direction=forward&min_level=DEBUG&limit=1",
                    None,
                    None,
                )
                .await?;
            let boundary: crate::logs::Page =
                serde_json::from_value(value).map_err(|_| "Invalid oldest log response.")?;
            if boundary.schema_version != 1
                || boundary.items.len() > 1
                || boundary.has_more_before
                || boundary
                    .items
                    .iter()
                    .any(|record| !valid_log_record(record))
            {
                return Err("Invalid oldest log response.".into());
            }
            page.oldest_record = Some(boundary.items.first().map(|record| {
                chrono::DateTime::parse_from_rfc3339(record["timestamp"].as_str().unwrap())
                    .unwrap()
                    .with_timezone(&chrono::Utc)
            }));
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
        self.stream_query(logs, None, events).await
    }

    pub async fn follow_logs(
        &self,
        query: &str,
        events: &mpsc::Sender<Event>,
    ) -> Result<(), String> {
        self.stream_query(true, Some(query), events).await
    }

    async fn stream_query(
        &self,
        logs: bool,
        query: Option<&str>,
        events: &mpsc::Sender<Event>,
    ) -> Result<(), String> {
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
            url.set_query(Some(query.unwrap_or("min_level=DEBUG")));
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
            .max_message_size(Some(STREAM_MESSAGE_LIMIT))
            .max_frame_size(Some(STREAM_MESSAGE_LIMIT));
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
            let initial = instance.is_none();
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
                    } else if initial {
                        events
                            .send(Event::LogStreamReady)
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
            client_preferences: crate::client_ui::ClientPreferences::default(),
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
        assert!(matches!(incoming.try_recv(), Ok(Event::LogStreamReady)));
        let received = match incoming.try_recv() {
            Ok(Event::Log(record)) => Some(record),
            Err(mpsc::error::TryRecvError::Empty) => None,
            _ => panic!("Unexpected event on log-only stream"),
        };
        assert!(incoming.try_recv().is_err());
        (result, received)
    }

    #[tokio::test]
    async fn log_stream_recovery_requires_a_valid_initial_snapshot() {
        for invalid in [
            None,
            Some("/data/schema_version"),
            Some("/data/server_time"),
            Some("/server_instance_id"),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = api_for(&listener);
            let mut snapshot = status_envelope(1, "snapshot");
            if let Some(path) = invalid {
                *snapshot.pointer_mut(path).unwrap() = json!("invalid");
            }
            let server = tokio::spawn(async move {
                let (socket, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(socket).await.unwrap();
                socket
                    .send(Message::Text(snapshot.to_string().into()))
                    .await
                    .unwrap();
                let _ = socket.close(None).await;
            });
            let (events, mut incoming) = mpsc::channel(4);
            assert!(
                timeout(Duration::from_secs(2), api.stream_once(true, &events))
                    .await
                    .unwrap()
                    .is_err()
            );
            server.await.unwrap();
            if invalid.is_none() {
                assert!(matches!(incoming.try_recv(), Ok(Event::LogStreamReady)));
            }
            assert!(
                incoming.try_recv().is_err(),
                "Invalid snapshots cannot rearm notices"
            );
        }
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
                "/data/connection/desired",
                json!("invalid"),
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
        for (size, fragmented) in [
            (STREAM_MESSAGE_LIMIT, false),
            (STREAM_MESSAGE_LIMIT + 1, false),
            (STREAM_MESSAGE_LIMIT, true),
            (STREAM_MESSAGE_LIMIT + 1, true),
        ] {
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
                    assert!(bytes.len() < STREAM_MESSAGE_LIMIT);
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
                if size == STREAM_MESSAGE_LIMIT { 2 } else { 1 },
                if size == STREAM_MESSAGE_LIMIT {
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
    async fn log_stream_preserves_long_unicode_messages() {
        let message = "🔋".repeat(4096);
        let record = json!({
            "schema_version": 1, "timestamp": "2026-10-05T12:00:00Z",
            "server_instance_id": "11111111-1111-4111-8111-111111111111",
            "sequence": 1, "level": "CRITICAL", "logger": "mypowers",
            "event": "application", "message": message, "context": {"truncated": true}
        });
        assert_eq!(record["message"].as_str().unwrap().len(), 16384);
        let (_, received) = receive_stream_record(record.clone()).await;
        assert!(
            received.as_ref() == Some(&record),
            "Unicode log was not preserved"
        );
        let mut logs = crate::logs::Logs::new_at(
            Some(chrono_tz::UTC),
            chrono::DateTime::parse_from_rfc3339("2026-10-05T12:00:00Z")
                .unwrap()
                .with_timezone(&chrono::Utc),
        );
        logs.records.push_back(record);
        assert!(logs.clipboard_text().contains(&message));
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
    async fn oldest_log_query_is_global_and_unfiltered_even_when_selected_day_is_empty() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = api_for(&listener);
        let server = tokio::spawn(async move {
            for step in 0..2 {
                let (mut socket, headers) = accept_http_request(&listener).await;
                if step == 0 {
                    assert!(headers.contains("min_level=ERROR"));
                    assert!(headers.contains("since="));
                } else {
                    assert!(headers.starts_with(
                        "GET /api/v1/logs?direction=forward&min_level=DEBUG&limit=1 "
                    ));
                    assert!(
                        !headers.contains("since=")
                            && !headers.contains("until=")
                            && !headers.contains("cursor=")
                    );
                }
                let body = json!({
                    "schema_version": 1, "items": if step == 0 { vec![] } else { vec![json!({
                        "timestamp":"2026-10-03T23:30:00Z", "sequence":1,
                        "server_instance_id":"88767477-2a2a-481f-843b-30d56a5e3f10",
                        "level":"DEBUG", "message":"Oldest retained record"
                    })] }, "previous_cursor":null, "next_cursor":null,
                    "has_more_before":false, "has_more_after":false,
                    "source":"files", "gap":false, "skipped_lines":0
                })
                .to_string();
                socket.write_all(format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",body.len()).as_bytes()).await.unwrap();
            }
        });
        let mut logs = crate::logs::Logs::new(Some(chrono_tz::UTC));
        logs.level = 3;
        let request = logs.open().unwrap();
        let page = api.log_page(&request).await.unwrap();
        assert!(page.items.is_empty());
        assert_eq!(
            page.oldest_record.unwrap().unwrap().to_rfc3339(),
            "2026-10-03T23:30:00+00:00"
        );
        server.await.unwrap();
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
            let mut request = crate::logs::Logs::new(Some(chrono_tz::UTC)).open().unwrap();
            request.discover_oldest = false;
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

    fn graph_history_request(generation: u64) -> crate::history::Request {
        let now = chrono::DateTime::parse_from_rfc3339("2026-10-05T12:00:00Z")
            .unwrap()
            .timestamp_millis();
        let mut status = crate::tests::status();
        status.server_instance_id = "test-daemon".into();
        crate::history::Request::new(
            &status,
            generation,
            crate::history::Resolution::TenSeconds,
            3,
            now,
        )
        .unwrap()
    }

    fn graph_history_point(start: i64) -> Value {
        json!({"bucket_start_ms": start, "input_power_w": 0.5, "output_power_w": 1.5, "sample_count": 2})
    }

    fn graph_response(request: &crate::history::Request, items: Value) -> Value {
        json!({"schema_version": 1, "source": "database", "bucket_seconds": request.resolution.seconds(),
            "since_ms": request.since.timestamp_millis(), "until_ms": request.until.timestamp_millis(), "items": items})
    }

    async fn respond_json(socket: &mut tokio::net::TcpStream, value: Value) {
        let body = value.to_string();
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
    }

    #[tokio::test]
    async fn graph_queries_request_visible_server_averages_at_all_resolutions() {
        for resolution in [
            crate::history::Resolution::TenSeconds,
            crate::history::Resolution::ThirtySeconds,
            crate::history::Resolution::Minute,
            crate::history::Resolution::Hour,
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = api_for(&listener);
            let mut request = graph_history_request(4);
            request.resolution = resolution;
            request.since = request.until
                - chrono::TimeDelta::milliseconds(1 + 2 * resolution.seconds() * 1000);
            let expected = request.clone();
            let server = tokio::spawn(async move {
                let (mut socket, headers) = accept_http_request(&listener).await;
                let target = headers
                    .lines()
                    .next()
                    .unwrap()
                    .split_whitespace()
                    .nth(1)
                    .unwrap();
                let url = reqwest::Url::parse(&format!("http://localhost{target}")).unwrap();
                assert_eq!(url.path(), "/api/v1/history/aggregates");
                let query: std::collections::HashMap<_, _> =
                    url.query_pairs().into_owned().collect();
                assert_eq!(
                    query["bucket_seconds"],
                    expected.resolution.seconds().to_string()
                );
                assert_eq!(query["limit"], "3");
                assert_eq!(query["since"], expected.since.to_rfc3339());
                assert_eq!(query["until"], expected.until.to_rfc3339());
                assert!(!query.contains_key("cursor"));
                respond_json(
                    &mut socket,
                    graph_response(
                        &expected,
                        json!([graph_history_point(expected.since.timestamp_millis())]),
                    ),
                )
                .await;
            });
            let points = api.history(&request).await.unwrap();
            assert_eq!(points.len(), 1);
            assert_eq!(points[0].input_power_w, 0.5);
            assert_eq!(points[0].sample_count, 2);
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn graph_history_rejects_wrong_ranges_resolutions_counts_and_unordered_buckets() {
        let request = graph_history_request(0);
        let start = request.since.timestamp_millis();
        let valid = graph_response(&request, json!([graph_history_point(start)]));
        let mut cases = Vec::new();
        for (field, value) in [
            ("schema_version", json!(2)),
            ("source", json!("memory")),
            ("bucket_seconds", json!(60)),
            ("since_ms", json!(start - 10_000)),
            ("until_ms", json!(request.until.timestamp_millis() + 1)),
        ] {
            let mut response = valid.clone();
            response[field] = value;
            cases.push(response);
        }
        for (field, value) in [
            ("sample_count", json!(0)),
            ("sample_count", json!(-1)),
            ("input_power_w", json!(-0.1)),
            ("output_power_w", json!(65535.1)),
            ("input_power_w", json!("NaN")),
            ("bucket_start_ms", json!(start + 1)),
            ("bucket_start_ms", json!(start - 10_000)),
            (
                "bucket_start_ms",
                json!(request.until.timestamp_millis() + 9999),
            ),
        ] {
            let mut response = valid.clone();
            response["items"][0][field] = value;
            cases.push(response);
        }
        let mut missing = valid.clone();
        missing["items"][0]
            .as_object_mut()
            .unwrap()
            .remove("sample_count");
        cases.push(missing);
        cases.push(graph_response(
            &request,
            json!([graph_history_point(start), graph_history_point(start)]),
        ));
        cases.push(graph_response(
            &request,
            json!([
                graph_history_point(start + 10_000),
                graph_history_point(start)
            ]),
        ));
        for response in cases {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = api_for(&listener);
            let server = tokio::spawn(async move {
                let (mut socket, _) = accept_http_request(&listener).await;
                respond_json(&mut socket, response).await;
            });
            assert!(api.history(&request).await.is_err());
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn graph_history_accepts_the_bucket_limit_and_rejects_an_extra_bucket() {
        for extra in [0, 1] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = api_for(&listener);
            let mut request = graph_history_request(0);
            request.width = crate::history::MAX_BUCKETS;
            request.since = request.until
                - chrono::TimeDelta::milliseconds(1 + i64::from(request.width - 1) * 10_000);
            let expected = request.clone();
            let server = tokio::spawn(async move {
                let (mut socket, _) = accept_http_request(&listener).await;
                let items: Vec<_> = (0..usize::from(expected.width) + extra)
                    .map(|index| {
                        graph_history_point(
                            expected.since.timestamp_millis() + index as i64 * 10_000,
                        )
                    })
                    .collect();
                respond_json(&mut socket, graph_response(&expected, json!(items))).await;
            });
            let result = api.history(&request).await;
            if extra == 0 {
                assert_eq!(
                    result.unwrap().len(),
                    usize::from(crate::history::MAX_BUCKETS)
                );
            } else {
                assert_eq!(
                    result.unwrap_err(),
                    "Invalid or oversized history response."
                );
            }
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn partial_aggregate_response_times_out_without_delivering_partial_history() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut api = api_for(&listener);
        api.timeout = Duration::from_millis(300);
        let server = tokio::spawn(async move {
            let (mut socket, _) = accept_http_request(&listener).await;
            let request = graph_history_request(0);
            let body = graph_response(
                &request,
                json!([graph_history_point(request.since.timestamp_millis())]),
            )
            .to_string();
            socket
                .write_all(
                    format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n{}",
                        body.len(),
                        &body[..body.len() / 2]
                    )
                    .as_bytes(),
                )
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_secs(5)).await;
        });
        let (requests, operations) = tokio::sync::watch::channel(None);
        let (events, mut incoming) = mpsc::channel(4);
        let worker = tokio::spawn(Arc::new(api).history_aggregates(operations, events));
        requests.send(Some(graph_history_request(1))).unwrap();
        let Event::History(_, result) = timeout(Duration::from_secs(2), incoming.recv())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("Expected bounded response");
        };
        assert_eq!(result.unwrap_err(), "History request timed out.");
        assert!(incoming.try_recv().is_err());
        server.abort();
        drop(requests);
        timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn graph_history_worker_cancels_obsolete_requests_and_bounds_total_wait() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let mut api = api_for(&listener);
        api.timeout = Duration::from_millis(300);
        let (requests, operations) = tokio::sync::watch::channel(None);
        let (events, mut incoming) = mpsc::channel(4);
        let worker = tokio::spawn(Arc::new(api).history_aggregates(operations, events));
        requests.send(Some(graph_history_request(1))).unwrap();
        let (_stalled, _) = accept_http_request(&listener).await;
        let mut next = graph_history_request(2);
        next.resolution = crate::history::Resolution::Minute;
        next.since = next.until - chrono::TimeDelta::milliseconds(120_001);
        requests.send(Some(next.clone())).unwrap();
        let (mut socket, _) = accept_http_request(&listener).await;
        respond_json(&mut socket, graph_response(&next, json!([]))).await;
        let Event::History(request, result) = timeout(Duration::from_secs(1), incoming.recv())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("Expected history");
        };
        assert_eq!(request.generation, 2);
        assert_eq!(request.resolution, crate::history::Resolution::Minute);
        assert!(result.unwrap().is_empty());
        assert!(incoming.try_recv().is_err());
        requests.send(Some(graph_history_request(3))).unwrap();
        let (_stalled, _) = accept_http_request(&listener).await;
        let Event::History(request, result) = timeout(Duration::from_secs(1), incoming.recv())
            .await
            .unwrap()
            .unwrap()
        else {
            panic!("Expected deadline");
        };
        assert_eq!(request.generation, 3);
        assert_eq!(result.unwrap_err(), "History request timed out.");
        requests.send(Some(graph_history_request(4))).unwrap();
        let (_stalled, _) = accept_http_request(&listener).await;
        requests.send(None).unwrap();
        drop(requests);
        timeout(Duration::from_secs(1), worker)
            .await
            .unwrap()
            .unwrap();
        assert!(incoming.recv().await.is_none());
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
        let (mut boundary_socket, boundary_headers) = accept_http_request(&listener).await;
        assert!(
            boundary_headers
                .starts_with("GET /api/v1/logs?direction=forward&min_level=DEBUG&limit=1 ")
        );
        boundary_socket
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
    async fn daemon_actions_require_valid_receipts_before_reporting_success() {
        let connection = |desired| {
            let mut value = serde_json::to_value(crate::tests::status().connection).unwrap();
            value["desired"] = json!(desired);
            value["adapter_address"] = json!("AA:BB:CC:DD:EE:FF");
            value
        };
        let logging = |configured, effective| {
            json!({
                "configured_level": configured, "effective_level": effective,
                "override_expires_at": null
            })
        };
        for (action, response, message) in [
            (
                "retry",
                connection("running"),
                Some("Reconnecting to station"),
            ),
            (
                "retry",
                connection("paused"),
                Some("Reconnecting to station"),
            ),
            ("retry", json!({}), None),
            ("retry", connection("invalid"), None),
            ("retry", json!({"desired":"running"}), None),
            (
                "pause",
                connection("paused"),
                Some("Station connection paused"),
            ),
            ("pause", connection("running"), None),
            ("pause", json!({}), None),
            (
                "resume",
                connection("running"),
                Some("Station connection resumed"),
            ),
            ("resume", connection("paused"), None),
            ("resume", json!({}), None),
            (
                "debug-on",
                logging("ERROR", "DEBUG"),
                Some("Log level changed to DEBUG"),
            ),
            ("debug-on", logging("INFO", "INFO"), None),
            ("debug-on", logging("INFO", "TRACE"), None),
            ("debug-on", json!({}), None),
            ("debug-on", json!({"effective_level":"DEBUG"}), None),
            (
                "debug-off",
                logging("WARNING", "WARNING"),
                Some("Log level changed to WARNING"),
            ),
            (
                "debug-off",
                logging("DEBUG", "DEBUG"),
                Some("Log level changed to DEBUG"),
            ),
            ("debug-off", logging("INFO", "DEBUG"), None),
            ("debug-off", json!({}), None),
            (
                "debug-off",
                json!({"configured_level":"INFO", "effective_level":"INFO", "override_expires_at":"2030-01-01T00:00:00Z"}),
                None,
            ),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = api_for(&listener);
            let (intent, path) = match action {
                "retry" => (Intent::Retry, "POST /api/v1/connection/retry HTTP/1.1"),
                "pause" => (Intent::Connection(false), "PUT /api/v1/connection HTTP/1.1"),
                "resume" => (Intent::Connection(true), "PUT /api/v1/connection HTTP/1.1"),
                "debug-on" => (
                    Intent::Debug(true),
                    "PUT /api/v1/runtime/log-level HTTP/1.1",
                ),
                "debug-off" => (
                    Intent::Debug(false),
                    "DELETE /api/v1/runtime/log-level HTTP/1.1",
                ),
                _ => unreachable!(),
            };
            let server = tokio::spawn(async move {
                let (mut socket, request) = accept_http_request(&listener).await;
                assert!(request.starts_with(path));
                let body = response.to_string();
                socket.write_all(format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                ).as_bytes()).await.unwrap();
                assert!(
                    timeout(Duration::from_millis(200), listener.accept())
                        .await
                        .is_err(),
                    "An invalid receipt must not replay the action"
                );
            });
            let (events, mut incoming) = mpsc::channel(4);
            let mut app = crate::app::App::new(false, Some(chrono_tz::UTC));
            app.update(Event::Status(Box::new(crate::tests::status())));
            let original = serde_json::to_value(app.status.as_ref().unwrap()).unwrap();
            app.pending = Some("daemon request".into());
            let feedback = timeout(Duration::from_secs(3), api.perform(intent, &events)).await;
            timeout(Duration::from_secs(3), server)
                .await
                .unwrap()
                .unwrap();
            let feedback = feedback.unwrap();
            if let Some(message) = message {
                assert_eq!(feedback.message, message, "action={action}");
                assert_eq!(feedback.severity, Severity::Info);
            } else {
                assert_eq!(
                    feedback.severity,
                    Severity::Error,
                    "action={action}, feedback={}",
                    feedback.message
                );
            }
            assert!(incoming.try_recv().is_err());
            app.update(Event::Finished(feedback));
            assert!(app.pending.is_none());
            assert_eq!(
                serde_json::to_value(app.status.as_ref().unwrap()).unwrap(),
                original
            );
        }
    }

    #[tokio::test]
    async fn daemon_restart_during_command_polling_preserves_the_new_snapshot() {
        const ID: &str = "6147f85c-53ef-42f4-b3f2-c15b071320a5";
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let api = Arc::new(api_for(&listener));
        let mut app = crate::app::App::new(false, Some(chrono_tz::UTC));
        app.update(Event::Status(Box::new(crate::tests::status())));
        let crate::app::Effect::Request(intent) = app.toggle(0) else {
            panic!("Expected an AC ON command");
        };
        let (polling, polled) = tokio::sync::oneshot::channel();
        let (restarted, restart) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, request) = accept_http_request(&listener).await;
            assert!(request.starts_with("PUT /api/v1/outputs/ac HTTP/1.1"));
            let body = json!({
                "schema_version": 1, "command_id": ID, "status": "accepted",
                "output": "ac", "requested_enabled": true, "reason_code": null
            })
            .to_string();
            socket.write_all(format!(
                "HTTP/1.1 202 Accepted\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ).as_bytes()).await.unwrap();
            drop(socket);
            let (mut socket, request) = accept_http_request(&listener).await;
            assert!(request.starts_with(&format!("GET /api/v1/commands/{ID} HTTP/1.1")));
            polling.send(()).unwrap();
            restart.await.unwrap();
            let body = json!({"schema_version": 1, "error": {
                "code": "command_not_found", "message": "Unknown command after restart"
            }})
            .to_string();
            socket.write_all(format!(
                "HTTP/1.1 404 Not Found\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            ).as_bytes()).await.unwrap();
            assert!(
                timeout(Duration::from_millis(300), listener.accept())
                    .await
                    .is_err(),
                "An old command must not be replayed against the new daemon"
            );
        });
        let (events, mut incoming) = mpsc::channel(4);
        let worker = tokio::spawn(async move { api.perform(intent, &events).await });
        let admission = timeout(Duration::from_secs(3), incoming.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(matches!(admission, Event::Command(_)));
        app.update(admission);
        timeout(Duration::from_secs(3), polled)
            .await
            .unwrap()
            .unwrap();
        let mut next = crate::tests::status();
        next.server_instance_id = "69be0c88-45b3-479b-a6e2-867579f98e73".into();
        next.connection.session_id = Some("new-session".into());
        next.controls.outputs_revision = 18;
        let sample = next.telemetry.sample.as_mut().unwrap();
        sample.segment_id = "new-segment".into();
        sample.sequence = 1;
        sample.ac_enabled = true;
        let new_snapshot = serde_json::to_value(&next).unwrap();
        app.update(Event::Status(Box::new(next)));
        assert!(app.graph.points.is_empty());
        assert_eq!(
            app.status
                .as_ref()
                .unwrap()
                .telemetry
                .sample
                .as_ref()
                .unwrap()
                .segment_id,
            "new-segment"
        );
        assert!(matches!(app.toggle(0), crate::app::Effect::None));
        restarted.send(()).unwrap();
        let feedback = timeout(Duration::from_secs(3), worker)
            .await
            .unwrap()
            .unwrap();
        timeout(Duration::from_secs(3), server)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            feedback.message,
            "Command outcome uncertain; check station before retrying"
        );
        assert_eq!(feedback.severity, Severity::Warning);
        assert!(incoming.try_recv().is_err());
        app.update(Event::Finished(feedback));
        assert!(app.pending.is_none());
        assert_eq!(
            serde_json::to_value(app.status.as_ref().unwrap()).unwrap(),
            new_snapshot
        );
        let crate::app::Effect::Request(Intent::Output {
            snapshot, enabled, ..
        }) = app.toggle(0)
        else {
            panic!("The new daemon's fresh snapshot should enable another user command");
        };
        assert!(!enabled);
        assert_eq!(
            snapshot.server_instance_id,
            new_snapshot["server_instance_id"]
        );
        assert_eq!(snapshot.controls.outputs_revision, 18);
    }

    #[tokio::test]
    async fn state_conflicts_report_a_refreshable_failure_without_replaying_the_command() {
        const ID: &str = "6147f85c-53ef-42f4-b3f2-c15b071320a5";
        for admitted in [false, true] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = api_for(&listener);
            let mut app = crate::app::App::new(false, Some(chrono_tz::UTC));
            app.update(Event::Status(Box::new(crate::tests::status())));
            let crate::app::Effect::Request(intent) = app.toggle(0) else {
                panic!("Expected an AC ON command");
            };
            let server = tokio::spawn(async move {
                let responses = if admitted {
                    vec![
                        (
                            "PUT /api/v1/outputs/ac HTTP/1.1".to_owned(),
                            "202 Accepted",
                            json!({
                                "schema_version": 1, "command_id": ID, "status": "accepted",
                                "output": "ac", "requested_enabled": true, "reason_code": null
                            }),
                        ),
                        (
                            format!("GET /api/v1/commands/{ID} HTTP/1.1"),
                            "200 OK",
                            json!({
                                "schema_version": 1, "command_id": ID, "status": "failed",
                                "output": "ac", "requested_enabled": true, "reason_code": "state_conflict"
                            }),
                        ),
                    ]
                } else {
                    vec![(
                        "PUT /api/v1/outputs/ac HTTP/1.1".to_owned(),
                        "409 Conflict",
                        json!({
                            "error": {"code": "state_conflict", "message": "internal detail must not be shown"}
                        }),
                    )]
                };
                for (path, status, response) in responses {
                    let (mut socket, request) = accept_http_request(&listener).await;
                    assert!(request.starts_with(&path));
                    let body = response.to_string();
                    socket.write_all(format!(
                        "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    ).as_bytes()).await.unwrap();
                }
                assert!(
                    timeout(Duration::from_millis(300), listener.accept())
                        .await
                        .is_err(),
                    "A state conflict must not replay the PUT or continue polling"
                );
            });
            let (events, mut incoming) = mpsc::channel(4);
            let feedback = timeout(Duration::from_secs(3), api.perform(intent, &events)).await;
            timeout(Duration::from_secs(3), server)
                .await
                .unwrap()
                .unwrap();
            let feedback = feedback.unwrap();
            assert_eq!(
                feedback.message,
                if admitted {
                    "AC ON failed: station state changed; try again"
                } else {
                    "Command failed: station state changed; try again"
                }
            );
            assert_eq!(feedback.severity, Severity::Error);
            if admitted {
                let Event::Command(command) = incoming.try_recv().unwrap() else {
                    panic!("Expected the admission event");
                };
                app.update(Event::Command(command));
            }
            assert!(incoming.try_recv().is_err());
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
        }
    }

    #[tokio::test]
    async fn command_polling_preserves_the_admitted_output_intention() {
        const ID: &str = "6147f85c-53ef-42f4-b3f2-c15b071320a5";
        for (output, enabled, status, id, valid) in [
            ("ac", true, "confirmed", ID, true),
            ("dc", true, "confirmed", ID, false),
            ("light", true, "confirmed", ID, false),
            ("ac", false, "confirmed", ID, false),
            ("dc", true, "sent", ID, false),
            ("ac", false, "sent", ID, false),
            (
                "ac",
                true,
                "confirmed",
                "69be0c88-45b3-479b-a6e2-867579f98e73",
                false,
            ),
        ] {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = api_for(&listener);
            let mut app = crate::app::App::new(false, Some(chrono_tz::UTC));
            app.update(Event::Status(Box::new(crate::tests::status())));
            let crate::app::Effect::Request(intent) = app.toggle(0) else {
                panic!("Expected an AC ON command");
            };
            let server = tokio::spawn(async move {
                for (path, response) in [
                    (
                        "PUT /api/v1/outputs/ac HTTP/1.1".to_owned(),
                        json!({
                            "schema_version": 1, "command_id": ID, "status": "accepted",
                            "output": "ac", "requested_enabled": true, "reason_code": null
                        }),
                    ),
                    (
                        format!("GET /api/v1/commands/{ID} HTTP/1.1"),
                        json!({
                            "schema_version": 1, "command_id": id, "status": status,
                            "output": output, "requested_enabled": enabled, "reason_code": null
                        }),
                    ),
                ] {
                    let (mut socket, request) = accept_http_request(&listener).await;
                    assert!(request.starts_with(&path));
                    let body = response.to_string();
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
                }
                assert!(
                    timeout(Duration::from_millis(300), listener.accept())
                        .await
                        .is_err(),
                    "A contradictory result must not be followed or replayed"
                );
            });
            let (events, mut incoming) = mpsc::channel(4);
            let feedback = timeout(Duration::from_secs(3), api.perform(intent, &events)).await;
            timeout(Duration::from_secs(3), server)
                .await
                .unwrap()
                .unwrap();
            let feedback = feedback.unwrap();
            assert_eq!(
                feedback.message,
                if valid {
                    "AC ON confirmed"
                } else {
                    "Command outcome uncertain; check station before retrying"
                },
                "poll output={output}, enabled={enabled}, status={status}, id={id}"
            );
            assert_eq!(
                feedback.severity,
                if valid {
                    Severity::Success
                } else {
                    Severity::Warning
                }
            );
            let Event::Command(admitted) = incoming.try_recv().unwrap() else {
                panic!("Expected only the valid admission event");
            };
            assert_eq!(admitted.command_id, ID);
            assert_eq!(admitted.output, "ac");
            assert!(admitted.requested_enabled);
            assert!(incoming.try_recv().is_err());
            app.update(Event::Command(admitted));
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
                    .ac_enabled,
                "Command feedback must never replace station telemetry"
            );
        }
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
    #[tokio::test]
    async fn telegram_test_requires_confirmed_receipt_and_never_replays() {
        for body in [
            json!({"status":"sent"}),
            json!({"status":"queued"}),
            json!({}),
        ] {
            let valid = body["status"] == "sent";
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = api_for(&listener);
            let server = tokio::spawn(async move {
                let (mut socket, request) = accept_http_request(&listener).await;
                assert!(request.starts_with("POST /api/v1/notifications/telegram/test HTTP/1.1"));
                respond_json(&mut socket, body).await;
                assert!(
                    timeout(Duration::from_millis(200), listener.accept())
                        .await
                        .is_err()
                );
            });
            let (events, _) = mpsc::channel(4);
            let feedback = api.perform(Intent::TestTelegram, &events).await;
            assert_eq!(
                feedback.severity,
                if valid {
                    Severity::Success
                } else {
                    Severity::Error
                }
            );
            server.await.unwrap();
        }
    }

    #[tokio::test]
    async fn settings_loader_validates_schema_and_save_requires_matching_receipt_without_replay() {
        for body in [
            json!({"schema_version":1,"graph_interval_seconds":60}),
            json!({"schema_version":2,"graph_interval_seconds":60}),
            json!({"schema_version":1,"graph_interval_seconds":11}),
            json!({"schema_version":1,"graph_interval_seconds":"60"}),
        ] {
            let valid = body["schema_version"] == 1 && body["graph_interval_seconds"] == 60;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = Arc::new(api_for(&listener));
            let server = tokio::spawn(async move {
                let (mut socket, request) = accept_http_request(&listener).await;
                assert!(request.starts_with("GET /api/v1/settings HTTP/1.1"));
                let mut complete =
                    serde_json::to_value(crate::settings::Settings::default()).unwrap();
                for (key, value) in body.as_object().unwrap() {
                    complete[key] = value.clone();
                }
                respond_json(&mut socket, complete).await;
            });
            let (events, mut incoming) = mpsc::channel(8);
            let task = tokio::spawn(api.load_settings(events));
            let event = timeout(Duration::from_secs(3), incoming.recv())
                .await
                .unwrap()
                .unwrap();
            assert!(matches!(event, Event::Settings(_)) == valid);
            if !valid {
                assert!(matches!(event, Event::SettingsUnavailable));
            }
            task.abort();
            server.await.unwrap();
        }
        for body in [
            json!({"schema_version":1,"graph_interval_seconds":3600}),
            json!({"schema_version":1,"graph_interval_seconds":60}),
            json!({"schema_version":2,"graph_interval_seconds":3600}),
        ] {
            let valid = body["schema_version"] == 1 && body["graph_interval_seconds"] == 3600;
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let api = api_for(&listener);
            let server = tokio::spawn(async move {
                let (mut socket, request) = accept_http_request(&listener).await;
                assert!(request.starts_with("PUT /api/v1/settings HTTP/1.1"));
                let mut complete =
                    serde_json::to_value(crate::settings::Settings::default()).unwrap();
                for (key, value) in body.as_object().unwrap() {
                    complete[key] = value.clone();
                }
                respond_json(&mut socket, complete).await;
                assert!(
                    timeout(Duration::from_millis(200), listener.accept())
                        .await
                        .is_err()
                );
            });
            let (events, mut incoming) = mpsc::channel(8);
            let feedback = api
                .perform(
                    Intent::SaveSettings(crate::settings::Settings {
                        graph_interval_seconds: 3600,
                        ..crate::settings::Settings::default()
                    }),
                    &events,
                )
                .await;
            if valid {
                assert_eq!(feedback.message, "Settings saved");
                assert!(matches!(incoming.try_recv().unwrap(), Event::Settings(_)));
            } else {
                assert_eq!(feedback.severity, Severity::Error);
                assert!(incoming.try_recv().is_err());
            }
            server.await.unwrap();
        }
    }
}
