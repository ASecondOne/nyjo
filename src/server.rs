use std::collections::BTreeMap;
use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use axum::extract::{DefaultBodyLimit, State};
use axum::http::StatusCode;
use axum::response::Html;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::backup::{
    BackupRecord, StudioBackupIdentity, create_local_project_backup, create_studio_tree_backup,
    read_studio_tree_backup,
};
use crate::control::{
    BridgeCommandEnvelope, BridgeCommandKind, BridgeCommandResult, BridgeCommandState,
    BridgePlacesSnapshot, BridgeSessionSnapshot, QueueBridgeCommandRequest,
    QueueBridgeCommandResponse, SelectBridgeSessionRequest, SelectBridgeSessionResponse,
};
use crate::model::{InstanceNode, NodeMetadata};
use crate::parser::{
    ROOT_SERVICE_CLASSES, metadata_path_for_target, read_embedded_text_payload, scan_project,
};

#[derive(Clone)]
struct AppState {
    root: PathBuf,
    dashboard: Arc<Mutex<DashboardState>>,
}

const LOGICAL_NODE_FILE_SUFFIXES: &[&str] = &[
    ".server.lua",
    ".client.lua",
    ".model.json",
    ".instance.json",
    ".service.json",
    ".worldmodel",
    ".model",
    ".part",
    ".screengui",
    ".canvasgroup",
    ".scrollingframe",
    ".surfacegui",
    ".billboardgui",
    ".frame",
    ".textlabel",
    ".textbutton",
    ".textbox",
    ".imagelabel",
    ".imagebutton",
    ".uilistlayout",
    ".uigridlayout",
    ".uipadding",
    ".uicorner",
    ".uistroke",
    ".uishadow",
    ".texture",
    ".decal",
    ".stringvalue",
    ".numbervalue",
    ".intvalue",
    ".boolvalue",
    ".color3value",
    ".vector3value",
    ".folder",
    ".lua",
    ".rf",
    ".re",
    ".bf",
    ".be",
];

const DASHBOARD_HTML: &str = include_str!("../assets/dashboard.html");
const DIRECTORY_HEADER_FILE_NAME: &str = ".nyjo";
const LOG_BUFFER_LIMIT: usize = 200;
const PLUGIN_CONNECTION_TIMEOUT_MS: u64 = 5_000;
const PLUGIN_SESSION_RETENTION_MS: u64 = 60_000;
const STUDIO_SYNC_MAX_BODY_BYTES: usize = 128 * 1024 * 1024;

#[derive(Debug, Default)]
struct DashboardState {
    next_command_id: u64,
    pending_command: Option<BridgeCommandEnvelope>,
    active_command: Option<BridgeCommandEnvelope>,
    sessions: BTreeMap<String, BridgeSessionRecord>,
    selected_session_id: Option<String>,
    last_result: Option<BridgeCommandResult>,
    logs: Vec<DashboardLogEntry>,
}

