use std::fs;
use std::net::{IpAddr, Ipv4Addr};
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::Html;
use axum::response::IntoResponse;
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{Value, json};
use tokio::net::TcpListener;

use crate::model::{InstanceNode, NodeMetadata};
use crate::parser::{metadata_path_for_target, read_embedded_text_payload, scan_project};

#[derive(Clone)]
struct AppState {
    root: PathBuf,
    dashboard: Arc<Mutex<DashboardState>>,
}

const SUPPORTED_ROOT_SERVICE_CLASSES: &[&str] = &[
    "Lighting",
    "ReplicatedFirst",
    "ReplicatedStorage",
    "ServerScriptService",
    "ServerStorage",
    "SoundService",
    "StarterGui",
    "StarterPlayer",
    "Teams",
    "TextChatService",
    "Workspace",
];

const LOGICAL_NODE_FILE_SUFFIXES: &[&str] = &[
    ".server.lua",
    ".client.lua",
    ".model.json",
    ".instance.json",
    ".service.json",
    ".worldmodel",
    ".model",
    ".part",
    ".folder",
    ".lua",
    ".rf",
    ".re",
    ".bf",
    ".be",
];

const DASHBOARD_HTML: &str = include_str!("../assets/dashboard.html");
const LOG_BUFFER_LIMIT: usize = 200;
const PLUGIN_CONNECTION_TIMEOUT_MS: u64 = 5_000;

