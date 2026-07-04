use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::Client;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::time::{Instant, sleep};

const DEFAULT_LOCAL_BINDING: &str = "127.0.0.1";
const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 5;
const DEFAULT_COMMAND_POLL_MS: u64 = 250;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BridgeCommandKind {
    PushLocalTree,
    PreviewPull,
    ApplyPull,
    ForcePull,
}

impl BridgeCommandKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::PushLocalTree => "push local tree to Studio",
            Self::PreviewPull => "preview pull from Studio",
            Self::ApplyPull => "apply pull from Studio",
            Self::ForcePull => "force pull from Studio",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeCommandEnvelope {
    pub id: u64,
    pub kind: BridgeCommandKind,
    pub requested_at_ms: u64,
    pub target_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_place_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_place_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeCommandResult {
    pub command_id: u64,
    pub kind: BridgeCommandKind,
    pub target_session_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_place_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_place_id: Option<u64>,
    pub ok: bool,
    pub summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    pub finished_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeSessionSnapshot {
    pub session_id: String,
    pub connected: bool,
    pub selected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bridge_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    pub last_seen_ms: u64,
}

impl BridgeSessionSnapshot {
    pub fn label(&self) -> String {
        let place = self
            .place_name
            .clone()
            .unwrap_or_else(|| "unknown place".to_string());
        match self.place_id {
            Some(place_id) => format!(
                "{place} (place {place_id}, session {})",
                short_session_id(&self.session_id)
            ),
            None => format!("{place} (session {})", short_session_id(&self.session_id)),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct BridgePlacesSnapshot {
    pub connected: bool,
    pub connected_sessions: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub target_session_id: Option<String>,
    pub selection_required: bool,
    pub sessions: Vec<BridgeSessionSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeCommandState {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pending_command: Option<BridgeCommandEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active_command: Option<BridgeCommandEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_result: Option<BridgeCommandResult>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueueBridgeCommandRequest {
    pub kind: BridgeCommandKind,
    #[serde(default)]
    pub target_session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct QueueBridgeCommandResponse {
    pub queued: bool,
    pub command: BridgeCommandEnvelope,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SelectBridgeSessionRequest {
    #[serde(default)]
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct SelectBridgeSessionResponse {
    pub selected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub selected_session_id: Option<String>,
    pub message: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct BridgeTargetSelector {
    pub session_id: Option<String>,
    pub place_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BridgeCommandRunReport {
    pub command: BridgeCommandEnvelope,
    pub result: BridgeCommandResult,
    pub waited_ms: u64,
}

#[derive(Debug, Clone)]
pub struct NyjoControlClient {
    base_url: String,
    http: Client,
}

#[derive(Debug, Deserialize)]
struct ApiEnvelope<T> {
    ok: bool,
    data: Option<T>,
    error: Option<ApiErrorPayload>,
}

#[derive(Debug, Deserialize)]
struct ApiErrorPayload {
    message: String,
}

impl NyjoControlClient {
    pub fn new(port: u16) -> Result<Self> {
        Self::with_base_url(format!("http://{DEFAULT_LOCAL_BINDING}:{port}"))
    }

    pub fn with_base_url(base_url: impl Into<String>) -> Result<Self> {
        let http = Client::builder()
            .timeout(Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS))
            .build()
            .context("failed to build local nyjo control client")?;

        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            http,
        })
    }

    pub async fn list_places(&self) -> Result<BridgePlacesSnapshot> {
        self.get("/api/control/places").await
    }

    pub async fn command_state(&self) -> Result<BridgeCommandState> {
        self.get("/api/control/command-state").await
    }

    pub async fn select_session(
        &self,
        session_id: Option<String>,
    ) -> Result<SelectBridgeSessionResponse> {
        self.post(
            "/api/control/select-session",
            &SelectBridgeSessionRequest { session_id },
        )
        .await
    }

    pub async fn queue_command(
        &self,
        kind: BridgeCommandKind,
        target_session_id: Option<String>,
    ) -> Result<QueueBridgeCommandResponse> {
        self.post(
            "/api/control/command",
            &QueueBridgeCommandRequest {
                kind,
                target_session_id,
            },
        )
        .await
    }

    pub async fn wait_for_command_result(
        &self,
        command_id: u64,
        timeout: Duration,
    ) -> Result<BridgeCommandResult> {
        let started = Instant::now();

        loop {
            let state = self.command_state().await?;

            if let Some(result) = state.last_result.clone()
                && result.command_id == command_id
            {
                return Ok(result);
            }

            if started.elapsed() >= timeout {
                let visibility = command_visibility(&state, command_id);
                bail!(
                    "timed out waiting {} ms for Studio command #{} ({visibility})",
                    timeout.as_millis(),
                    command_id
                );
            }

            sleep(Duration::from_millis(DEFAULT_COMMAND_POLL_MS)).await;
        }
    }

    pub async fn run_command(
        &self,
        kind: BridgeCommandKind,
        target_session_id: Option<String>,
        timeout: Duration,
    ) -> Result<BridgeCommandRunReport> {
        let queued = self.queue_command(kind, target_session_id).await?;
        let started = Instant::now();
        let result = self
            .wait_for_command_result(queued.command.id, timeout)
            .await?;

        Ok(BridgeCommandRunReport {
            command: queued.command,
            result,
            waited_ms: started.elapsed().as_millis() as u64,
        })
    }

    async fn get<T>(&self, path: &str) -> Result<T>
    where
        T: DeserializeOwned,
    {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("failed to GET {url}"))?;

        parse_api_response(response, path).await
    }

    async fn post<T, B>(&self, path: &str, body: &B) -> Result<T>
    where
        T: DeserializeOwned,
        B: Serialize + ?Sized,
    {
        let url = format!("{}{}", self.base_url, path);
        let response = self
            .http
            .post(&url)
            .json(body)
            .send()
            .await
            .with_context(|| format!("failed to POST {url}"))?;

        parse_api_response(response, path).await
    }
}

pub fn resolve_target_session_id(
    places: &BridgePlacesSnapshot,
    selector: &BridgeTargetSelector,
) -> Result<Option<String>> {
    if let Some(session_id) = selector
        .session_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        return Ok(Some(session_id.to_string()));
    }

    let Some(place_id) = selector.place_id else {
        return Ok(None);
    };

    let mut matching = places
        .sessions
        .iter()
        .filter(|session| session.connected && session.place_id == Some(place_id));
    let first = matching.next();
    let second = matching.next();

    match (first, second) {
        (Some(session), None) => Ok(Some(session.session_id.clone())),
        (Some(_), Some(_)) => {
            bail!("multiple connected Studio sessions match place {place_id}; use --session-id")
        }
        _ => bail!("no connected Studio session matches place {place_id}"),
    }
}

async fn parse_api_response<T>(response: reqwest::Response, path: &str) -> Result<T>
where
    T: DeserializeOwned,
{
    let status = response.status();
    let body = response
        .text()
        .await
        .with_context(|| format!("failed to read response body from {path}"))?;
    let envelope = serde_json::from_str::<ApiEnvelope<T>>(&body)
        .with_context(|| format!("failed to parse JSON response from {path}: {body}"))?;

    if status.is_success() && envelope.ok {
        return envelope
            .data
            .with_context(|| format!("response from {path} did not include data"));
    }

    let message = envelope
        .error
        .map(|error| error.message)
        .unwrap_or_else(|| format!("request to {path} failed with HTTP {status}"));
    bail!(message)
}

fn command_visibility(state: &BridgeCommandState, command_id: u64) -> &'static str {
    if state
        .active_command
        .as_ref()
        .is_some_and(|command| command.id == command_id)
    {
        return "active";
    }

    if state
        .pending_command
        .as_ref()
        .is_some_and(|command| command.id == command_id)
    {
        return "pending";
    }

    "not visible"
}

fn short_session_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}

#[cfg(test)]
mod tests {
    use anyhow::Result;

    use super::{
        BridgePlacesSnapshot, BridgeSessionSnapshot, BridgeTargetSelector,
        resolve_target_session_id,
    };

    fn snapshot_with_sessions(sessions: Vec<BridgeSessionSnapshot>) -> BridgePlacesSnapshot {
        BridgePlacesSnapshot {
            connected: sessions.iter().any(|session| session.connected),
            connected_sessions: sessions.iter().filter(|session| session.connected).count(),
            selected_session_id: None,
            target_session_id: None,
            selection_required: false,
            sessions,
        }
    }

    #[test]
    fn resolves_connected_session_by_place_id() -> Result<()> {
        let places = snapshot_with_sessions(vec![
            BridgeSessionSnapshot {
                session_id: "session-a".to_string(),
                connected: true,
                selected: false,
                bridge_version: None,
                place_name: Some("Alpha".to_string()),
                place_id: Some(101),
                status: None,
                last_seen_ms: 1,
            },
            BridgeSessionSnapshot {
                session_id: "session-b".to_string(),
                connected: true,
                selected: false,
                bridge_version: None,
                place_name: Some("Beta".to_string()),
                place_id: Some(202),
                status: None,
                last_seen_ms: 2,
            },
        ]);

        let resolved = resolve_target_session_id(
            &places,
            &BridgeTargetSelector {
                session_id: None,
                place_id: Some(202),
            },
        )?;

        assert_eq!(resolved.as_deref(), Some("session-b"));
        Ok(())
    }

    #[test]
    fn rejects_ambiguous_place_id_selection() {
        let places = snapshot_with_sessions(vec![
            BridgeSessionSnapshot {
                session_id: "session-a".to_string(),
                connected: true,
                selected: false,
                bridge_version: None,
                place_name: Some("Alpha".to_string()),
                place_id: Some(101),
                status: None,
                last_seen_ms: 1,
            },
            BridgeSessionSnapshot {
                session_id: "session-b".to_string(),
                connected: true,
                selected: false,
                bridge_version: None,
                place_name: Some("Alpha".to_string()),
                place_id: Some(101),
                status: None,
                last_seen_ms: 2,
            },
        ]);

        let error = resolve_target_session_id(
            &places,
            &BridgeTargetSelector {
                session_id: None,
                place_id: Some(101),
            },
        )
        .expect_err("expected duplicate place id selection to be rejected");

        assert!(
            error
                .to_string()
                .contains("multiple connected Studio sessions")
        );
    }
}