#[derive(Debug, Clone)]
struct BridgeSessionRecord {
    session_id: String,
    bridge_version: Option<String>,
    place_name: Option<String>,
    place_id: Option<u64>,
    status: Option<String>,
    last_seen_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DashboardLogEntry {
    timestamp_ms: u64,
    level: String,
    message: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DashboardPluginSnapshot {
    connected: bool,
    connected_sessions: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    selected_session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    target_session_id: Option<String>,
    selection_required: bool,
    sessions: Vec<BridgeSessionSnapshot>,
    #[serde(skip_serializing_if = "Option::is_none")]
    bridge_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_command: Option<BridgeCommandEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_command: Option<BridgeCommandEnvelope>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DashboardSnapshot {
    server: Value,
    plugin: DashboardPluginSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_result: Option<BridgeCommandResult>,
    logs: Vec<DashboardLogEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginHeartbeatRequest {
    session_id: String,
    #[serde(default)]
    bridge_version: Option<String>,
    #[serde(default)]
    place_name: Option<String>,
    #[serde(default)]
    place_id: Option<u64>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginPollRequest {
    session_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginCommandResultRequest {
    command_id: u64,
    session_id: String,
    ok: bool,
    summary: String,
    #[serde(default)]
    detail: Option<String>,
    #[serde(default)]
    data: Option<Value>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct CreateRequest {
    path: String,
    #[serde(default)]
    node_type: Option<String>,
    #[serde(default)]
    contents: Option<String>,
    #[serde(default)]
    metadata: Option<NodeMetadata>,
    #[serde(default)]
    overwrite: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct UpdateRequest {
    path: String,
    #[serde(default)]
    contents: Option<String>,
    #[serde(default)]
    metadata: Option<NodeMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DeleteRequest {
    path: String,
    #[serde(default)]
    recursive: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StudioSyncRequest {
    tree: StudioNode,
    #[serde(default)]
    mode: StudioSyncMode,
    #[serde(default)]
    force: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StudioBackupCreateRequest {
    session_id: String,
    #[serde(default)]
    place_name: Option<String>,
    #[serde(default)]
    place_id: Option<u64>,
    #[serde(default)]
    reason: Option<String>,
    tree: StudioNode,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StudioBackupReadRequest {
    backup_id: String,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct StudioNode {
    name: String,
    class_name: String,
    #[serde(default)]
    children: Vec<StudioNode>,
    #[serde(default, deserialize_with = "deserialize_map_or_empty_array")]
    properties: std::collections::BTreeMap<String, Value>,
    #[serde(default, deserialize_with = "deserialize_map_or_empty_array")]
    attributes: std::collections::BTreeMap<String, Value>,
    #[serde(default)]
    tags: Vec<String>,
    #[serde(default)]
    source: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum StudioSyncMode {
    Preview,
    #[default]
    Apply,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct StudioSyncStats {
    nodes_seen: usize,
    supported_services: usize,
    directories_planned: usize,
    files_planned: usize,
    metadata_planned: usize,
    removals_planned: usize,
    directories_written: usize,
    files_written: usize,
    metadata_written: usize,
    removals_applied: usize,
    unchanged: usize,
    skipped_nodes: usize,
    conflicts: usize,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StudioSyncOperation {
    action: &'static str,
    kind: &'static str,
    path: String,
    detail: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StudioSyncConflict {
    kind: &'static str,
    path: String,
    message: String,
}

#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
struct StudioSyncPlan {
    operations: Vec<StudioSyncOperation>,
    conflicts: Vec<StudioSyncConflict>,
    stats: StudioSyncStats,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct StudioSyncResponse {
    mode: StudioSyncMode,
    force: bool,
    applied: bool,
    blocked: bool,
    plan: StudioSyncPlan,
    #[serde(skip_serializing_if = "Option::is_none")]
    backup: Option<BackupRecord>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tree: Option<InstanceNode>,
}

#[derive(Debug)]
enum PendingWrite {
    EnsureDirectory {
        path: PathBuf,
    },
    WriteFile {
        path: PathBuf,
        contents: String,
        kind: &'static str,
    },
    RemoveFile {
        path: PathBuf,
    },
    RemoveDirectory {
        path: PathBuf,
    },
}

impl BridgeSessionRecord {
    fn connected(&self) -> bool {
        now_ms().saturating_sub(self.last_seen_ms) <= PLUGIN_CONNECTION_TIMEOUT_MS
    }

    fn label(&self) -> String {
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

impl DashboardState {
    fn queue_command(
        &mut self,
        kind: BridgeCommandKind,
        requested_target_session_id: Option<&str>,
    ) -> Result<BridgeCommandEnvelope> {
        self.prune_sessions();
        if !self.plugin_connected() {
            bail!("Studio bridge is not connected; open the Nyjo plugin in Studio first");
        }

        if let Some(active) = &self.active_command {
            bail!(
                "Studio bridge is busy running command #{} ({})",
                active.id,
                active.kind.label()
            );
        }

        if let Some(pending) = &self.pending_command {
            bail!(
                "command #{} ({}) is already queued",
                pending.id,
                pending.kind.label()
            );
        }

        let target_session_id = self.resolve_target_session_id(requested_target_session_id)?;
        let target_session = self.sessions.get(&target_session_id).with_context(|| {
            format!("target Studio bridge session {target_session_id} was not found")
        })?;
        let target_session_label = target_session.label();
        let target_place_name = target_session.place_name.clone();
        let target_place_id = target_session.place_id;

        self.next_command_id += 1;
        let command = BridgeCommandEnvelope {
            id: self.next_command_id,
            kind,
            requested_at_ms: now_ms(),
            target_session_id,
            target_place_name,
            target_place_id,
        };
        self.log(
            "info",
            format!(
                "Queued command #{}: {} -> {}",
                command.id,
                kind.label(),
                target_session_label
            ),
        );
        self.pending_command = Some(command.clone());
        Ok(command)
    }

    fn take_pending_command(&mut self, session_id: &str) -> Option<BridgeCommandEnvelope> {
        self.prune_sessions();
        if self.active_command.is_some() {
            return None;
        }

        if self
            .pending_command
            .as_ref()
            .is_none_or(|command| command.target_session_id != session_id)
        {
            return None;
        }

        let command = self.pending_command.take()?;
        self.log(
            "info",
            format!(
                "Dispatched command #{} to {}",
                command.id,
                self.describe_command_target(&command)
            ),
        );
        self.active_command = Some(command.clone());
        Some(command)
    }

    fn update_plugin_heartbeat(&mut self, heartbeat: PluginHeartbeatRequest) {
        self.prune_sessions();
        let session_id = heartbeat.session_id;
        let was_connected = self
            .sessions
            .get(&session_id)
            .is_some_and(BridgeSessionRecord::connected);
        let record = BridgeSessionRecord {
            session_id: session_id.clone(),
            bridge_version: heartbeat.bridge_version,
            place_name: heartbeat.place_name,
            place_id: heartbeat.place_id,
            status: heartbeat.status,
            last_seen_ms: now_ms(),
        };

        if !was_connected {
            self.log(
                "info",
                format!("Studio bridge connected from {}", record.label()),
            );
        }

        self.sessions.insert(session_id, record);
    }

    fn finish_command(
        &mut self,
        result: PluginCommandResultRequest,
    ) -> Result<BridgeCommandResult> {
        self.prune_sessions();
        let active = self
            .active_command
            .as_ref()
            .with_context(|| "no active Studio bridge command to finish")?;

        if active.id != result.command_id {
            bail!(
                "received result for command #{} but active command is #{}",
                result.command_id,
                active.id
            );
        }

        if active.target_session_id != result.session_id {
            bail!(
                "received result from session {} but active command #{} belongs to session {}",
                result.session_id,
                active.id,
                active.target_session_id
            );
        }

        let active = self
            .active_command
            .take()
            .with_context(|| "no active Studio bridge command to finish")?;

        let record = BridgeCommandResult {
            command_id: result.command_id,
            kind: active.kind,
            target_session_id: active.target_session_id.clone(),
            target_place_name: active.target_place_name.clone(),
            target_place_id: active.target_place_id,
            ok: result.ok,
            summary: result.summary,
            detail: result.detail,
            data: result.data,
            finished_at_ms: now_ms(),
        };

        let level = if record.ok { "info" } else { "error" };
        self.log(
            level,
            format!(
                "Studio bridge finished command #{} ({}) on {}: {}",
                record.command_id,
                record.kind.label(),
                self.describe_result_target(&record),
                record.summary
            ),
        );
        self.last_result = Some(record.clone());
        Ok(record)
    }

    fn snapshot(&self, root: &Path) -> DashboardSnapshot {
        let places = self.places_snapshot();
        let command_state = self.command_state_snapshot();
        let selected_session = self
            .selected_session_id
            .as_deref()
            .and_then(|session_id| self.sessions.get(session_id));

        DashboardSnapshot {
            server: json!({
                "version": env!("CARGO_PKG_VERSION"),
                "binding": "127.0.0.1",
                "projectRoot": root.display().to_string(),
                "dashboardPath": "/"
            }),
            plugin: DashboardPluginSnapshot {
                connected: places.connected,
                connected_sessions: places.connected_sessions,
                selected_session_id: places.selected_session_id,
                target_session_id: places.target_session_id,
                selection_required: places.selection_required,
                sessions: places.sessions,
                bridge_version: selected_session.and_then(|session| session.bridge_version.clone()),
                status: selected_session.and_then(|session| session.status.clone()),
                last_seen_ms: selected_session.map(|session| session.last_seen_ms),
                pending_command: command_state.pending_command,
                active_command: command_state.active_command,
            },
            last_result: self.last_result.clone(),
            logs: self.logs.clone(),
        }
    }

    fn places_snapshot(&self) -> BridgePlacesSnapshot {
        let sessions = self.session_snapshots();
        let connected_sessions = sessions.iter().filter(|session| session.connected).count();
        let target_session_id = self.resolved_target_session_id();

        BridgePlacesSnapshot {
            connected: self.plugin_connected(),
            connected_sessions,
            selected_session_id: self.selected_session_id.clone(),
            target_session_id: target_session_id.clone(),
            selection_required: connected_sessions > 1 && target_session_id.is_none(),
            sessions,
        }
    }

    fn command_state_snapshot(&self) -> BridgeCommandState {
        BridgeCommandState {
            pending_command: self.pending_command.clone(),
            active_command: self.active_command.clone(),
            last_result: self.last_result.clone(),
        }
    }

    fn session_snapshots(&self) -> Vec<BridgeSessionSnapshot> {
        let mut sessions: Vec<_> = self
            .sessions
            .values()
            .map(|session| BridgeSessionSnapshot {
                session_id: session.session_id.clone(),
                connected: session.connected(),
                selected: self.selected_session_id.as_deref() == Some(session.session_id.as_str()),
                bridge_version: session.bridge_version.clone(),
                place_name: session.place_name.clone(),
                place_id: session.place_id,
                status: session.status.clone(),
                last_seen_ms: session.last_seen_ms,
            })
            .collect();
        sessions.sort_by(|left, right| {
            right
                .connected
                .cmp(&left.connected)
                .then_with(|| right.last_seen_ms.cmp(&left.last_seen_ms))
                .then_with(|| left.place_name.cmp(&right.place_name))
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        sessions
    }

    fn plugin_connected(&self) -> bool {
        self.sessions.values().any(BridgeSessionRecord::connected)
    }

    fn set_selected_session(&mut self, session_id: Option<String>) -> Result<()> {
        self.prune_sessions();
        match session_id
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            Some(session_id) => {
                let session = self
                    .sessions
                    .get(session_id)
                    .with_context(|| format!("Studio bridge session {session_id} was not found"))?;
                let session_label = session.label();
                self.selected_session_id = Some(session_id.to_string());
                self.log(
                    "info",
                    format!("Selected Studio bridge target: {session_label}"),
                );
            }
            None => {
                self.selected_session_id = None;
                self.log("info", "Cleared Studio bridge target selection".to_string());
            }
        }
        Ok(())
    }

    fn resolve_target_session_id(&self, requested: Option<&str>) -> Result<String> {
        if let Some(session_id) = requested
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or(self.selected_session_id.as_deref())
        {
            let session = self
                .sessions
                .get(session_id)
                .with_context(|| format!("Studio bridge session {session_id} was not found"))?;
            if !session.connected() {
                bail!("selected Studio bridge session is offline; choose another connected place");
            }
            return Ok(session_id.to_string());
        }

        let mut connected = self
            .sessions
            .values()
            .filter(|session| session.connected())
            .map(|session| session.session_id.as_str());
        let first = connected.next();
        let second = connected.next();
        match (first, second) {
            (Some(session_id), None) => Ok(session_id.to_string()),
            (Some(_), Some(_)) => {
                bail!(
                    "multiple Studio places are connected; choose a target place in the dashboard first"
                )
            }
            _ => bail!("Studio bridge is not connected; open the Nyjo plugin in Studio first"),
        }
    }

    fn resolved_target_session_id(&self) -> Option<String> {
        self.resolve_target_session_id(None).ok()
    }

    fn describe_command_target(&self, command: &BridgeCommandEnvelope) -> String {
        if let Some(place_name) = &command.target_place_name {
            return match command.target_place_id {
                Some(place_id) => format!(
                    "{place_name} (place {place_id}, session {})",
                    short_session_id(&command.target_session_id)
                ),
                None => format!(
                    "{place_name} (session {})",
                    short_session_id(&command.target_session_id)
                ),
            };
        }

        format!("session {}", short_session_id(&command.target_session_id))
    }

    fn describe_result_target(&self, result: &BridgeCommandResult) -> String {
        if let Some(place_name) = &result.target_place_name {
            return match result.target_place_id {
                Some(place_id) => format!(
                    "{place_name} (place {place_id}, session {})",
                    short_session_id(&result.target_session_id)
                ),
                None => format!(
                    "{place_name} (session {})",
                    short_session_id(&result.target_session_id)
                ),
            };
        }

        format!("session {}", short_session_id(&result.target_session_id))
    }

    fn prune_sessions(&mut self) {
        let cutoff = now_ms().saturating_sub(PLUGIN_SESSION_RETENTION_MS);
        self.sessions.retain(|session_id, session| {
            session.last_seen_ms >= cutoff
                || self.selected_session_id.as_deref() == Some(session_id.as_str())
                || self
                    .pending_command
                    .as_ref()
                    .is_some_and(|command| command.target_session_id == *session_id)
                || self
                    .active_command
                    .as_ref()
                    .is_some_and(|command| command.target_session_id == *session_id)
        });
        if self
            .selected_session_id
            .as_deref()
            .is_some_and(|session_id| !self.sessions.contains_key(session_id))
        {
            self.selected_session_id = None;
        }
    }

    fn log(&mut self, level: &str, message: String) {
        self.logs.push(DashboardLogEntry {
            timestamp_ms: now_ms(),
            level: level.to_string(),
            message,
        });
        if self.logs.len() > LOG_BUFFER_LIMIT {
            let excess = self.logs.len().saturating_sub(LOG_BUFFER_LIMIT);
            self.logs.drain(0..excess);
        }
    }
}

fn short_session_id(session_id: &str) -> &str {
    session_id.get(..8).unwrap_or(session_id)
}

pub async fn serve(root: PathBuf, port: u16) -> Result<()> {
    let host = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let dashboard = Arc::new(Mutex::new(DashboardState::default()));
    let app = Router::new()
        .route("/", get(dashboard_ui))
        .route("/api/tree", get(get_tree))
        .route("/api/create", post(create_entry))
        .route("/api/update", post(update_entry))
        .route("/api/delete", post(delete_entry))
        .route("/api/sync-from-studio", post(sync_from_studio))
        .route("/api/backups/studio/create", post(create_studio_backup))
        .route("/api/backups/studio/read", post(read_studio_backup))
        .route("/api/health", get(health))
        .route("/api/info", get(info))
        .route("/api/control/places", get(control_places))
        .route("/api/control/command-state", get(control_command_state))
        .route("/api/control/command", post(queue_control_command))
        .route("/api/control/select-session", post(select_control_session))
        .route("/api/dashboard/state", get(dashboard_state))
        .route("/api/dashboard/command", post(queue_dashboard_command))
        .route(
            "/api/dashboard/select-session",
            post(select_dashboard_session),
        )
        .route("/api/plugin/heartbeat", post(plugin_heartbeat))
        .route("/api/plugin/poll", post(plugin_poll))
        .route("/api/plugin/command-result", post(plugin_command_result))
        .layer(DefaultBodyLimit::max(STUDIO_SYNC_MAX_BODY_BYTES))
        .with_state(AppState {
            root: root.clone(),
            dashboard,
        });

    let listener = TcpListener::bind((host, port))
        .await
        .with_context(|| format!("failed to bind nyjo server to {host}:{port}"))?;

    println!("nyjo server listening on http://{}:{}/", host, port);
    axum::serve(listener, app)
        .await
        .context("nyjo server stopped unexpectedly")
}

async fn health() -> impl IntoResponse {
    success(json!({
        "status": "ok",
        "binding": "127.0.0.1",
        "version": env!("CARGO_PKG_VERSION")
    }))
}

async fn dashboard_ui() -> Html<&'static str> {
    Html(DASHBOARD_HTML)
}

async fn info(State(state): State<AppState>) -> impl IntoResponse {
    success(json!({
        "status": "ok",
        "binding": "127.0.0.1",
        "version": env!("CARGO_PKG_VERSION"),
        "projectRoot": state.root.display().to_string(),
        "capabilities": {
            "webDashboard": true,
            "studioBridge": true,
            "controlApi": true,
            "placeListingApi": true,
            "treePreview": true,
            "studioPush": true,
            "studioPullPreview": true,
            "studioPullApply": true,
            "studioPullForce": true,
            "studioPushBackups": true,
            "studioRestore": true,
            "localPullBackups": true,
            "localRestoreCli": true,
            "multiPlaceTargeting": true,
            "pushPullCli": true,
            "embeddedFileMetadata": true,
            "compactStudioPull": true
        },
        "translatedItems": {
            "compactFiles": [
                ".instance.json",
                ".model.json",
                ".service.json"
            ],
            "embeddedHeaderFiles": [
                ".server.lua",
                ".client.lua",
                ".lua",
                ".part",
                ".model",
                ".worldmodel",
                ".folder",
                ".rf",
                ".re",
                ".bf",
                ".be"
            ],
            "rootServices": ROOT_SERVICE_CLASSES,
            "typedValues": [
                "boolean",
                "number",
                "string",
                "Color3",
                "Vector2",
                "Vector3",
                "CFrame",
                "UDim",
                "UDim2",
                "EnumItem"
            ],
            "commonProperties": [
                "BasePart",
                "ValueBase",
                "ScreenGui",
                "GuiObject",
                "TextLabel",
                "TextButton",
                "TextBox",
                "ImageLabel",
                "ImageButton",
                "ScrollingFrame",
                "UIListLayout",
                "UIGridLayout",
                "UIPadding",
                "UICorner",
                "UIStroke",
                "UIShadow"
            ]
        }
    }))
}

async fn dashboard_state(State(state): State<AppState>) -> impl IntoResponse {
    match state.dashboard.lock() {
        Ok(mut dashboard) => {
            dashboard.prune_sessions();
            success(json!(dashboard.snapshot(&state.root)))
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            anyhow::anyhow!("dashboard state lock was poisoned"),
        ),
    }
}

async fn control_places(State(state): State<AppState>) -> impl IntoResponse {
    match state.dashboard.lock() {
        Ok(mut dashboard) => {
            dashboard.prune_sessions();
            success(json!(dashboard.places_snapshot()))
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            anyhow::anyhow!("dashboard state lock was poisoned"),
        ),
    }
}

async fn control_command_state(State(state): State<AppState>) -> impl IntoResponse {
    match state.dashboard.lock() {
        Ok(mut dashboard) => {
            dashboard.prune_sessions();
            success(json!(dashboard.command_state_snapshot()))
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            anyhow::anyhow!("dashboard state lock was poisoned"),
        ),
    }
}

async fn queue_dashboard_command(
    State(state): State<AppState>,
    Json(request): Json<QueueBridgeCommandRequest>,
) -> impl IntoResponse {
    queue_bridge_command(state, request)
}

async fn queue_control_command(
    State(state): State<AppState>,
    Json(request): Json<QueueBridgeCommandRequest>,
) -> impl IntoResponse {
    queue_bridge_command(state, request)
}

fn queue_bridge_command(state: AppState, request: QueueBridgeCommandRequest) -> impl IntoResponse {
    match state.dashboard.lock() {
        Ok(mut dashboard) => {
            match dashboard.queue_command(request.kind, request.target_session_id.as_deref()) {
                Ok(command) => success(json!(QueueBridgeCommandResponse {
                    queued: true,
                    command,
                    message: format!("Queued {}", request.kind.label()),
                })),
                Err(error) => error_response(StatusCode::CONFLICT, error),
            }
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            anyhow::anyhow!("dashboard state lock was poisoned"),
        ),
    }
}

async fn select_dashboard_session(
    State(state): State<AppState>,
    Json(request): Json<SelectBridgeSessionRequest>,
) -> impl IntoResponse {
    select_bridge_session(state, request)
}

async fn select_control_session(
    State(state): State<AppState>,
    Json(request): Json<SelectBridgeSessionRequest>,
) -> impl IntoResponse {
    select_bridge_session(state, request)
}

fn select_bridge_session(
    state: AppState,
    request: SelectBridgeSessionRequest,
) -> impl IntoResponse {
    match state.dashboard.lock() {
        Ok(mut dashboard) => match dashboard.set_selected_session(request.session_id) {
            Ok(()) => success(json!(SelectBridgeSessionResponse {
                selected: true,
                selected_session_id: dashboard.selected_session_id.clone(),
                message: "Studio bridge target updated".to_string(),
            })),
            Err(error) => error_response(StatusCode::BAD_REQUEST, error),
        },
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            anyhow::anyhow!("dashboard state lock was poisoned"),
        ),
    }
}

async fn plugin_heartbeat(
    State(state): State<AppState>,
    Json(request): Json<PluginHeartbeatRequest>,
) -> impl IntoResponse {
    match state.dashboard.lock() {
        Ok(mut dashboard) => {
            dashboard.update_plugin_heartbeat(request);
            success(json!({
                "connected": true,
                "serverTimeMs": now_ms()
            }))
        }
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            anyhow::anyhow!("dashboard state lock was poisoned"),
        ),
    }
}

async fn plugin_poll(
    State(state): State<AppState>,
    Json(request): Json<PluginPollRequest>,
) -> impl IntoResponse {
    match state.dashboard.lock() {
        Ok(mut dashboard) => success(json!({
            "command": dashboard.take_pending_command(&request.session_id)
        })),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            anyhow::anyhow!("dashboard state lock was poisoned"),
        ),
    }
}

async fn plugin_command_result(
    State(state): State<AppState>,
    Json(request): Json<PluginCommandResultRequest>,
) -> impl IntoResponse {
    match state.dashboard.lock() {
        Ok(mut dashboard) => match dashboard.finish_command(request) {
            Ok(record) => success(json!({
                "accepted": true,
                "result": record
            })),
            Err(error) => error_response(StatusCode::BAD_REQUEST, error),
        },
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            anyhow::anyhow!("dashboard state lock was poisoned"),
        ),
    }
}

async fn get_tree(State(state): State<AppState>) -> impl IntoResponse {
    match scan_project(&state.root) {
        Ok(tree) => success(json!({ "tree": tree })),
        Err(error) => error_response(StatusCode::INTERNAL_SERVER_ERROR, error),
    }
}

async fn create_entry(
    State(state): State<AppState>,
    Json(request): Json<CreateRequest>,
) -> impl IntoResponse {
    match create_entry_impl(&state.root, request) {
        Ok(tree) => success(json!({ "tree": tree })),
        Err(error) => error_response(StatusCode::BAD_REQUEST, error),
    }
}

async fn update_entry(
    State(state): State<AppState>,
    Json(request): Json<UpdateRequest>,
) -> impl IntoResponse {
    match update_entry_impl(&state.root, request) {
        Ok(tree) => success(json!({ "tree": tree })),
        Err(error) => error_response(StatusCode::BAD_REQUEST, error),
    }
}

async fn delete_entry(
    State(state): State<AppState>,
    Json(request): Json<DeleteRequest>,
) -> impl IntoResponse {
    match delete_entry_impl(&state.root, request) {
        Ok(tree) => success(json!({ "tree": tree })),
        Err(error) => error_response(StatusCode::BAD_REQUEST, error),
    }
}

async fn sync_from_studio(
    State(state): State<AppState>,
    Json(request): Json<StudioSyncRequest>,
) -> impl IntoResponse {
    match sync_from_studio_impl(&state.root, request) {
        Ok(response) => success(json!(response)),
        Err(error) => error_response(StatusCode::BAD_REQUEST, error),
    }
}

async fn create_studio_backup(
    State(state): State<AppState>,
    Json(request): Json<StudioBackupCreateRequest>,
) -> impl IntoResponse {
    match create_studio_backup_impl(&state.root, request) {
        Ok(backup) => success(json!({ "backup": backup })),
        Err(error) => error_response(StatusCode::BAD_REQUEST, error),
    }
}

async fn read_studio_backup(
    State(state): State<AppState>,
    Json(request): Json<StudioBackupReadRequest>,
) -> impl IntoResponse {
    match read_studio_backup_impl(&state.root, request) {
        Ok(snapshot) => success(json!(snapshot)),
        Err(error) => error_response(StatusCode::BAD_REQUEST, error),
    }
}

fn create_entry_impl(root: &Path, request: CreateRequest) -> Result<crate::model::InstanceNode> {
    let target = resolve_project_path(root, &request.path)?;
    let node_type = request
        .node_type
        .unwrap_or_else(|| infer_node_type(&target));

    match node_type.as_str() {
        "directory" => {
            if request.contents.is_some() {
                bail!("cannot write file contents into directory {}", request.path);
            }
            validate_fixed_target_metadata(&target, request.metadata.as_ref())?;
            if target.exists() && !target.is_dir() {
                bail!("{} already exists as a file", request.path);
            }
            fs::create_dir_all(&target)
                .with_context(|| format!("failed to create directory {}", target.display()))?;
            if let Some(metadata) = request.metadata {
                write_directory_metadata(&target, metadata, None)?;
            }
        }
        "file" => {
            validate_fixed_target_metadata(&target, request.metadata.as_ref())?;
            if target.exists() && !request.overwrite {
                bail!(
                    "{} already exists; set overwrite=true to replace it",
                    request.path
                );
            }
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent).with_context(|| {
                    format!("failed to create parent directory {}", parent.display())
                })?;
            }

            if let Some(always_embed_header) = embedded_metadata_mode(&target) {
                write_embedded_entry_file(
                    &target,
                    request.contents,
                    request.metadata,
                    always_embed_header,
                )?;
            } else {
                fs::write(&target, request.contents.unwrap_or_default())
                    .with_context(|| format!("failed to write file {}", target.display()))?;

                if let Some(metadata) = request.metadata {
                    write_metadata(&target, metadata)?;
                }
            }
        }
        other => bail!("unsupported nodeType {other}; expected file or directory"),
    }

    scan_project(root)
}

fn update_entry_impl(root: &Path, request: UpdateRequest) -> Result<crate::model::InstanceNode> {
    let target = resolve_project_path(root, &request.path)?;
    if !target.exists() {
        bail!("{} does not exist", request.path);
    }

    if target.is_dir() {
        if request.contents.is_some() {
            bail!("cannot write file contents into directory {}", request.path);
        }
        if let Some(metadata) = request.metadata {
            validate_fixed_target_metadata(&target, Some(&metadata))?;
            write_directory_metadata(&target, metadata, None)?;
        }
    } else if let Some(always_embed_header) = embedded_metadata_mode(&target) {
        validate_fixed_target_metadata(&target, request.metadata.as_ref())?;
        write_embedded_entry_file(
            &target,
            request.contents,
            request.metadata,
            always_embed_header,
        )?;
    } else {
        validate_fixed_target_metadata(&target, request.metadata.as_ref())?;
        if let Some(contents) = request.contents {
            fs::write(&target, contents)
                .with_context(|| format!("failed to write file {}", target.display()))?;
        }

        if let Some(metadata) = request.metadata {
            write_metadata(&target, metadata)?;
        }
    }

    scan_project(root)
}

fn delete_entry_impl(root: &Path, request: DeleteRequest) -> Result<crate::model::InstanceNode> {
    let target = resolve_project_path(root, &request.path)?;
    if !target.exists() {
        bail!("{} does not exist", request.path);
    }

    if target.is_dir() {
        if request.recursive {
            fs::remove_dir_all(&target)
                .with_context(|| format!("failed to remove directory {}", target.display()))?;
        } else {
            fs::remove_dir(&target).with_context(|| {
                format!(
                    "failed to remove directory {}; use recursive=true for non-empty directories",
                    target.display()
                )
            })?;
        }
    } else {
        let metadata_path = metadata_path_for_target(&target)?;
        fs::remove_file(&target)
            .with_context(|| format!("failed to remove file {}", target.display()))?;
        if metadata_path.exists() {
            let _ = fs::remove_file(metadata_path);
        }
    }

    scan_project(root)
}

fn create_studio_backup_impl(
    root: &Path,
    request: StudioBackupCreateRequest,
) -> Result<BackupRecord> {
    if request.tree.class_name != "DataModel" {
        bail!(
            "expected a DataModel snapshot root, got {}",
            request.tree.class_name
        );
    }

    let project_root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve project root {}", root.display()))?;
    let tree = serde_json::to_value(&request.tree)?;

    create_studio_tree_backup(
        &project_root,
        StudioBackupIdentity {
            session_id: Some(request.session_id),
            place_name: request.place_name,
            place_id: request.place_id,
        },
        request
            .reason
            .unwrap_or_else(|| "before Studio mutation".to_string()),
        &tree,
        Some(json!({
            "source": "studioPlugin"
        })),
    )
}

fn read_studio_backup_impl(
    root: &Path,
    request: StudioBackupReadRequest,
) -> Result<crate::backup::StudioBackupSnapshot> {
    let project_root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve project root {}", root.display()))?;
    read_studio_tree_backup(&project_root, request.backup_id.trim())
}

fn sync_from_studio_impl(root: &Path, request: StudioSyncRequest) -> Result<StudioSyncResponse> {
    let project_root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve project root {}", root.display()))?;

    if request.tree.class_name != "DataModel" {
        bail!(
            "expected a DataModel snapshot root, got {}",
            request.tree.class_name
        );
    }

    let mut planner = StudioSyncPlanner::new(&project_root, request.force);
    planner.plan_snapshot(&request.tree)?;

    let blocked =
        request.mode == StudioSyncMode::Apply && planner.plan.stats.conflicts > 0 && !request.force;

    let mut applied = false;
    let mut backup = None;
    let mut tree = None;

    if request.mode == StudioSyncMode::Apply && !blocked {
        if !planner.pending_writes.is_empty() {
            backup = Some(create_local_project_backup(
                &project_root,
                if request.force {
                    "before force pull from Studio"
                } else {
                    "before apply pull from Studio"
                },
                Some(json!({
                    "mode": request.mode,
                    "force": request.force,
                    "stats": &planner.plan.stats,
                })),
            )?);
        }
        planner.apply()?;
        applied = true;
        tree = Some(scan_project(&project_root)?);
    }

    Ok(StudioSyncResponse {
        mode: request.mode,
        force: request.force,
        applied,
        blocked,
        plan: planner.plan,
        backup,
        tree,
    })
}

struct StudioSyncPlanner<'a> {
    project_root: &'a Path,
    force: bool,
    plan: StudioSyncPlan,
    pending_writes: Vec<PendingWrite>,
}

impl<'a> StudioSyncPlanner<'a> {
    fn new(project_root: &'a Path, force: bool) -> Self {
        Self {
            project_root,
            force,
            plan: StudioSyncPlan::default(),
            pending_writes: Vec::new(),
        }
    }

    fn plan_snapshot(&mut self, root: &StudioNode) -> Result<()> {
        for service_node in &root.children {
            let node_count = count_studio_nodes(service_node);
            self.plan.stats.nodes_seen += node_count;

            if !is_supported_root_service(service_node) {
                self.plan.stats.skipped_nodes += node_count;
                self.push_operation(
                    "skip",
                    "service",
                    service_node.name.clone(),
                    format!(
                        "unsupported root service {}; skipping this subtree",
                        service_node.class_name
                    ),
                );
                continue;
            }

            self.plan.stats.supported_services += 1;
            let service_dir = self.project_root.join(&service_node.name);
            if !self.plan_directory_target(
                &service_dir,
                format!("service directory for {}", service_node.class_name),
            )? {
                continue;
            }

            for child in &service_node.children {
                self.plan_studio_node(&service_dir, Some(&service_node.class_name), child)?;
            }
        }

        Ok(())
    }

    fn plan_studio_node(
        &mut self,
        parent_dir: &Path,
        parent_class: Option<&str>,
        node: &StudioNode,
    ) -> Result<()> {
        if let Some(target_dir) = studio_container_target(parent_dir, node) {
            return self.plan_container_node(&target_dir, parent_class, node);
        }

        if let Some((target, contents)) = studio_file_target(parent_dir, parent_class, node)? {
            if !self.plan_logical_representation_conflicts(
                parent_dir,
                &node.name,
                &target,
                format!("replace alternate local representation for {}", node.name),
            )? {
                return Ok(());
            }

            self.plan_text_file_target(
                &target,
                contents,
                "file",
                format!("{} source for {}", node.class_name, node.name),
            )?;
            self.plan_file_metadata_target(
                &target,
                None,
                format!("remove sidecar metadata for {}", node.name),
            )?;
            return Ok(());
        }

        let target_dir = parent_dir.join(&node.name);
        if !self.plan_directory_target(
            &target_dir,
            format!("container directory for {}", node.class_name),
        )? {
            return Ok(());
        }

        self.plan_directory_metadata_target(
            &target_dir,
            metadata_from_studio_node(node, true, parent_class),
            Some(&node.class_name),
            format!("metadata for {}", node.name),
        )?;

        for child in &node.children {
            self.plan_studio_node(target_dir.as_path(), Some(&node.class_name), child)?;
        }

        Ok(())
    }

    fn plan_container_node(
        &mut self,
        target_dir: &Path,
        parent_class: Option<&str>,
        node: &StudioNode,
    ) -> Result<()> {
        if !self.plan_directory_target(
            target_dir,
            format!("directory-backed {} container", node.class_name),
        )? {
            return Ok(());
        }

        let inline_metadata = metadata_from_studio_node(node, false, parent_class);
        if let Some((source_file_name, contents, always_embed_header)) =
            container_source_file(node, inline_metadata.is_some())
        {
            let rendered_source = render_embedded_text_document(
                contents.as_str(),
                inline_metadata,
                always_embed_header,
            )?;
            let source_path = target_dir.join(&source_file_name);
            self.plan_text_file_target(
                &source_path,
                rendered_source,
                "file",
                format!("container source for {}", node.name),
            )?;
            self.plan_directory_metadata_cleanup(
                target_dir,
                Some(&node.class_name),
                Some(&source_path),
                format!("remove legacy metadata for {}", node.name),
            )?;
        } else {
            self.plan_directory_metadata_target(
                target_dir,
                metadata_from_studio_node(node, true, parent_class),
                Some(&node.class_name),
                format!("metadata for {}", node.name),
            )?;
        }

        for child in &node.children {
            self.plan_studio_node(target_dir, Some(&node.class_name), child)?;
        }

        Ok(())
    }

    fn plan_directory_target(&mut self, target: &Path, detail: String) -> Result<bool> {
        let parent = target.parent().with_context(|| {
            format!("directory target {} is missing a parent", target.display())
        })?;
        let name = file_name(target)?;

        if !self.plan_logical_representation_conflicts(parent, &name, target, detail.clone())? {
            return Ok(false);
        }

        let display = display_path(self.project_root, target);
        if target.exists() {
            if target.is_dir() {
                self.plan.stats.unchanged += 1;
                return Ok(true);
            }

            if !self.force {
                self.record_conflict(
                    "directory",
                    display,
                    "local file blocks a directory target; rerun with force to replace it"
                        .to_string(),
                );
                return Ok(false);
            }

            self.schedule_removal(target, format!("replace local file before {detail}"))?;
            self.schedule_directory_create(target, "replace", detail);
            return Ok(true);
        }

        self.schedule_directory_create(target, "create", detail);
        Ok(true)
    }

    fn plan_text_file_target(
        &mut self,
        target: &Path,
        contents: String,
        kind: &'static str,
        detail: String,
    ) -> Result<()> {
        let display = display_path(self.project_root, target);
        if target.exists() {
            if target.is_dir() {
                if !self.force {
                    self.record_conflict(
                        kind,
                        display,
                        "local directory blocks a file target; rerun with force to replace it"
                            .to_string(),
                    );
                    return Ok(());
                }

                self.schedule_removal(target, format!("replace local directory before {detail}"))?;
                self.schedule_file_write(target, contents, kind, "replace", detail);
                return Ok(());
            }

            let existing = fs::read_to_string(target)
                .with_context(|| format!("failed to read local file {}", target.display()))?;
            if existing == contents {
                self.plan.stats.unchanged += 1;
                return Ok(());
            }

            if !self.force {
                let message = match kind {
                    "metadata" => {
                        "local metadata differs from the Studio snapshot; rerun with force to replace it"
                    }
                    _ => {
                        "local file contents differ from the Studio snapshot; rerun with force to replace them"
                    }
                };
                self.record_conflict(kind, display, message.to_string());
                return Ok(());
            }

            self.schedule_file_write(target, contents, kind, "update", detail);
            return Ok(());
        }

        self.schedule_file_write(target, contents, kind, "create", detail);
        Ok(())
    }

    fn plan_file_metadata_target(
        &mut self,
        target: &Path,
        metadata: Option<NodeMetadata>,
        detail: String,
    ) -> Result<()> {
        let metadata_path = metadata_path_for_target(target)?;
        match metadata {
            Some(metadata) => self.plan_text_file_target(
                &metadata_path,
                serde_json::to_string_pretty(&metadata)?,
                "metadata",
                detail,
            ),
            None => {
                if !metadata_path.exists() {
                    return Ok(());
                }

                if !self.force {
                    self.record_conflict(
                        "metadata",
                        display_path(self.project_root, &metadata_path),
                        "local metadata would be removed by this pull; rerun with force to remove it"
                            .to_string(),
                    );
                    return Ok(());
                }

                self.schedule_removal(
                    &metadata_path,
                    format!("remove local metadata not present in Studio for {detail}"),
                )
            }
        }
    }

    fn plan_directory_metadata_target(
        &mut self,
        target: &Path,
        metadata: Option<NodeMetadata>,
        class_name_hint: Option<&str>,
        detail: String,
    ) -> Result<()> {
        match metadata {
            Some(metadata) => {
                let metadata_path = preferred_directory_metadata_path(target, class_name_hint);
                let rendered = render_embedded_text_document("", Some(metadata), true)?;
                self.plan_text_file_target(&metadata_path, rendered, "metadata", detail.clone())?;
                self.plan_directory_metadata_cleanup(
                    target,
                    class_name_hint,
                    Some(&metadata_path),
                    format!("remove legacy metadata for {detail}"),
                )
            }
            None => self.plan_directory_metadata_cleanup(target, class_name_hint, None, detail),
        }
    }

    fn plan_directory_metadata_cleanup(
        &mut self,
        target: &Path,
        class_name_hint: Option<&str>,
        keep: Option<&Path>,
        detail: String,
    ) -> Result<()> {
        for path in directory_metadata_artifact_paths(target, class_name_hint) {
            if keep.is_some_and(|keep| keep == path.as_path()) || !path.exists() {
                continue;
            }

            if !self.force {
                self.record_conflict(
                    "metadata",
                    display_path(self.project_root, &path),
                    "local metadata would be removed by this pull; rerun with force to remove it"
                        .to_string(),
                );
                continue;
            }

            self.schedule_removal(
                &path,
                format!("remove local metadata not present in Studio for {detail}"),
            )?;
        }

        Ok(())
    }

    fn plan_logical_representation_conflicts(
        &mut self,
        parent_dir: &Path,
        node_name: &str,
        allowed_target: &Path,
        detail: String,
    ) -> Result<bool> {
        let mut clear = true;

        for peer in logical_peer_paths(parent_dir, node_name) {
            if peer == allowed_target || !peer.exists() {
                continue;
            }

            if !self.force {
                clear = false;
                self.record_conflict(
                    classify_existing_path_kind(&peer),
                    display_path(self.project_root, &peer),
                    format!(
                        "local path already represents {} in a different shape; rerun with force to replace it",
                        node_name
                    ),
                );
                continue;
            }

            self.schedule_removal(&peer, detail.clone())?;
        }

        Ok(clear)
    }

    fn schedule_directory_create(&mut self, target: &Path, action: &'static str, detail: String) {
        self.plan.stats.directories_planned += 1;
        self.push_operation(
            action,
            "directory",
            display_path(self.project_root, target),
            detail,
        );
        self.pending_writes.push(PendingWrite::EnsureDirectory {
            path: target.to_path_buf(),
        });
    }

    fn schedule_file_write(
        &mut self,
        target: &Path,
        contents: String,
        kind: &'static str,
        action: &'static str,
        detail: String,
    ) {
        match kind {
            "metadata" => self.plan.stats.metadata_planned += 1,
            _ => self.plan.stats.files_planned += 1,
        }

        self.push_operation(
            action,
            kind,
            display_path(self.project_root, target),
            detail,
        );
        self.pending_writes.push(PendingWrite::WriteFile {
            path: target.to_path_buf(),
            contents,
            kind,
        });
    }

    fn schedule_removal(&mut self, path: &Path, detail: String) -> Result<()> {
        if !path.exists() || self.removal_already_scheduled(path) {
            return Ok(());
        }

        let kind = classify_existing_path_kind(path);
        self.plan.stats.removals_planned += 1;
        self.push_operation(
            "delete",
            kind,
            display_path(self.project_root, path),
            detail,
        );

        if path.is_dir() {
            self.pending_writes.push(PendingWrite::RemoveDirectory {
                path: path.to_path_buf(),
            });
            return Ok(());
        }

        self.pending_writes.push(PendingWrite::RemoveFile {
            path: path.to_path_buf(),
        });

        if kind != "metadata" {
            let metadata_path = metadata_path_for_target(path)?;
            if metadata_path.exists() {
                self.schedule_removal(
                    &metadata_path,
                    format!(
                        "remove metadata alongside {}",
                        display_path(self.project_root, path)
                    ),
                )?;
            }
        }

        Ok(())
    }

    fn removal_already_scheduled(&self, target: &Path) -> bool {
        self.pending_writes.iter().any(|write| match write {
            PendingWrite::RemoveFile { path } | PendingWrite::RemoveDirectory { path } => {
                path == target
            }
            PendingWrite::EnsureDirectory { .. } | PendingWrite::WriteFile { .. } => false,
        })
    }

    fn record_conflict(&mut self, kind: &'static str, path: String, message: String) {
        self.plan.stats.conflicts += 1;
        self.plan.conflicts.push(StudioSyncConflict {
            kind,
            path: path.clone(),
            message: message.clone(),
        });
        self.push_operation("conflict", kind, path, message);
    }

    fn push_operation(
        &mut self,
        action: &'static str,
        kind: &'static str,
        path: String,
        detail: String,
    ) {
        self.plan.operations.push(StudioSyncOperation {
            action,
            kind,
            path,
            detail,
        });
    }

    fn apply(&mut self) -> Result<()> {
        let pending_writes = std::mem::take(&mut self.pending_writes);
        for write in pending_writes {
            match write {
                PendingWrite::EnsureDirectory { path } => {
                    self.ensure_directory_for_apply(&path)?;
                    self.plan.stats.directories_written += 1;
                }
                PendingWrite::WriteFile {
                    path,
                    contents,
                    kind,
                } => {
                    if let Some(parent) = path.parent() {
                        self.ensure_directory_for_apply(parent)?;
                    }

                    fs::write(&path, contents)
                        .with_context(|| format!("failed to write {}", path.display()))?;

                    match kind {
                        "metadata" => self.plan.stats.metadata_written += 1,
                        _ => self.plan.stats.files_written += 1,
                    }
                }
                PendingWrite::RemoveFile { path } => {
                    if path.exists() {
                        fs::remove_file(&path).with_context(|| {
                            format!("failed to remove studio-synced file {}", path.display())
                        })?;
                        self.plan.stats.removals_applied += 1;
                    }
                }
                PendingWrite::RemoveDirectory { path } => {
                    if path.exists() {
                        fs::remove_dir_all(&path).with_context(|| {
                            format!(
                                "failed to remove studio-synced directory {}",
                                path.display()
                            )
                        })?;
                        self.plan.stats.removals_applied += 1;
                    }
                }
            }
        }

        Ok(())
    }

    fn ensure_directory_for_apply(&mut self, directory: &Path) -> Result<()> {
        let mut ancestors = directory.ancestors().collect::<Vec<_>>();
        ancestors.reverse();

        for ancestor in ancestors {
            if ancestor == self.project_root {
                continue;
            }

            if ancestor.is_file() {
                if !self.force {
                    bail!(
                        "local file {} blocks directory creation for {}; rerun with force to replace it",
                        ancestor.display(),
                        directory.display()
                    );
                }

                fs::remove_file(ancestor).with_context(|| {
                    format!(
                        "failed to remove local file {} before creating directory {}",
                        ancestor.display(),
                        directory.display()
                    )
                })?;
                self.plan.stats.removals_applied += 1;
            }
        }

        fs::create_dir_all(directory).with_context(|| {
            format!(
                "failed to create studio-synced directory {}",
                directory.display()
            )
        })
    }
}

fn studio_file_target(
    parent_dir: &Path,
    parent_class: Option<&str>,
    node: &StudioNode,
) -> Result<Option<(PathBuf, String)>> {
    if !node.children.is_empty() && has_child_local_name_collisions(&node.children) {
        return Ok(Some((
            parent_dir.join(format!("{}.instance.json", node.name)),
            render_compact_instance_document(node)?,
        )));
    }

    let Some((suffix, contents, always_embed_header)) = (match node.class_name.as_str() {
        "Script" => Some((".server.lua", node.source.clone().unwrap_or_default(), true)),
        "LocalScript" => Some((".client.lua", node.source.clone().unwrap_or_default(), true)),
        "ModuleScript" => Some((".lua", node.source.clone().unwrap_or_default(), true)),
        "RemoteFunction" => Some((".rf", String::new(), true)),
        "RemoteEvent" => Some((".re", String::new(), true)),
        "BindableFunction" => Some((".bf", String::new(), true)),
        "BindableEvent" => Some((".be", String::new(), true)),
        "Folder" => Some((".folder", String::new(), true)),
        "Part" => Some((".part", render_structured_instance_contents(node)?, true)),
        "Model" => Some((".model", render_structured_instance_contents(node)?, true)),
        "WorldModel" => Some((
            ".worldmodel",
            render_structured_instance_contents(node)?,
            true,
        )),
        "ScreenGui" => Some((".screengui", String::new(), true)),
        "CanvasGroup" => Some((".canvasgroup", String::new(), true)),
        "ScrollingFrame" => Some((".scrollingframe", String::new(), true)),
        "SurfaceGui" => Some((".surfacegui", String::new(), true)),
        "BillboardGui" => Some((".billboardgui", String::new(), true)),
        "Frame" => Some((".frame", String::new(), true)),
        "TextLabel" => Some((".textlabel", String::new(), true)),
        "TextButton" => Some((".textbutton", String::new(), true)),
        "TextBox" => Some((".textbox", String::new(), true)),
        "ImageLabel" => Some((".imagelabel", String::new(), true)),
        "ImageButton" => Some((".imagebutton", String::new(), true)),
        "UIListLayout" => Some((".uilistlayout", String::new(), true)),
        "UIGridLayout" => Some((".uigridlayout", String::new(), true)),
        "UIPadding" => Some((".uipadding", String::new(), true)),
        "UICorner" => Some((".uicorner", String::new(), true)),
        "UIStroke" => Some((".uistroke", String::new(), true)),
        "UIShadow" => Some((".uishadow", String::new(), true)),
        "Texture" => Some((".texture", String::new(), true)),
        "Decal" => Some((".decal", String::new(), true)),
        "StringValue" => Some((".stringvalue", String::new(), true)),
        "NumberValue" => Some((".numbervalue", String::new(), true)),
        "IntValue" => Some((".intvalue", String::new(), true)),
        "BoolValue" => Some((".boolvalue", String::new(), true)),
        "Color3Value" => Some((".color3value", String::new(), true)),
        "Vector3Value" => Some((".vector3value", String::new(), true)),
        _ => None,
    }) else {
        return Ok(Some((
            parent_dir.join(format!("{}.instance.json", node.name)),
            render_compact_instance_document(node)?,
        )));
    };

    let metadata = metadata_from_studio_node(node, false, parent_class);
    let rendered = render_embedded_text_document(&contents, metadata, always_embed_header)?;

    Ok(Some((
        parent_dir.join(format!("{}{}", node.name, suffix)),
        rendered,
    )))
}

fn studio_container_target(parent_dir: &Path, node: &StudioNode) -> Option<PathBuf> {
    if node.children.is_empty() {
        return None;
    }

    if has_child_local_name_collisions(&node.children) {
        return None;
    }

    if is_script_container_class(&node.class_name) {
        return Some(parent_dir.join(&node.name));
    }

    if node.class_name == "Folder" {
        return Some(parent_dir.join(&node.name));
    }

    if let Some(suffix) = typed_container_suffix_for_class(&node.class_name) {
        return Some(parent_dir.join(format!("{}{}", node.name, suffix)));
    }

    Some(parent_dir.join(&node.name))
}

fn is_script_container_class(class_name: &str) -> bool {
    matches!(class_name, "Script" | "LocalScript" | "ModuleScript")
}

fn container_source_file(
    node: &StudioNode,
    metadata_present: bool,
) -> Option<(String, String, bool)> {
    match node.class_name.as_str() {
        "Script" => Some((
            "init.server.lua".to_string(),
            node.source.clone().unwrap_or_default(),
            true,
        )),
        "LocalScript" => Some((
            "init.client.lua".to_string(),
            node.source.clone().unwrap_or_default(),
            true,
        )),
        "ModuleScript" => Some((
            "init.lua".to_string(),
            node.source.clone().unwrap_or_default(),
            true,
        )),
        "Folder" if metadata_present => Some(("init.folder".to_string(), String::new(), true)),
        class_name if metadata_present => typed_container_suffix_for_class(class_name)
            .map(|suffix| (format!("init{suffix}"), String::new(), true)),
        _ => None,
    }
}

fn has_child_local_name_collisions(children: &[StudioNode]) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    children
        .iter()
        .any(|child| !seen.insert(studio_local_entry_name(child)))
}

fn studio_local_entry_name(node: &StudioNode) -> String {
    if !node.children.is_empty() {
        if is_script_container_class(&node.class_name) || node.class_name == "Folder" {
            return node.name.clone();
        }

        if let Some(suffix) = typed_container_suffix_for_class(&node.class_name) {
            return format!("{}{}", node.name, suffix);
        }

        return node.name.clone();
    }

    match node.class_name.as_str() {
        "Script" => format!("{}.server.lua", node.name),
        "LocalScript" => format!("{}.client.lua", node.name),
        "ModuleScript" => format!("{}.lua", node.name),
        "RemoteFunction" => format!("{}.rf", node.name),
        "RemoteEvent" => format!("{}.re", node.name),
        "BindableFunction" => format!("{}.bf", node.name),
        "BindableEvent" => format!("{}.be", node.name),
        "Folder" => format!("{}.folder", node.name),
        _ => typed_container_suffix_for_class(&node.class_name)
            .map(|suffix| format!("{}{}", node.name, suffix))
            .unwrap_or_else(|| format!("{}.instance.json", node.name)),
    }
}

fn typed_container_suffix_for_class(class_name: &str) -> Option<&'static str> {
    match class_name {
        "Part" => Some(".part"),
        "Model" => Some(".model"),
        "WorldModel" => Some(".worldmodel"),
        "RemoteFunction" => Some(".rf"),
        "RemoteEvent" => Some(".re"),
        "BindableFunction" => Some(".bf"),
        "BindableEvent" => Some(".be"),
        "ScreenGui" => Some(".screengui"),
        "CanvasGroup" => Some(".canvasgroup"),
        "ScrollingFrame" => Some(".scrollingframe"),
        "SurfaceGui" => Some(".surfacegui"),
        "BillboardGui" => Some(".billboardgui"),
        "Frame" => Some(".frame"),
        "TextLabel" => Some(".textlabel"),
        "TextButton" => Some(".textbutton"),
        "TextBox" => Some(".textbox"),
        "ImageLabel" => Some(".imagelabel"),
        "ImageButton" => Some(".imagebutton"),
        "UIListLayout" => Some(".uilistlayout"),
        "UIGridLayout" => Some(".uigridlayout"),
        "UIPadding" => Some(".uipadding"),
        "UICorner" => Some(".uicorner"),
        "UIStroke" => Some(".uistroke"),
        "UIShadow" => Some(".uishadow"),
        "Texture" => Some(".texture"),
        "Decal" => Some(".decal"),
        "StringValue" => Some(".stringvalue"),
        "NumberValue" => Some(".numbervalue"),
        "IntValue" => Some(".intvalue"),
        "BoolValue" => Some(".boolvalue"),
        "Color3Value" => Some(".color3value"),
        "Vector3Value" => Some(".vector3value"),
        _ => None,
    }
}

fn deserialize_map_or_empty_array<'de, D>(
    deserializer: D,
) -> Result<std::collections::BTreeMap<String, Value>, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Value::deserialize(deserializer)?;
    match value {
        Value::Object(map) => Ok(map.into_iter().collect()),
        Value::Array(items) if items.is_empty() => Ok(Default::default()),
        Value::Null => Ok(Default::default()),
        other => Err(serde::de::Error::custom(format!(
            "expected object or empty array, got {other}"
        ))),
    }
}

fn metadata_from_studio_node(
    node: &StudioNode,
    is_directory_target: bool,
    parent_class: Option<&str>,
) -> Option<NodeMetadata> {
    if is_directory_target
        && matches!(
            node.class_name.as_str(),
            "Script" | "LocalScript" | "ModuleScript"
        )
    {
        return None;
    }

    let mut metadata = NodeMetadata::default();

    if class_needs_metadata(node, is_directory_target, parent_class) {
        metadata.class_name = Some(node.class_name.clone());
    }

    metadata.properties = node.properties.clone();
    metadata.attributes = node.attributes.clone();
    metadata.tags = node.tags.clone();

    if metadata.class_name.is_some()
        || !metadata.properties.is_empty()
        || !metadata.attributes.is_empty()
        || !metadata.tags.is_empty()
    {
        Some(metadata)
    } else {
        None
    }
}

fn render_embedded_text_document(
    contents: &str,
    metadata: Option<NodeMetadata>,
    always_embed_header: bool,
) -> Result<String> {
    if metadata.is_none() && !always_embed_header {
        return Ok(contents.to_string());
    }

    let metadata = metadata.unwrap_or_default();
    let header_json = serde_json::to_string_pretty(&metadata)?;
    let mut rendered = String::new();
    rendered.push_str("--!nyjo\n");
    rendered.push_str("--HEADER\n");
    for line in header_json.lines() {
        rendered.push_str("--");
        rendered.push_str(line);
        rendered.push('\n');
    }
    rendered.push_str("--CONTENTS");
    if !contents.is_empty() {
        rendered.push('\n');
        rendered.push_str(contents);
    } else {
        rendered.push('\n');
    }
    Ok(rendered)
}

fn render_structured_instance_contents(node: &StudioNode) -> Result<String> {
    if node.children.is_empty() {
        return Ok(String::new());
    }

    Ok(serde_json::to_string_pretty(&json!({
        "children": studio_children_to_inline_value(&node.children)?
    }))?)
}

fn render_compact_instance_document(node: &StudioNode) -> Result<String> {
    serde_json::to_string_pretty(&studio_node_to_inline_value(node, false)?).map_err(Into::into)
}

fn studio_node_to_inline_value(node: &StudioNode, include_name: bool) -> Result<Value> {
    let mut object = serde_json::Map::new();
    if include_name {
        object.insert("name".to_string(), Value::String(node.name.clone()));
    }
    object.insert(
        "className".to_string(),
        Value::String(node.class_name.clone()),
    );

    if !node.properties.is_empty() {
        object.insert(
            "properties".to_string(),
            serde_json::to_value(&node.properties)?,
        );
    }
    if !node.attributes.is_empty() {
        object.insert(
            "attributes".to_string(),
            serde_json::to_value(&node.attributes)?,
        );
    }
    if !node.tags.is_empty() {
        object.insert("tags".to_string(), serde_json::to_value(&node.tags)?);
    }
    if let Some(source) = &node.source {
        object.insert("source".to_string(), Value::String(source.clone()));
    }
    if !node.children.is_empty() {
        object.insert(
            "children".to_string(),
            studio_children_to_inline_value(&node.children)?,
        );
    }

    Ok(Value::Object(object))
}

fn studio_children_to_inline_value(children: &[StudioNode]) -> Result<Value> {
    if has_duplicate_child_names(children) {
        return children
            .iter()
            .map(|child| studio_node_to_inline_value(child, true))
            .collect::<Result<Vec<_>>>()
            .map(Value::Array);
    }

    let mut children_by_name = serde_json::Map::new();
    for child in children {
        children_by_name.insert(
            child.name.clone(),
            studio_node_to_inline_value(child, false)?,
        );
    }
    Ok(Value::Object(children_by_name))
}

fn has_duplicate_child_names(children: &[StudioNode]) -> bool {
    let mut seen = std::collections::BTreeSet::new();
    children.iter().any(|child| !seen.insert(&child.name))
}

fn class_needs_metadata(
    node: &StudioNode,
    is_directory_target: bool,
    parent_class: Option<&str>,
) -> bool {
    if is_directory_target {
        if node.class_name == "Folder" {
            return false;
        }

        if typed_container_suffix_for_class(&node.class_name).is_some() {
            return false;
        }

        if parent_class == Some("StarterPlayer")
            && (node.class_name == "StarterPlayerScripts"
                || node.class_name == "StarterCharacterScripts")
            && node.name == node.class_name
        {
            return false;
        }

        return !is_supported_root_service(node);
    }

    false
}

fn is_supported_root_service(node: &StudioNode) -> bool {
    ROOT_SERVICE_CLASSES.contains(&node.class_name.as_str()) && node.name == node.class_name
}

fn logical_peer_paths(parent_dir: &Path, node_name: &str) -> Vec<PathBuf> {
    let mut paths = Vec::with_capacity(LOGICAL_NODE_FILE_SUFFIXES.len() + 1);
    paths.push(parent_dir.join(node_name));

    for suffix in LOGICAL_NODE_FILE_SUFFIXES {
        paths.push(parent_dir.join(format!("{node_name}{suffix}")));
    }

    paths
}

fn count_studio_nodes(node: &StudioNode) -> usize {
    1 + node.children.iter().map(count_studio_nodes).sum::<usize>()
}

fn classify_existing_path_kind(path: &Path) -> &'static str {
    if path.is_dir() {
        "directory"
    } else if is_metadata_path(path) {
        "metadata"
    } else {
        "file"
    }
}

fn is_metadata_path(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| {
            name == DIRECTORY_HEADER_FILE_NAME
                || name == ".meta.json"
                || name.ends_with(".meta.json")
        })
}

fn display_path(root: &Path, path: &Path) -> String {
    match path.strip_prefix(root) {
        Ok(relative) if relative.as_os_str().is_empty() => ".".to_string(),
        Ok(relative) => relative
            .components()
            .filter_map(|component| component.as_os_str().to_str())
            .collect::<Vec<_>>()
            .join("/"),
        Err(_) => path.display().to_string(),
    }
}

fn file_name(path: &Path) -> Result<String> {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .with_context(|| format!("path {} is missing a valid utf-8 file name", path.display()))
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

fn write_metadata(target: &Path, metadata: NodeMetadata) -> Result<()> {
    let metadata_path = metadata_path_for_target(target)?;
    let Some(metadata) = normalize_metadata(metadata) else {
        if metadata_path.exists() {
            fs::remove_file(&metadata_path).with_context(|| {
                format!(
                    "failed to remove empty metadata file {}",
                    metadata_path.display()
                )
            })?;
        }
        return Ok(());
    };

    if let Some(parent) = metadata_path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create metadata directory {}", parent.display()))?;
    }

    let contents = serde_json::to_string_pretty(&metadata)?;
    fs::write(&metadata_path, contents)
        .with_context(|| format!("failed to write metadata file {}", metadata_path.display()))
}

fn legacy_directory_metadata_path(target: &Path) -> PathBuf {
    target.join(".meta.json")
}

fn directory_descriptor_file_name_for_class(class_name: &str) -> Option<String> {
    match class_name {
        "Script" => Some("init.server.lua".to_string()),
        "LocalScript" => Some("init.client.lua".to_string()),
        "ModuleScript" => Some("init.lua".to_string()),
        "Folder" => Some("init.folder".to_string()),
        _ => typed_container_suffix_for_class(class_name).map(|suffix| format!("init{suffix}")),
    }
}

fn preferred_directory_metadata_path(target: &Path, class_name_hint: Option<&str>) -> PathBuf {
    let class_name = class_name_hint
        .map(ToOwned::to_owned)
        .or_else(|| fixed_target_class(target));

    if let Some(class_name) = class_name
        && let Some(file_name) = directory_descriptor_file_name_for_class(&class_name)
    {
        return target.join(file_name);
    }

    target.join(DIRECTORY_HEADER_FILE_NAME)
}

fn directory_metadata_artifact_paths(target: &Path, class_name_hint: Option<&str>) -> Vec<PathBuf> {
    let mut paths = vec![
        legacy_directory_metadata_path(target),
        target.join(DIRECTORY_HEADER_FILE_NAME),
        preferred_directory_metadata_path(target, class_name_hint),
    ];
    paths.sort();
    paths.dedup();
    paths
}

fn remove_directory_metadata_artifacts(
    target: &Path,
    class_name_hint: Option<&str>,
    keep: Option<&Path>,
) -> Result<()> {
    for path in directory_metadata_artifact_paths(target, class_name_hint) {
        if keep.is_some_and(|keep| keep == path.as_path()) || !path.exists() {
            continue;
        }

        fs::remove_file(&path).with_context(|| {
            format!(
                "failed to remove migrated directory metadata file {}",
                path.display()
            )
        })?;
    }

    Ok(())
}

fn write_directory_metadata(
    target: &Path,
    metadata: NodeMetadata,
    class_name_hint: Option<&str>,
) -> Result<()> {
    let metadata = normalize_metadata(metadata);
    let keep_path = metadata
        .as_ref()
        .map(|_| preferred_directory_metadata_path(target, class_name_hint));

    if let Some(path) = keep_path.as_ref() {
        write_embedded_entry_file(path, Some(String::new()), metadata, true)?;
    }

    remove_directory_metadata_artifacts(target, class_name_hint, keep_path.as_deref())
}

fn embedded_metadata_mode(target: &Path) -> Option<bool> {
    let file_name = target.file_name()?.to_str()?;

    if file_name.ends_with(".part")
        || file_name.ends_with(".model")
        || file_name.ends_with(".worldmodel")
    {
        return Some(true);
    }

    if file_name.ends_with(".server.lua")
        || file_name.ends_with(".client.lua")
        || file_name.ends_with(".lua")
        || file_name.ends_with(".folder")
        || file_name.ends_with(".rf")
        || file_name.ends_with(".re")
        || file_name.ends_with(".bf")
        || file_name.ends_with(".be")
        || file_name.ends_with(".screengui")
        || file_name.ends_with(".canvasgroup")
        || file_name.ends_with(".scrollingframe")
        || file_name.ends_with(".surfacegui")
        || file_name.ends_with(".billboardgui")
        || file_name.ends_with(".frame")
        || file_name.ends_with(".textlabel")
        || file_name.ends_with(".textbutton")
        || file_name.ends_with(".textbox")
        || file_name.ends_with(".imagelabel")
        || file_name.ends_with(".imagebutton")
        || file_name.ends_with(".uilistlayout")
        || file_name.ends_with(".uigridlayout")
        || file_name.ends_with(".uipadding")
        || file_name.ends_with(".uicorner")
        || file_name.ends_with(".uistroke")
        || file_name.ends_with(".uishadow")
        || file_name.ends_with(".texture")
        || file_name.ends_with(".decal")
        || file_name.ends_with(".stringvalue")
        || file_name.ends_with(".numbervalue")
        || file_name.ends_with(".intvalue")
        || file_name.ends_with(".boolvalue")
        || file_name.ends_with(".color3value")
        || file_name.ends_with(".vector3value")
    {
        return Some(true);
    }

    None
}

fn fixed_target_class(target: &Path) -> Option<String> {
    let file_name = target.file_name()?.to_str()?;

    if file_name.ends_with(".server.lua") {
        return Some("Script".to_string());
    }
    if file_name.ends_with(".client.lua") {
        return Some("LocalScript".to_string());
    }
    if file_name.ends_with(".service.json") {
        let service_name = file_name.strip_suffix(".service.json")?;
        if service_name.is_empty() {
            return None;
        }
        return Some(service_name.to_string());
    }
    if file_name.ends_with(".model.json") {
        return Some("Model".to_string());
    }
    if file_name.ends_with(".part") {
        return Some("Part".to_string());
    }
    if file_name.ends_with(".worldmodel") {
        return Some("WorldModel".to_string());
    }
    if file_name.ends_with(".model") {
        return Some("Model".to_string());
    }
    if file_name.ends_with(".screengui") {
        return Some("ScreenGui".to_string());
    }
    if file_name.ends_with(".canvasgroup") {
        return Some("CanvasGroup".to_string());
    }
    if file_name.ends_with(".scrollingframe") {
        return Some("ScrollingFrame".to_string());
    }
    if file_name.ends_with(".surfacegui") {
        return Some("SurfaceGui".to_string());
    }
    if file_name.ends_with(".billboardgui") {
        return Some("BillboardGui".to_string());
    }
    if file_name.ends_with(".frame") {
        return Some("Frame".to_string());
    }
    if file_name.ends_with(".textlabel") {
        return Some("TextLabel".to_string());
    }
    if file_name.ends_with(".textbutton") {
        return Some("TextButton".to_string());
    }
    if file_name.ends_with(".textbox") {
        return Some("TextBox".to_string());
    }
    if file_name.ends_with(".imagelabel") {
        return Some("ImageLabel".to_string());
    }
    if file_name.ends_with(".imagebutton") {
        return Some("ImageButton".to_string());
    }
    if file_name.ends_with(".uilistlayout") {
        return Some("UIListLayout".to_string());
    }
    if file_name.ends_with(".uigridlayout") {
        return Some("UIGridLayout".to_string());
    }
    if file_name.ends_with(".uipadding") {
        return Some("UIPadding".to_string());
    }
    if file_name.ends_with(".uicorner") {
        return Some("UICorner".to_string());
    }
    if file_name.ends_with(".uistroke") {
        return Some("UIStroke".to_string());
    }
    if file_name.ends_with(".uishadow") {
        return Some("UIShadow".to_string());
    }
    if file_name.ends_with(".texture") {
        return Some("Texture".to_string());
    }
    if file_name.ends_with(".decal") {
        return Some("Decal".to_string());
    }
    if file_name.ends_with(".stringvalue") {
        return Some("StringValue".to_string());
    }
    if file_name.ends_with(".numbervalue") {
        return Some("NumberValue".to_string());
    }
    if file_name.ends_with(".intvalue") {
        return Some("IntValue".to_string());
    }
    if file_name.ends_with(".boolvalue") {
        return Some("BoolValue".to_string());
    }
    if file_name.ends_with(".color3value") {
        return Some("Color3Value".to_string());
    }
    if file_name.ends_with(".vector3value") {
        return Some("Vector3Value".to_string());
    }
    if file_name.ends_with(".folder") {
        return Some("Folder".to_string());
    }
    if file_name.ends_with(".lua") {
        return Some("ModuleScript".to_string());
    }
    if file_name.ends_with(".rf") {
        return Some("RemoteFunction".to_string());
    }
    if file_name.ends_with(".re") {
        return Some("RemoteEvent".to_string());
    }
    if file_name.ends_with(".bf") {
        return Some("BindableFunction".to_string());
    }
    if file_name.ends_with(".be") {
        return Some("BindableEvent".to_string());
    }

    None
}

fn validate_fixed_target_metadata(target: &Path, metadata: Option<&NodeMetadata>) -> Result<()> {
    let Some(metadata) = metadata else {
        return Ok(());
    };
    let Some(declared_class_name) = metadata.class_name.as_deref() else {
        return Ok(());
    };
    let Some(expected_class_name) = fixed_target_class(target) else {
        return Ok(());
    };

    if declared_class_name == expected_class_name {
        return Ok(());
    }

    bail!(
        "path {} has a fixed local shape and cannot declare className {}; expected {}",
        target.display(),
        declared_class_name,
        expected_class_name
    )
}

fn embedded_file_allows_body(target: &Path) -> bool {
    let Some(file_name) = target.file_name().and_then(|name| name.to_str()) else {
        return false;
    };

    file_name.ends_with(".server.lua")
        || file_name.ends_with(".client.lua")
        || file_name.ends_with(".lua")
        || file_name.ends_with(".part")
        || file_name.ends_with(".model")
        || file_name.ends_with(".worldmodel")
}

fn normalize_metadata(metadata: NodeMetadata) -> Option<NodeMetadata> {
    if metadata.class_name.is_none()
        && metadata.properties.is_empty()
        && metadata.attributes.is_empty()
        && metadata.tags.is_empty()
    {
        return None;
    }

    Some(metadata)
}

fn read_legacy_metadata(target: &Path) -> Result<Option<NodeMetadata>> {
    let Ok(metadata_path) = metadata_path_for_target(target) else {
        return Ok(None);
    };
    if !metadata_path.exists() {
        return Ok(None);
    }

    let contents = fs::read_to_string(&metadata_path)
        .with_context(|| format!("failed to read metadata file {}", metadata_path.display()))?;
    let metadata = serde_json::from_str::<NodeMetadata>(&contents)
        .with_context(|| format!("failed to parse metadata file {}", metadata_path.display()))?;
    Ok(normalize_metadata(metadata))
}

fn remove_legacy_metadata(target: &Path) -> Result<()> {
    let Ok(metadata_path) = metadata_path_for_target(target) else {
        return Ok(());
    };
    if metadata_path.exists() {
        fs::remove_file(&metadata_path).with_context(|| {
            format!(
                "failed to remove migrated metadata file {}",
                metadata_path.display()
            )
        })?;
    }
    Ok(())
}

fn write_embedded_entry_file(
    target: &Path,
    contents_override: Option<String>,
    metadata_override: Option<NodeMetadata>,
    always_embed_header: bool,
) -> Result<()> {
    let mut existing_contents = String::new();
    let mut existing_metadata = None;

    if target.exists() {
        let raw_contents = fs::read_to_string(target)
            .with_context(|| format!("failed to read file {}", target.display()))?;
        let parsed_payload = read_embedded_text_payload(target, &raw_contents)?;
        existing_contents = parsed_payload.contents;
        existing_metadata = normalize_metadata(parsed_payload.metadata.unwrap_or_default());
    }

    if existing_metadata.is_none() {
        existing_metadata = read_legacy_metadata(target)?;
    }

    let contents = contents_override.unwrap_or(existing_contents);
    if !embedded_file_allows_body(target) && !contents.trim().is_empty() {
        bail!(
            "{} stores metadata only and cannot keep a body section",
            target.display()
        );
    }

    let metadata = match metadata_override {
        Some(metadata) => normalize_metadata(metadata),
        None => existing_metadata,
    };
    validate_fixed_target_metadata(target, metadata.as_ref())?;

    let rendered = render_embedded_text_document(
        &contents,
        metadata.clone(),
        always_embed_header || metadata.is_some(),
    )?;

    fs::write(target, rendered)
        .with_context(|| format!("failed to write file {}", target.display()))?;
    remove_legacy_metadata(target)
}

fn resolve_project_path(root: &Path, relative: &str) -> Result<PathBuf> {
    let mut normalized = PathBuf::new();
    for component in Path::new(relative).components() {
        match component {
            Component::CurDir => {}
            Component::Normal(part) => normalized.push(part),
            Component::ParentDir => bail!("path traversal is not allowed: {}", relative),
            Component::RootDir | Component::Prefix(_) => {
                bail!("absolute paths are not allowed: {}", relative)
            }
        }
    }

    if normalized.as_os_str().is_empty() {
        bail!("path cannot be empty");
    }

    Ok(root.join(normalized))
}

fn infer_node_type(target: &Path) -> String {
    if target.extension().is_some() {
        "file".to_string()
    } else {
        "directory".to_string()
    }
}

fn success(data: Value) -> (StatusCode, Json<Value>) {
    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "data": data
        })),
    )
}

fn error_response(status: StatusCode, error: anyhow::Error) -> (StatusCode, Json<Value>) {
    (
        status,
        Json(json!({
            "ok": false,
            "error": {
                "message": error.to_string()
            }
        })),
    )
}

#[cfg(test)]
mod tests {
    use std::fs;

    use crate::model::NodeMetadata;
    use anyhow::Result;
    use serde_json::json;
    use tempfile::tempdir;

    use super::{
        BridgeCommandKind, CreateRequest, DashboardState, PluginCommandResultRequest,
        PluginHeartbeatRequest, StudioNode, StudioSyncMode, StudioSyncRequest, UpdateRequest,
        create_entry_impl, sync_from_studio_impl, update_entry_impl,
    };

    fn studio_sync_request(tree: StudioNode) -> StudioSyncRequest {
        StudioSyncRequest {
            tree,
            mode: StudioSyncMode::Apply,
            force: false,
        }
    }

    #[test]
    fn syncs_scripts_and_remotes_from_studio() -> Result<()> {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path())?;

        let request = studio_sync_request(StudioNode {
            name: "game".to_string(),
            class_name: "DataModel".to_string(),
            children: vec![
                StudioNode {
                    name: "ReplicatedStorage".to_string(),
                    class_name: "ReplicatedStorage".to_string(),
                    children: vec![
                        StudioNode {
                            name: "Shared".to_string(),
                            class_name: "ModuleScript".to_string(),
                            children: vec![],
                            properties: Default::default(),
                            attributes: Default::default(),
                            tags: vec![],
                            source: Some("return 1".to_string()),
                        },
                        StudioNode {
                            name: "Ping".to_string(),
                            class_name: "RemoteEvent".to_string(),
                            children: vec![],
                            properties: Default::default(),
                            attributes: Default::default(),
                            tags: vec![],
                            source: None,
                        },
                    ],
                    properties: Default::default(),
                    attributes: Default::default(),
                    tags: vec![],
                    source: None,
                },
                StudioNode {
                    name: "ServerScriptService".to_string(),
                    class_name: "ServerScriptService".to_string(),
                    children: vec![StudioNode {
                        name: "Boot".to_string(),
                        class_name: "Script".to_string(),
                        children: vec![],
                        properties: std::collections::BTreeMap::from([(
                            "Disabled".to_string(),
                            json!(false),
                        )]),
                        attributes: std::collections::BTreeMap::from([(
                            "Stage".to_string(),
                            json!("Boot"),
                        )]),
                        tags: vec!["Startup".to_string()],
                        source: Some("print('boot')".to_string()),
                    }],
                    properties: Default::default(),
                    attributes: Default::default(),
                    tags: vec![],
                    source: None,
                },
            ],
            properties: Default::default(),
            attributes: Default::default(),
            tags: vec![],
            source: None,
        });

        let response = sync_from_studio_impl(dir.path(), request)?;
        let shared_source = fs::read_to_string(dir.path().join("ReplicatedStorage/Shared.lua"))?;
        assert!(shared_source.contains("--!nyjo"));
        assert!(shared_source.ends_with("return 1"));
        let ping_source = fs::read_to_string(dir.path().join("ReplicatedStorage/Ping.re"))?;
        assert!(ping_source.contains("--!nyjo"));
        let boot_source =
            fs::read_to_string(dir.path().join("ServerScriptService/Boot.server.lua"))?;
        assert!(boot_source.contains("--!nyjo"));
        assert!(boot_source.contains("\"Disabled\": false"));
        assert!(boot_source.contains("\"Stage\": \"Boot\""));
        assert!(boot_source.contains("\"Startup\""));
        assert!(boot_source.ends_with("print('boot')"));
        assert!(response.plan.stats.files_written >= 3);
        assert!(response.applied);

        Ok(())
    }

    #[test]
    fn syncs_script_with_children_from_studio() -> Result<()> {
        let dir = tempdir()?;

        let request = studio_sync_request(StudioNode {
            name: "Studio".to_string(),
            class_name: "DataModel".to_string(),
            children: vec![StudioNode {
                name: "ServerScriptService".to_string(),
                class_name: "ServerScriptService".to_string(),
                children: vec![StudioNode {
                    name: "Main".to_string(),
                    class_name: "Script".to_string(),
                    children: vec![StudioNode {
                        name: "EnemyHandler".to_string(),
                        class_name: "ModuleScript".to_string(),
                        children: vec![],
                        properties: Default::default(),
                        attributes: Default::default(),
                        tags: vec![],
                        source: Some("return { enemy = true }".to_string()),
                    }],
                    properties: Default::default(),
                    attributes: std::collections::BTreeMap::from([(
                        "Role".to_string(),
                        json!("Controller"),
                    )]),
                    tags: vec![],
                    source: Some("print('main')".to_string()),
                }],
                properties: Default::default(),
                attributes: Default::default(),
                tags: vec![],
                source: None,
            }],
            properties: Default::default(),
            attributes: Default::default(),
            tags: vec![],
            source: None,
        });

        let response = sync_from_studio_impl(dir.path(), request)?;
        let tree = response.tree.expect("expected updated tree after apply");
        let init_source =
            fs::read_to_string(dir.path().join("ServerScriptService/Main/init.server.lua"))?;
        assert!(init_source.contains("--!nyjo"));
        assert!(init_source.contains("\"Role\": \"Controller\""));
        assert!(init_source.ends_with("print('main')"));
        let child_source =
            fs::read_to_string(dir.path().join("ServerScriptService/Main/EnemyHandler.lua"))?;
        assert!(child_source.contains("--!nyjo"));
        assert!(child_source.ends_with("return { enemy = true }"));
        assert!(
            !dir.path()
                .join("ServerScriptService/Main/.meta.json")
                .exists()
        );

        let sss = tree
            .children
            .iter()
            .find(|child| child.name == "ServerScriptService")
            .expect("missing ServerScriptService");
        let main = sss
            .children
            .iter()
            .find(|child| child.name == "Main")
            .expect("missing Main script");
        assert_eq!(main.class_name, "Script");
        assert_eq!(main.children.len(), 1);
        assert_eq!(main.children[0].name, "EnemyHandler");

        Ok(())
    }

    #[test]
    fn syncs_nested_parts_into_typed_directories() -> Result<()> {
        let dir = tempdir()?;
        let request = studio_sync_request(StudioNode {
            name: "game".to_string(),
            class_name: "DataModel".to_string(),
            children: vec![StudioNode {
                name: "Workspace".to_string(),
                class_name: "Workspace".to_string(),
                children: vec![StudioNode {
                    name: "Spawn".to_string(),
                    class_name: "Part".to_string(),
                    children: vec![StudioNode {
                        name: "Label".to_string(),
                        class_name: "StringValue".to_string(),
                        children: vec![],
                        properties: std::collections::BTreeMap::from([(
                            "Value".to_string(),
                            json!("Spawn"),
                        )]),
                        attributes: Default::default(),
                        tags: vec![],
                        source: None,
                    }],
                    properties: std::collections::BTreeMap::from([(
                        "Anchored".to_string(),
                        json!(true),
                    )]),
                    attributes: Default::default(),
                    tags: vec!["SpawnPoint".to_string()],
                    source: None,
                }],
                properties: Default::default(),
                attributes: Default::default(),
                tags: vec![],
                source: None,
            }],
            properties: Default::default(),
            attributes: Default::default(),
            tags: vec![],
            source: None,
        });

        sync_from_studio_impl(dir.path(), request)?;
        let part_dir = dir.path().join("Workspace/Spawn.part");
        assert!(part_dir.is_dir());
        let part_header = fs::read_to_string(part_dir.join("init.part"))?;
        assert!(part_header.contains("--!nyjo"));
        assert!(part_header.contains("\"Anchored\": true"));
        assert!(part_header.contains("\"SpawnPoint\""));

        let label_source = fs::read_to_string(part_dir.join("Label.stringvalue"))?;
        assert!(label_source.contains("--!nyjo"));
        assert!(label_source.contains("\"Value\": \"Spawn\""));
        assert!(!part_dir.join(".meta.json").exists());
        assert!(!part_dir.join("Label.meta.json").exists());

        Ok(())
    }

    #[test]
    fn force_pull_serializes_duplicate_child_names_as_compact_json() -> Result<()> {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path().join("Workspace"))?;
        fs::write(dir.path().join("Workspace/Shop.model"), "")?;

        let request = StudioSyncRequest {
            tree: StudioNode {
                name: "game".to_string(),
                class_name: "DataModel".to_string(),
                children: vec![StudioNode {
                    name: "Workspace".to_string(),
                    class_name: "Workspace".to_string(),
                    children: vec![StudioNode {
                        name: "Shop".to_string(),
                        class_name: "Model".to_string(),
                        children: vec![StudioNode {
                            name: "Lantern".to_string(),
                            class_name: "Model".to_string(),
                            children: vec![
                                StudioNode {
                                    name: "Part".to_string(),
                                    class_name: "Part".to_string(),
                                    children: vec![StudioNode {
                                        name: "Surface".to_string(),
                                        class_name: "Texture".to_string(),
                                        children: vec![],
                                        properties: Default::default(),
                                        attributes: Default::default(),
                                        tags: vec![],
                                        source: None,
                                    }],
                                    properties: Default::default(),
                                    attributes: Default::default(),
                                    tags: vec![],
                                    source: None,
                                },
                                StudioNode {
                                    name: "Part".to_string(),
                                    class_name: "Part".to_string(),
                                    children: vec![],
                                    properties: Default::default(),
                                    attributes: Default::default(),
                                    tags: vec![],
                                    source: None,
                                },
                            ],
                            properties: Default::default(),
                            attributes: Default::default(),
                            tags: vec![],
                            source: None,
                        }],
                        properties: Default::default(),
                        attributes: Default::default(),
                        tags: vec![],
                        source: None,
                    }],
                    properties: Default::default(),
                    attributes: Default::default(),
                    tags: vec![],
                    source: None,
                }],
                properties: Default::default(),
                attributes: Default::default(),
                tags: vec![],
                source: None,
            },
            mode: StudioSyncMode::Apply,
            force: true,
        };

        let response = sync_from_studio_impl(dir.path(), request)?;
        assert!(response.applied);
        assert!(dir.path().join("Workspace/Shop.model").is_dir());

        let lantern_document = fs::read_to_string(
            dir.path()
                .join("Workspace/Shop.model/Lantern.instance.json"),
        )?;
        assert!(lantern_document.contains("\"className\": \"Model\""));
        assert!(lantern_document.contains("\"name\": \"Part\""));
        assert!(lantern_document.contains("\"Surface\""));

        let tree = response
            .tree
            .expect("expected updated tree after force pull");
        let workspace = tree
            .children
            .iter()
            .find(|child| child.name == "Workspace")
            .expect("missing Workspace");
        let shop = workspace
            .children
            .iter()
            .find(|child| child.name == "Shop")
            .expect("missing Shop");
        let lantern = shop
            .children
            .iter()
            .find(|child| child.name == "Lantern")
            .expect("missing Lantern");
        assert_eq!(
            lantern
                .children
                .iter()
                .filter(|child| child.name == "Part")
                .count(),
            2
        );

        Ok(())
    }

    #[test]
    fn force_pull_replaces_file_parent_with_directory_for_nested_children() -> Result<()> {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path().join("Workspace/Shop.model/Lantern.model"))?;
        fs::write(
            dir.path()
                .join("Workspace/Shop.model/Lantern.model/Part.part"),
            "",
        )?;

        let request = StudioSyncRequest {
            tree: StudioNode {
                name: "game".to_string(),
                class_name: "DataModel".to_string(),
                children: vec![StudioNode {
                    name: "Workspace".to_string(),
                    class_name: "Workspace".to_string(),
                    children: vec![StudioNode {
                        name: "Shop".to_string(),
                        class_name: "Model".to_string(),
                        children: vec![StudioNode {
                            name: "Lantern".to_string(),
                            class_name: "Model".to_string(),
                            children: vec![StudioNode {
                                name: "Part".to_string(),
                                class_name: "Part".to_string(),
                                children: vec![StudioNode {
                                    name: "Mesh".to_string(),
                                    class_name: "SpecialMesh".to_string(),
                                    children: vec![],
                                    properties: Default::default(),
                                    attributes: Default::default(),
                                    tags: vec![],
                                    source: None,
                                }],
                                properties: Default::default(),
                                attributes: Default::default(),
                                tags: vec![],
                                source: None,
                            }],
                            properties: Default::default(),
                            attributes: Default::default(),
                            tags: vec![],
                            source: None,
                        }],
                        properties: Default::default(),
                        attributes: Default::default(),
                        tags: vec![],
                        source: None,
                    }],
                    properties: Default::default(),
                    attributes: Default::default(),
                    tags: vec![],
                    source: None,
                }],
                properties: Default::default(),
                attributes: Default::default(),
                tags: vec![],
                source: None,
            },
            mode: StudioSyncMode::Apply,
            force: true,
        };

        let response = sync_from_studio_impl(dir.path(), request)?;
        assert!(response.applied);
        let part_dir = dir
            .path()
            .join("Workspace/Shop.model/Lantern.model/Part.part");
        assert!(part_dir.is_dir());
        assert!(part_dir.join("Mesh.instance.json").is_file());

        Ok(())
    }