#[derive(Debug, Default)]
struct DashboardState {
    next_command_id: u64,
    pending_command: Option<PluginCommandEnvelope>,
    active_command: Option<PluginCommandEnvelope>,
    plugin_last_seen_ms: Option<u64>,
    plugin_bridge_version: Option<String>,
    plugin_place_name: Option<String>,
    plugin_status: Option<String>,
    last_result: Option<PluginCommandResultRecord>,
    logs: Vec<DashboardLogEntry>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
enum PluginCommandKind {
    PushLocalTree,
    PreviewPull,
    ApplyPull,
    ForcePull,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginCommandEnvelope {
    id: u64,
    kind: PluginCommandKind,
    requested_at_ms: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct DashboardLogEntry {
    timestamp_ms: u64,
    level: String,
    message: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PluginCommandResultRecord {
    command_id: u64,
    kind: PluginCommandKind,
    ok: bool,
    summary: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
    finished_at_ms: u64,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DashboardPluginSnapshot {
    connected: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    bridge_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    place_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_seen_ms: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_command: Option<PluginCommandEnvelope>,
    #[serde(skip_serializing_if = "Option::is_none")]
    active_command: Option<PluginCommandEnvelope>,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct DashboardSnapshot {
    server: Value,
    plugin: DashboardPluginSnapshot,
    #[serde(skip_serializing_if = "Option::is_none")]
    last_result: Option<PluginCommandResultRecord>,
    logs: Vec<DashboardLogEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct DashboardCommandRequest {
    kind: PluginCommandKind,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginHeartbeatRequest {
    #[serde(default)]
    bridge_version: Option<String>,
    #[serde(default)]
    place_name: Option<String>,
    #[serde(default)]
    status: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PluginCommandResultRequest {
    command_id: u64,
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

#[derive(Debug, Clone, Deserialize)]
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

impl PluginCommandKind {
    fn label(self) -> &'static str {
        match self {
            Self::PushLocalTree => "push local tree to Studio",
            Self::PreviewPull => "preview pull from Studio",
            Self::ApplyPull => "apply pull from Studio",
            Self::ForcePull => "force pull from Studio",
        }
    }
}

impl DashboardState {
    fn queue_command(&mut self, kind: PluginCommandKind) -> Result<PluginCommandEnvelope> {
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

        self.next_command_id += 1;
        let command = PluginCommandEnvelope {
            id: self.next_command_id,
            kind,
            requested_at_ms: now_ms(),
        };
        self.log(
            "info",
            format!("Queued command #{}: {}", command.id, kind.label()),
        );
        self.pending_command = Some(command.clone());
        Ok(command)
    }

    fn take_pending_command(&mut self) -> Option<PluginCommandEnvelope> {
        if self.active_command.is_some() {
            return None;
        }

        let command = self.pending_command.take()?;
        self.log(
            "info",
            format!("Dispatched command #{} to Studio bridge", command.id),
        );
        self.active_command = Some(command.clone());
        Some(command)
    }

    fn update_plugin_heartbeat(&mut self, heartbeat: PluginHeartbeatRequest) {
        let was_connected = self.plugin_connected();
        self.plugin_last_seen_ms = Some(now_ms());
        self.plugin_bridge_version = heartbeat.bridge_version;
        self.plugin_place_name = heartbeat.place_name;
        self.plugin_status = heartbeat.status;

        if !was_connected {
            let place = self
                .plugin_place_name
                .clone()
                .unwrap_or_else(|| "unknown place".to_string());
            self.log("info", format!("Studio bridge connected from {place}"));
        }
    }

    fn finish_command(
        &mut self,
        result: PluginCommandResultRequest,
    ) -> Result<PluginCommandResultRecord> {
        let active = self
            .active_command
            .take()
            .with_context(|| "no active Studio bridge command to finish")?;

        if active.id != result.command_id {
            bail!(
                "received result for command #{} but active command is #{}",
                result.command_id,
                active.id
            );
        }

        let record = PluginCommandResultRecord {
            command_id: result.command_id,
            kind: active.kind,
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
                "Studio bridge finished command #{} ({}): {}",
                record.command_id,
                record.kind.label(),
                record.summary
            ),
        );
        self.last_result = Some(record.clone());
        Ok(record)
    }

    fn snapshot(&self, root: &Path) -> DashboardSnapshot {
        DashboardSnapshot {
            server: json!({
                "version": env!("CARGO_PKG_VERSION"),
                "binding": "127.0.0.1",
                "projectRoot": root.display().to_string(),
                "dashboardPath": "/"
            }),
            plugin: DashboardPluginSnapshot {
                connected: self.plugin_connected(),
                bridge_version: self.plugin_bridge_version.clone(),
                place_name: self.plugin_place_name.clone(),
                status: self.plugin_status.clone(),
                last_seen_ms: self.plugin_last_seen_ms,
                pending_command: self.pending_command.clone(),
                active_command: self.active_command.clone(),
            },
            last_result: self.last_result.clone(),
            logs: self.logs.clone(),
        }
    }

    fn plugin_connected(&self) -> bool {
        self.plugin_last_seen_ms.is_some_and(|timestamp| {
            now_ms().saturating_sub(timestamp) <= PLUGIN_CONNECTION_TIMEOUT_MS
        })
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
        .route("/api/health", get(health))
        .route("/api/info", get(info))
        .route("/api/dashboard/state", get(dashboard_state))
        .route("/api/dashboard/command", post(queue_dashboard_command))
        .route("/api/plugin/heartbeat", post(plugin_heartbeat))
        .route("/api/plugin/poll", get(plugin_poll))
        .route("/api/plugin/command-result", post(plugin_command_result))
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
            "treePreview": true,
            "studioPush": true,
            "studioPullPreview": true,
            "studioPullApply": true,
            "studioPullForce": true,
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
            "rootServices": SUPPORTED_ROOT_SERVICE_CLASSES,
            "typedValues": [
                "boolean",
                "number",
                "string",
                "Color3",
                "Vector2",
                "Vector3",
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
                "UIStroke"
            ]
        }
    }))
}

async fn dashboard_state(State(state): State<AppState>) -> impl IntoResponse {
    match state.dashboard.lock() {
        Ok(dashboard) => success(json!(dashboard.snapshot(&state.root))),
        Err(_) => error_response(
            StatusCode::INTERNAL_SERVER_ERROR,
            anyhow::anyhow!("dashboard state lock was poisoned"),
        ),
    }
}

async fn queue_dashboard_command(
    State(state): State<AppState>,
    Json(request): Json<DashboardCommandRequest>,
) -> impl IntoResponse {
    match state.dashboard.lock() {
        Ok(mut dashboard) => match dashboard.queue_command(request.kind) {
            Ok(command) => success(json!({
                "queued": true,
                "command": command,
                "message": format!("Queued {}", request.kind.label())
            })),
            Err(error) => error_response(StatusCode::CONFLICT, error),
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

async fn plugin_poll(State(state): State<AppState>) -> impl IntoResponse {
    match state.dashboard.lock() {
        Ok(mut dashboard) => success(json!({
            "command": dashboard.take_pending_command()
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

fn create_entry_impl(root: &Path, request: CreateRequest) -> Result<crate::model::InstanceNode> {
    let target = resolve_project_path(root, &request.path)?;
    let node_type = request
        .node_type
        .unwrap_or_else(|| infer_node_type(&target));

    match node_type.as_str() {
        "directory" => {
            if target.exists() && !target.is_dir() {
                bail!("{} already exists as a file", request.path);
            }
            fs::create_dir_all(&target)
                .with_context(|| format!("failed to create directory {}", target.display()))?;
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
            write_metadata(&target, metadata)?;
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
    let mut tree = None;

    if request.mode == StudioSyncMode::Apply && !blocked {
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
        if node_has_container_children(node) {
            return self.plan_container_node(parent_dir, parent_class, node);
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
            self.plan_metadata_target(
                &target,
                false,
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

        self.plan_metadata_target(
            &target_dir,
            true,
            metadata_from_studio_node(node, true, parent_class),
            format!("metadata for {}", node.name),
        )?;

        for child in &node.children {
            self.plan_studio_node(&target_dir, Some(&node.class_name), child)?;
        }

        Ok(())
    }

    fn plan_container_node(
        &mut self,
        parent_dir: &Path,
        parent_class: Option<&str>,
        node: &StudioNode,
    ) -> Result<()> {
        let target_dir = parent_dir.join(&node.name);
        if !self.plan_directory_target(
            &target_dir,
            format!("directory-backed {} container", node.class_name),
        )? {
            return Ok(());
        }

        self.plan_metadata_target(
            &target_dir,
            true,
            metadata_from_studio_node(node, true, parent_class),
            format!("metadata for {}", node.name),
        )?;

        if let Some((source_file_name, contents)) = container_source_file(node) {
            let rendered_source = render_embedded_text_document(
                contents.as_str(),
                metadata_from_studio_node(node, false, parent_class),
                false,
            )?;
            self.plan_text_file_target(
                &target_dir.join(source_file_name),
                rendered_source,
                "file",
                format!("container source for {}", node.name),
            )?;
        }

        for child in &node.children {
            self.plan_studio_node(&target_dir, Some(&node.class_name), child)?;
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

    fn plan_metadata_target(
        &mut self,
        target: &Path,
        is_directory_target: bool,
        metadata: Option<NodeMetadata>,
        detail: String,
    ) -> Result<()> {
        let metadata_path = metadata_path_for_sync_target(target, is_directory_target)?;
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
        for write in &self.pending_writes {
            match write {
                PendingWrite::EnsureDirectory { path } => {
                    fs::create_dir_all(path).with_context(|| {
                        format!(
                            "failed to create studio-synced directory {}",
                            path.display()
                        )
                    })?;
                    self.plan.stats.directories_written += 1;
                }
                PendingWrite::WriteFile {
                    path,
                    contents,
                    kind,
                } => {
                    if let Some(parent) = path.parent() {
                        fs::create_dir_all(parent).with_context(|| {
                            format!("failed to create parent directory {}", parent.display())
                        })?;
                    }

                    fs::write(path, contents)
                        .with_context(|| format!("failed to write {}", path.display()))?;

                    match *kind {
                        "metadata" => self.plan.stats.metadata_written += 1,
                        _ => self.plan.stats.files_written += 1,
                    }
                }
                PendingWrite::RemoveFile { path } => {
                    if path.exists() {
                        fs::remove_file(path).with_context(|| {
                            format!("failed to remove studio-synced file {}", path.display())
                        })?;
                        self.plan.stats.removals_applied += 1;
                    }
                }
                PendingWrite::RemoveDirectory { path } => {
                    if path.exists() {
                        fs::remove_dir_all(path).with_context(|| {
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
}

fn studio_file_target(
    parent_dir: &Path,
    parent_class: Option<&str>,
    node: &StudioNode,
) -> Result<Option<(PathBuf, String)>> {
    let Some((suffix, contents, always_embed_header)) = (match node.class_name.as_str() {
        "Script" => Some((
            ".server.lua",
            node.source.clone().unwrap_or_default(),
            false,
        )),
        "LocalScript" => Some((
            ".client.lua",
            node.source.clone().unwrap_or_default(),
            false,
        )),
        "ModuleScript" => Some((".lua", node.source.clone().unwrap_or_default(), false)),
        "RemoteFunction" => Some((".rf", String::new(), false)),
        "RemoteEvent" => Some((".re", String::new(), false)),
        "BindableFunction" => Some((".bf", String::new(), false)),
        "BindableEvent" => Some((".be", String::new(), false)),
        "Folder" => Some((".folder", String::new(), false)),
        "Part" => Some((".part", render_structured_instance_contents(node)?, true)),
        "Model" => Some((".model", render_structured_instance_contents(node)?, true)),
        "WorldModel" => Some((
            ".worldmodel",
            render_structured_instance_contents(node)?,
            true,
        )),
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

fn node_has_container_children(node: &StudioNode) -> bool {
    !node.children.is_empty() && container_source_file(node).is_some()
}

fn container_source_file(node: &StudioNode) -> Option<(&'static str, String)> {
    match node.class_name.as_str() {
        "Script" => Some(("init.server.lua", node.source.clone().unwrap_or_default())),
        "LocalScript" => Some(("init.client.lua", node.source.clone().unwrap_or_default())),
        "ModuleScript" => Some(("init.lua", node.source.clone().unwrap_or_default())),
        "RemoteFunction" | "RemoteEvent" | "BindableFunction" | "BindableEvent" => None,
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

    let mut children = serde_json::Map::new();
    for child in &node.children {
        children.insert(child.name.clone(), studio_node_to_inline_value(child)?);
    }

    Ok(serde_json::to_string_pretty(
        &json!({ "children": children }),
    )?)
}

fn render_compact_instance_document(node: &StudioNode) -> Result<String> {
    serde_json::to_string_pretty(&studio_node_to_inline_value(node)?).map_err(Into::into)
}

fn studio_node_to_inline_value(node: &StudioNode) -> Result<Value> {
    let mut object = serde_json::Map::new();
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
        let mut children = serde_json::Map::new();
        for child in &node.children {
            children.insert(child.name.clone(), studio_node_to_inline_value(child)?);
        }
        object.insert("children".to_string(), Value::Object(children));
    }

    Ok(Value::Object(object))
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
    SUPPORTED_ROOT_SERVICE_CLASSES.contains(&node.class_name.as_str())
        && node.name == node.class_name
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
        .is_some_and(|name| name == ".meta.json" || name.ends_with(".meta.json"))
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

fn metadata_path_for_sync_target(target: &Path, is_directory_target: bool) -> Result<PathBuf> {
    if is_directory_target {
        return Ok(target.join(".meta.json"));
    }

    metadata_path_for_target(target)
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
    {
        return Some(false);
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
        "file {} has a fixed local shape and cannot declare className {}; expected {}",
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
    let metadata_path = metadata_path_for_target(target)?;
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
    let metadata_path = metadata_path_for_target(target)?;
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
        CreateRequest, DashboardState, PluginCommandKind, PluginCommandResultRequest,
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
        assert_eq!(
            fs::read_to_string(dir.path().join("ReplicatedStorage/Shared.lua"))?,
            "return 1"
        );
        assert!(dir.path().join("ReplicatedStorage/Ping.re").is_file());
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
        assert_eq!(
            fs::read_to_string(dir.path().join("ServerScriptService/Main/EnemyHandler.lua"))?,
            "return { enemy = true }"
        );
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
    fn syncs_generic_instances_into_structured_instance_files() -> Result<()> {
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
        let part_source = fs::read_to_string(dir.path().join("Workspace/Spawn.part"))?;
        assert!(part_source.contains("--!nyjo"));
        assert!(part_source.contains("\"Anchored\": true"));
        assert!(part_source.contains("\"SpawnPoint\""));
        assert!(part_source.contains("\"Label\""));
        assert!(!dir.path().join("Workspace/Spawn/.meta.json").exists());

        Ok(())
    }

    #[test]
    fn syncs_generic_ui_trees_into_compact_instance_files() -> Result<()> {
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
                        name: "Root".to_string(),
                        class_name: "Frame".to_string(),
                        children: vec![StudioNode {
                            name: "Title".to_string(),
                            class_name: "TextLabel".to_string(),
                            children: vec![],
                            properties: std::collections::BTreeMap::from([(
                                "Text".to_string(),
                                json!("Nyjo"),
                            )]),
                            attributes: Default::default(),
                            tags: vec![],
                            source: None,
                        }],
                        properties: std::collections::BTreeMap::from([(
                            "Visible".to_string(),
                            json!(true),
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
        let document = fs::read_to_string(dir.path().join("StarterGui/Hud.instance.json"))?;
        assert!(document.contains("\"className\": \"ScreenGui\""));
        assert!(document.contains("\"ResetOnSpawn\": false"));
        assert!(document.contains("\"Root\""));
        assert!(document.contains("\"Title\""));
        assert!(!dir.path().join("StarterGui/Hud/.meta.json").exists());

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
    fn update_entry_can_clear_embedded_metadata_and_drop_header_for_script_files() -> Result<()> {
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
        assert_eq!(source, "return 3");

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
    fn dashboard_rejects_commands_without_bridge_heartbeat() {
        let mut dashboard = DashboardState::default();
        let error = dashboard
            .queue_command(PluginCommandKind::PushLocalTree)
            .expect_err("expected disconnected bridge to reject commands");
        assert!(error.to_string().contains("not connected"));
    }

    #[test]
    fn dashboard_command_lifecycle_moves_from_pending_to_result() -> Result<()> {
        let mut dashboard = DashboardState::default();
        dashboard.update_plugin_heartbeat(PluginHeartbeatRequest {
            bridge_version: Some("test-bridge".to_string()),
            place_name: Some("UnitTest".to_string()),
            status: Some("idle".to_string()),
        });

        let queued = dashboard.queue_command(PluginCommandKind::PreviewPull)?;
        assert_eq!(queued.id, 1);
        assert!(dashboard.pending_command.is_some());

        let dispatched = dashboard
            .take_pending_command()
            .expect("expected pending command to dispatch");
        assert_eq!(dispatched.id, queued.id);
        assert!(dashboard.pending_command.is_none());
        assert!(dashboard.active_command.is_some());

        let result = dashboard.finish_command(PluginCommandResultRequest {
            command_id: dispatched.id,
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
}