    #[test]
    fn force_pull_replaces_file_target_with_directory() -> Result<()> {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path().join("Workspace/Spacegate.model"))?;
        fs::write(dir.path().join("Workspace/Spacegate.model/Part.part"), "")?;

        let request = StudioSyncRequest {
            tree: StudioNode {
                name: "game".to_string(),
                class_name: "DataModel".to_string(),
                children: vec![StudioNode {
                    name: "Workspace".to_string(),
                    class_name: "Workspace".to_string(),
                    children: vec![StudioNode {
                        name: "Spacegate".to_string(),
                        class_name: "Model".to_string(),
                        children: vec![StudioNode {
                            name: "Part".to_string(),
                            class_name: "Part".to_string(),
                            children: vec![StudioNode {
                                name: "Decal".to_string(),
                                class_name: "Decal".to_string(),
                                children: vec![],
                                properties: Default::default(),
                                attributes: Default::default(),
                                tags: vec![],
                                source: None,
                            }],
                            properties: Default::default(),
                            attributes: Default::default(),
                            tags: vec![],
                            source: None,
                        }],
                        properties: Default::default(),
                        attributes: Default::default(),
                        tags: vec![],
                        source: None,
                    }],
                    properties: Default::default(),
                    attributes: Default::default(),
                    tags: vec![],
                    source: None,
                }],
                properties: Default::default(),
                attributes: Default::default(),
                tags: vec![],
                source: None,
            },
            mode: StudioSyncMode::Apply,
            force: true,
        };

        let response = sync_from_studio_impl(dir.path(), request)?;
        assert!(response.applied);
        let part_dir = dir.path().join("Workspace/Spacegate.model/Part.part");
        assert!(part_dir.is_dir());
        assert!(part_dir.join("Decal.decal").is_file());

        Ok(())
    }

    #[test]
    fn syncs_nested_ui_trees_into_typed_directories() -> Result<()> {
        let dir = tempdir()?;
        let request = studio_sync_request(StudioNode {
            name: "game".to_string(),
            class_name: "DataModel".to_string(),
            children: vec![StudioNode {
                name: "StarterGui".to_string(),
                class_name: "StarterGui".to_string(),
                children: vec![StudioNode {
                    name: "Hud".to_string(),
                    class_name: "ScreenGui".to_string(),
                    children: vec![StudioNode {
                        name: "Play".to_string(),
                        class_name: "TextButton".to_string(),
                        children: vec![
                            StudioNode {
                                name: "Corner".to_string(),
                                class_name: "UICorner".to_string(),
                                children: vec![],
                                properties: std::collections::BTreeMap::from([
                                    (
                                        "TopLeftRadius".to_string(),
                                        json!({
                                            "__nyjoType": "UDim",
                                            "scale": 0,
                                            "offset": 0
                                        }),
                                    ),
                                    (
                                        "TopRightRadius".to_string(),
                                        json!({
                                            "__nyjoType": "UDim",
                                            "scale": 0,
                                            "offset": 20
                                        }),
                                    ),
                                    (
                                        "BottomRightRadius".to_string(),
                                        json!({
                                            "__nyjoType": "UDim",
                                            "scale": 0.25,
                                            "offset": 0
                                        }),
                                    ),
                                    (
                                        "BottomLeftRadius".to_string(),
                                        json!({
                                            "__nyjoType": "UDim",
                                            "scale": 0,
                                            "offset": 6
                                        }),
                                    ),
                                ]),
                                attributes: Default::default(),
                                tags: vec![],
                                source: None,
                            },
                            StudioNode {
                                name: "Shadow".to_string(),
                                class_name: "UIShadow".to_string(),
                                children: vec![],
                                properties: std::collections::BTreeMap::from([
                                    (
                                        "BlurRadius".to_string(),
                                        json!({
                                            "__nyjoType": "UDim",
                                            "scale": 0,
                                            "offset": 18
                                        }),
                                    ),
                                    (
                                        "Color".to_string(),
                                        json!({
                                            "__nyjoType": "Color3",
                                            "r": 0.05,
                                            "g": 0.05,
                                            "b": 0.08
                                        }),
                                    ),
                                    ("Enabled".to_string(), json!(true)),
                                    (
                                        "Offset".to_string(),
                                        json!({
                                            "__nyjoType": "UDim2",
                                            "x": { "scale": 0, "offset": 2 },
                                            "y": { "scale": 0, "offset": 6 }
                                        }),
                                    ),
                                    ("Transparency".to_string(), json!(0.4)),
                                ]),
                                attributes: Default::default(),
                                tags: vec![],
                                source: None,
                            },
                        ],
                        properties: std::collections::BTreeMap::from([(
                            "Text".to_string(),
                            json!("Play"),
                        )]),
                        attributes: Default::default(),
                        tags: vec![],
                        source: None,
                    }],
                    properties: std::collections::BTreeMap::from([(
                        "ResetOnSpawn".to_string(),
                        json!(false),
                    )]),
                    attributes: Default::default(),
                    tags: vec![],
                    source: None,
                }],
                properties: Default::default(),
                attributes: Default::default(),
                tags: vec![],
                source: None,
            }],
            properties: Default::default(),
            attributes: Default::default(),
            tags: vec![],
            source: None,
        });

        sync_from_studio_impl(dir.path(), request)?;
        let hud_dir = dir.path().join("StarterGui/Hud.screengui");
        assert!(hud_dir.is_dir());
        let hud_header = fs::read_to_string(hud_dir.join("init.screengui"))?;
        assert!(hud_header.contains("--!nyjo"));
        assert!(hud_header.contains("\"ResetOnSpawn\": false"));

        let play_dir = hud_dir.join("Play.textbutton");
        assert!(play_dir.is_dir());
        let play_header = fs::read_to_string(play_dir.join("init.textbutton"))?;
        assert!(play_header.contains("--!nyjo"));
        assert!(play_header.contains("\"Text\": \"Play\""));

        let corner_source = fs::read_to_string(play_dir.join("Corner.uicorner"))?;
        assert!(corner_source.contains("--!nyjo"));
        assert!(corner_source.contains("\"TopRightRadius\""));

        let shadow_source = fs::read_to_string(play_dir.join("Shadow.uishadow"))?;
        assert!(shadow_source.contains("--!nyjo"));
        assert!(shadow_source.contains("\"BlurRadius\""));
        assert!(shadow_source.contains("\"Enabled\": true"));

        Ok(())
    }

    #[test]
    fn create_entry_embeds_metadata_into_script_files() -> Result<()> {
        let dir = tempdir()?;
        let tree = create_entry_impl(
            dir.path(),
            CreateRequest {
                path: "ServerScriptService/Boot.server.lua".to_string(),
                node_type: Some("file".to_string()),
                contents: Some("print('boot')".to_string()),
                metadata: Some(NodeMetadata {
                    class_name: None,
                    properties: Default::default(),
                    attributes: std::collections::BTreeMap::from([(
                        "Stage".to_string(),
                        json!("Boot"),
                    )]),
                    tags: vec!["Startup".to_string()],
                }),
                overwrite: false,
            },
        )?;

        let source = fs::read_to_string(dir.path().join("ServerScriptService/Boot.server.lua"))?;
        assert!(source.contains("--!nyjo"));
        assert!(source.contains("\"Stage\": \"Boot\""));
        assert!(source.contains("\"Startup\""));
        assert!(
            !dir.path()
                .join("ServerScriptService/Boot.meta.json")
                .exists()
        );
        assert!(
            tree.children
                .iter()
                .any(|child| child.name == "ServerScriptService")
        );

        Ok(())
    }

    #[test]
    fn create_entry_writes_metadata_for_typed_directories() -> Result<()> {
        let dir = tempdir()?;
        let tree = create_entry_impl(
            dir.path(),
            CreateRequest {
                path: "Workspace/Spawn.part".to_string(),
                node_type: Some("directory".to_string()),
                contents: None,
                metadata: Some(NodeMetadata {
                    class_name: None,
                    properties: std::collections::BTreeMap::from([(
                        "Anchored".to_string(),
                        json!(true),
                    )]),
                    attributes: Default::default(),
                    tags: vec!["SpawnPoint".to_string()],
                }),
                overwrite: false,
            },
        )?;

        let header = fs::read_to_string(dir.path().join("Workspace/Spawn.part/init.part"))?;
        assert!(header.contains("--!nyjo"));
        assert!(header.contains("\"Anchored\": true"));
        assert!(header.contains("\"SpawnPoint\""));
        assert!(!dir.path().join("Workspace/Spawn.part/.meta.json").exists());

        let workspace = tree
            .children
            .iter()
            .find(|child| child.name == "Workspace")
            .expect("missing Workspace");
        let spawn = workspace
            .children
            .iter()
            .find(|child| child.name == "Spawn")
            .expect("missing Spawn");
        assert_eq!(spawn.class_name, "Part");

        Ok(())
    }

    #[test]
    fn create_entry_writes_generic_directory_header_file() -> Result<()> {
        let dir = tempdir()?;
        let tree = create_entry_impl(
            dir.path(),
            CreateRequest {
                path: "Workspace/Quest".to_string(),
                node_type: Some("directory".to_string()),
                contents: None,
                metadata: Some(NodeMetadata {
                    class_name: None,
                    properties: Default::default(),
                    attributes: std::collections::BTreeMap::from([(
                        "Stage".to_string(),
                        json!("Lobby"),
                    )]),
                    tags: vec!["Tracked".to_string()],
                }),
                overwrite: false,
            },
        )?;

        let header = fs::read_to_string(dir.path().join("Workspace/Quest/.nyjo"))?;
        assert!(header.contains("--!nyjo"));
        assert!(header.contains("\"Stage\": \"Lobby\""));
        assert!(header.contains("\"Tracked\""));
        assert!(!dir.path().join("Workspace/Quest/.meta.json").exists());

        let workspace = tree
            .children
            .iter()
            .find(|child| child.name == "Workspace")
            .expect("missing Workspace");
        let quest = workspace
            .children
            .iter()
            .find(|child| child.name == "Quest")
            .expect("missing Quest");
        assert_eq!(quest.class_name, "Folder");

        Ok(())
    }

    #[test]
    fn update_entry_migrates_sidecar_metadata_into_embedded_header() -> Result<()> {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path().join("ReplicatedStorage"))?;
        fs::write(dir.path().join("ReplicatedStorage/Shared.lua"), "return 1")?;
        fs::write(
            dir.path().join("ReplicatedStorage/Shared.meta.json"),
            r#"{
  "attributes": {
    "Tier": "Core"
  }
}"#,
        )?;

        update_entry_impl(
            dir.path(),
            UpdateRequest {
                path: "ReplicatedStorage/Shared.lua".to_string(),
                contents: Some("return 2".to_string()),
                metadata: None,
            },
        )?;

        let source = fs::read_to_string(dir.path().join("ReplicatedStorage/Shared.lua"))?;
        assert!(source.contains("--!nyjo"));
        assert!(source.contains("\"Tier\": \"Core\""));
        assert!(source.ends_with("return 2"));
        assert!(
            !dir.path()
                .join("ReplicatedStorage/Shared.meta.json")
                .exists()
        );

        Ok(())
    }

    #[test]
    fn update_entry_keeps_header_when_clearing_script_metadata() -> Result<()> {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path().join("ReplicatedStorage"))?;
        fs::write(
            dir.path().join("ReplicatedStorage/Shared.lua"),
            r#"--!nyjo
--HEADER
--{
--  "attributes": {
--    "Tier": "Core"
--  }
--}
--CONTENTS
return 1"#,
        )?;

        update_entry_impl(
            dir.path(),
            UpdateRequest {
                path: "ReplicatedStorage/Shared.lua".to_string(),
                contents: Some("return 3".to_string()),
                metadata: Some(NodeMetadata::default()),
            },
        )?;

        let source = fs::read_to_string(dir.path().join("ReplicatedStorage/Shared.lua"))?;
        assert!(source.contains("--!nyjo"));
        assert!(source.ends_with("return 3"));

        Ok(())
    }

    #[test]
    fn create_entry_rejects_mismatched_metadata_class_for_fixed_file_shape() -> Result<()> {
        let dir = tempdir()?;
        let error = create_entry_impl(
            dir.path(),
            CreateRequest {
                path: "ServerScriptService/Boot.server.lua".to_string(),
                node_type: Some("file".to_string()),
                contents: Some("print('boot')".to_string()),
                metadata: Some(NodeMetadata {
                    class_name: Some("LocalScript".to_string()),
                    properties: Default::default(),
                    attributes: Default::default(),
                    tags: vec![],
                }),
                overwrite: false,
            },
        )
        .expect_err("expected mismatched class metadata to be rejected");

        assert!(error.to_string().contains("fixed local shape"));
        assert!(error.to_string().contains("expected Script"));

        Ok(())
    }

    #[test]
    fn update_entry_rejects_body_contents_for_marker_files() -> Result<()> {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path().join("ReplicatedStorage"))?;
        fs::write(dir.path().join("ReplicatedStorage/Ping.re"), "")?;

        let error = update_entry_impl(
            dir.path(),
            UpdateRequest {
                path: "ReplicatedStorage/Ping.re".to_string(),
                contents: Some("should fail".to_string()),
                metadata: None,
            },
        )
        .expect_err("expected marker file body write to fail");

        assert!(error.to_string().contains("stores metadata only"));

        Ok(())
    }

    #[test]
    fn preview_pull_reports_changes_without_writing() -> Result<()> {
        let dir = tempdir()?;
        let request = StudioSyncRequest {
            tree: StudioNode {
                name: "Studio".to_string(),
                class_name: "DataModel".to_string(),
                children: vec![StudioNode {
                    name: "ReplicatedStorage".to_string(),
                    class_name: "ReplicatedStorage".to_string(),
                    children: vec![StudioNode {
                        name: "Shared".to_string(),
                        class_name: "ModuleScript".to_string(),
                        children: vec![],
                        properties: Default::default(),
                        attributes: Default::default(),
                        tags: vec![],
                        source: Some("return 42".to_string()),
                    }],
                    properties: Default::default(),
                    attributes: Default::default(),
                    tags: vec![],
                    source: None,
                }],
                properties: Default::default(),
                attributes: Default::default(),
                tags: vec![],
                source: None,
            },
            mode: StudioSyncMode::Preview,
            force: false,
        };

        let response = sync_from_studio_impl(dir.path(), request)?;
        assert!(!response.applied);
        assert!(!response.blocked);
        assert!(response.tree.is_none());
        assert!(response.plan.stats.files_planned >= 1);
        assert!(!dir.path().join("ReplicatedStorage/Shared.lua").exists());

        Ok(())
    }

    #[test]
    fn apply_pull_blocks_conflicting_local_file_without_force() -> Result<()> {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path().join("ReplicatedStorage"))?;
        fs::write(
            dir.path().join("ReplicatedStorage/Shared.lua"),
            "return 'local'",
        )?;

        let request = studio_sync_request(StudioNode {
            name: "Studio".to_string(),
            class_name: "DataModel".to_string(),
            children: vec![StudioNode {
                name: "ReplicatedStorage".to_string(),
                class_name: "ReplicatedStorage".to_string(),
                children: vec![StudioNode {
                    name: "Shared".to_string(),
                    class_name: "ModuleScript".to_string(),
                    children: vec![],
                    properties: Default::default(),
                    attributes: Default::default(),
                    tags: vec![],
                    source: Some("return 'studio'".to_string()),
                }],
                properties: Default::default(),
                attributes: Default::default(),
                tags: vec![],
                source: None,
            }],
            properties: Default::default(),
            attributes: Default::default(),
            tags: vec![],
            source: None,
        });

        let response = sync_from_studio_impl(dir.path(), request)?;
        assert!(response.blocked);
        assert!(!response.applied);
        assert_eq!(response.plan.stats.conflicts, 1);
        assert_eq!(
            fs::read_to_string(dir.path().join("ReplicatedStorage/Shared.lua"))?,
            "return 'local'"
        );

        Ok(())
    }

    #[test]
    fn force_pull_creates_local_backup_that_can_be_restored() -> Result<()> {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path().join("ReplicatedStorage"))?;
        fs::write(
            dir.path().join("ReplicatedStorage/Shared.lua"),
            "return 'local'",
        )?;

        let request = StudioSyncRequest {
            tree: StudioNode {
                name: "Studio".to_string(),
                class_name: "DataModel".to_string(),
                children: vec![StudioNode {
                    name: "ReplicatedStorage".to_string(),
                    class_name: "ReplicatedStorage".to_string(),
                    children: vec![StudioNode {
                        name: "Shared".to_string(),
                        class_name: "ModuleScript".to_string(),
                        children: vec![],
                        properties: Default::default(),
                        attributes: Default::default(),
                        tags: vec![],
                        source: Some("return 'studio'".to_string()),
                    }],
                    properties: Default::default(),
                    attributes: Default::default(),
                    tags: vec![],
                    source: None,
                }],
                properties: Default::default(),
                attributes: Default::default(),
                tags: vec![],
                source: None,
            },
            mode: StudioSyncMode::Apply,
            force: true,
        };

        let response = sync_from_studio_impl(dir.path(), request)?;
        let backup = response
            .backup
            .clone()
            .expect("expected apply pull backup to be created");

        let synced = fs::read_to_string(dir.path().join("ReplicatedStorage/Shared.lua"))?;
        assert!(synced.contains("--!nyjo"));
        assert!(synced.ends_with("return 'studio'"));

        let report = crate::backup::restore_local_project_backup(dir.path(), Some(&backup.id))?;
        assert_eq!(report.restored_backup.id, backup.id);
        assert_eq!(
            fs::read_to_string(dir.path().join("ReplicatedStorage/Shared.lua"))?,
            "return 'local'"
        );

        Ok(())
    }

    #[test]
    fn dashboard_rejects_commands_without_bridge_heartbeat() {
        let mut dashboard = DashboardState::default();
        let error = dashboard
            .queue_command(BridgeCommandKind::PushLocalTree, None)
            .expect_err("expected disconnected bridge to reject commands");
        assert!(error.to_string().contains("not connected"));
    }

    #[test]
    fn dashboard_requires_selection_when_multiple_places_are_connected() {
        let mut dashboard = DashboardState::default();
        dashboard.update_plugin_heartbeat(PluginHeartbeatRequest {
            session_id: "session-a".to_string(),
            bridge_version: Some("test-bridge".to_string()),
            place_name: Some("PlaceA".to_string()),
            status: Some("idle".to_string()),
            place_id: Some(101),
        });
        dashboard.update_plugin_heartbeat(PluginHeartbeatRequest {
            session_id: "session-b".to_string(),
            bridge_version: Some("test-bridge".to_string()),
            place_name: Some("PlaceB".to_string()),
            status: Some("idle".to_string()),
            place_id: Some(202),
        });

        let error = dashboard
            .queue_command(BridgeCommandKind::PreviewPull, None)
            .expect_err("expected multi-place bridge to require target selection");
        assert!(
            error
                .to_string()
                .contains("multiple Studio places are connected")
        );
    }

    #[test]
    fn dashboard_places_snapshot_tracks_place_ids_and_selection() -> Result<()> {
        let mut dashboard = DashboardState::default();
        dashboard.update_plugin_heartbeat(PluginHeartbeatRequest {
            session_id: "session-a".to_string(),
            bridge_version: Some("test-bridge".to_string()),
            place_name: Some("PlaceA".to_string()),
            status: Some("idle".to_string()),
            place_id: Some(101),
        });
        dashboard.update_plugin_heartbeat(PluginHeartbeatRequest {
            session_id: "session-b".to_string(),
            bridge_version: Some("test-bridge".to_string()),
            place_name: Some("PlaceB".to_string()),
            status: Some("idle".to_string()),
            place_id: Some(202),
        });

        dashboard.set_selected_session(Some("session-b".to_string()))?;
        let snapshot = dashboard.places_snapshot();

        assert!(snapshot.connected);
        assert_eq!(snapshot.connected_sessions, 2);
        assert_eq!(snapshot.selected_session_id.as_deref(), Some("session-b"));
        assert_eq!(snapshot.target_session_id.as_deref(), Some("session-b"));
        assert!(!snapshot.selection_required);

        let place_b = snapshot
            .sessions
            .iter()
            .find(|session| session.session_id == "session-b")
            .expect("expected selected place to be present");
        assert_eq!(place_b.place_id, Some(202));
        assert_eq!(place_b.place_name.as_deref(), Some("PlaceB"));
        assert!(place_b.selected);

        Ok(())
    }

    #[test]
    fn dashboard_command_lifecycle_moves_from_pending_to_result() -> Result<()> {
        let mut dashboard = DashboardState::default();
        dashboard.update_plugin_heartbeat(PluginHeartbeatRequest {
            session_id: "session-a".to_string(),
            bridge_version: Some("test-bridge".to_string()),
            place_name: Some("UnitTest".to_string()),
            status: Some("idle".to_string()),
            place_id: Some(101),
        });

        let queued = dashboard.queue_command(BridgeCommandKind::PreviewPull, None)?;
        assert_eq!(queued.id, 1);
        assert!(dashboard.pending_command.is_some());

        let dispatched = dashboard
            .take_pending_command("session-a")
            .expect("expected pending command to dispatch");
        assert_eq!(dispatched.id, queued.id);
        assert_eq!(dispatched.target_session_id, "session-a");
        assert!(dashboard.pending_command.is_none());
        assert!(dashboard.active_command.is_some());

        let result = dashboard.finish_command(PluginCommandResultRequest {
            command_id: dispatched.id,
            session_id: "session-a".to_string(),
            ok: true,
            summary: "Preview ready".to_string(),
            detail: Some("No conflicts".to_string()),
            data: Some(json!({ "blocked": false })),
        })?;

        assert!(dashboard.active_command.is_none());
        assert_eq!(result.summary, "Preview ready");
        assert_eq!(
            dashboard.last_result.as_ref().map(|value| value.command_id),
            Some(1)
        );
        assert!(!dashboard.logs.is_empty());

        Ok(())
    }

    #[test]
    fn dashboard_only_dispatches_to_the_target_session() -> Result<()> {
        let mut dashboard = DashboardState::default();
        dashboard.update_plugin_heartbeat(PluginHeartbeatRequest {
            session_id: "session-a".to_string(),
            bridge_version: Some("test-bridge".to_string()),
            place_name: Some("PlaceA".to_string()),
            status: Some("idle".to_string()),
            place_id: Some(101),
        });
        dashboard.update_plugin_heartbeat(PluginHeartbeatRequest {
            session_id: "session-b".to_string(),
            bridge_version: Some("test-bridge".to_string()),
            place_name: Some("PlaceB".to_string()),
            status: Some("idle".to_string()),
            place_id: Some(202),
        });

        let queued =
            dashboard.queue_command(BridgeCommandKind::PushLocalTree, Some("session-b"))?;
        assert!(dashboard.take_pending_command("session-a").is_none());

        let dispatched = dashboard
            .take_pending_command("session-b")
            .expect("expected session-b to receive the queued command");
        assert_eq!(dispatched.id, queued.id);
        assert_eq!(dispatched.target_place_name.as_deref(), Some("PlaceB"));

        let wrong_session = dashboard
            .finish_command(PluginCommandResultRequest {
                command_id: dispatched.id,
                session_id: "session-a".to_string(),
                ok: true,
                summary: "wrong".to_string(),
                detail: None,
                data: None,
            })
            .expect_err("expected wrong session result to be rejected");
        assert!(wrong_session.to_string().contains("belongs to session"));
        assert!(dashboard.active_command.is_some());

        assert!(dashboard.last_result.is_none());
        Ok(())
    }
}
