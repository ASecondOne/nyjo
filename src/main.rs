use std::path::PathBuf;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand, ValueEnum};
use nyjo::{
    BridgeCommandEnvelope, BridgeCommandKind, BridgeCommandRunReport, BridgeSessionSnapshot,
    BridgeTargetSelector, NyjoControlClient, init_project, install_plugin,
    resolve_target_session_id, restore_local_project_backup, scan_project, server, summarize_tree,
};

#[derive(Debug, Parser)]
#[command(name = "nyjo")]
#[command(about = "A Roblox-aware local project scanner and sync server for Linux.")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Debug, Subcommand)]
enum Commands {
    Init {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        force: bool,
    },
    Install {
        #[arg(long, default_value = "dist")]
        output_dir: PathBuf,
        #[arg(long, default_value_t = 34872)]
        port: u16,
    },
    Tree {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        json: bool,
    },
    Serve {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long, default_value_t = 34872)]
        port: u16,
    },
    Doctor {
        #[arg(long, default_value = ".")]
        root: PathBuf,
    },
    Places {
        #[arg(long, default_value_t = 34872)]
        port: u16,
        #[arg(long)]
        json: bool,
    },
    Push {
        #[arg(long, default_value_t = 34872)]
        port: u16,
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        place_id: Option<u64>,
        #[arg(long, default_value_t = 30_000)]
        wait_ms: u64,
        #[arg(long)]
        json: bool,
    },
    Pull {
        #[arg(long, default_value_t = 34872)]
        port: u16,
        #[arg(long, value_enum, default_value_t = PullMode::Preview)]
        mode: PullMode,
        #[arg(long)]
        session_id: Option<String>,
        #[arg(long)]
        place_id: Option<u64>,
        #[arg(long, default_value_t = 30_000)]
        wait_ms: u64,
        #[arg(long)]
        json: bool,
    },
    Restore {
        #[arg(long, default_value = ".")]
        root: PathBuf,
        #[arg(long)]
        backup_id: Option<String>,
        #[arg(long)]
        json: bool,
    },
}

#[derive(Debug, Clone, Copy, ValueEnum)]
enum PullMode {
    Preview,
    Apply,
    Force,
}

impl PullMode {
    fn command_kind(self) -> BridgeCommandKind {
        match self {
            Self::Preview => BridgeCommandKind::PreviewPull,
            Self::Apply => BridgeCommandKind::ApplyPull,
            Self::Force => BridgeCommandKind::ForcePull,
        }
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli.command {
        Some(Commands::Init { root, force }) => run_init(root, force)?,
        Some(Commands::Install { output_dir, port }) => run_install(output_dir, port)?,
        Some(Commands::Tree { root, json }) => print_tree(root, json)?,
        Some(Commands::Serve { root, port }) => {
            let root = canonical_root(root)?;
            server::serve(root, port).await?;
        }
        Some(Commands::Doctor { root }) => run_doctor(root)?,
        Some(Commands::Places { port, json }) => run_places(port, json).await?,
        Some(Commands::Push {
            port,
            session_id,
            place_id,
            wait_ms,
            json,
        }) => {
            run_bridge_command(
                port,
                BridgeCommandKind::PushLocalTree,
                BridgeTargetSelector {
                    session_id,
                    place_id,
                },
                wait_ms,
                json,
            )
            .await?;
        }
        Some(Commands::Pull {
            port,
            mode,
            session_id,
            place_id,
            wait_ms,
            json,
        }) => {
            run_bridge_command(
                port,
                mode.command_kind(),
                BridgeTargetSelector {
                    session_id,
                    place_id,
                },
                wait_ms,
                json,
            )
            .await?;
        }
        Some(Commands::Restore {
            root,
            backup_id,
            json,
        }) => run_restore(root, backup_id, json)?,
        None => {
            Cli::command().print_help()?;
            println!();
        }
    }

    Ok(())
}

fn print_tree(root: PathBuf, json: bool) -> Result<()> {
    let root = canonical_root(root)?;
    let tree = scan_project(&root)?;

    if json {
        println!("{}", serde_json::to_string_pretty(&tree)?);
    } else {
        print!("{}", tree.pretty_print());
    }

    Ok(())
}

fn run_init(root: PathBuf, force: bool) -> Result<()> {
    let report = init_project(&root, force)?;

    println!("Initialized nyjo project at {}", report.root.display());
    println!("Created: {}", report.created_paths.len());
    println!("Reused: {}", report.reused_paths.len());

    for path in &report.created_paths {
        if let Ok(relative) = path.strip_prefix(&report.root) {
            let display = if relative.as_os_str().is_empty() {
                ".".to_string()
            } else {
                relative.display().to_string()
            };
            println!("  + {display}");
        }
    }

    println!(
        "Next: add files like Shared.lua, Boot.server.lua, Click.re, or compact subtree files like Hud.instance.json and StarterGui.service.json."
    );

    Ok(())
}

fn run_install(output_dir: PathBuf, port: u16) -> Result<()> {
    let report = install_plugin(&output_dir, port)?;

    println!(
        "Packaged Nyjo plugin at {}",
        report.packaged_plugin_path.display()
    );
    if report.installed_paths.is_empty() {
        if let Some(users_dir) = report.searched_users_dir {
            println!(
                "Auto-install skipped: no Vinegar Studio root found under {}",
                users_dir.display()
            );
        } else {
            println!(
                "Auto-install skipped: HOME is not set, so Vinegar roots could not be discovered"
            );
        }
    } else {
        println!(
            "Installed into {} Vinegar plugin folder(s):",
            report.installed_paths.len()
        );
        for path in report.installed_paths {
            println!("  + {}", path.display());
        }
    }

    Ok(())
}

fn run_doctor(root: PathBuf) -> Result<()> {
    let root = canonical_root(root)?;
    let tree = scan_project(&root)?;
    let summary = summarize_tree(&tree);
    let ignore_path = root.join(".nyjoignore");

    println!("Project root: {}", root.display());
    println!(
        "Ignore file: {}",
        if ignore_path.exists() {
            "found"
        } else {
            "missing"
        }
    );
    println!("Total nodes: {}", summary.total_nodes);
    println!("Class counts:");
    for (class_name, count) in summary.class_counts {
        println!("  {class_name}: {count}");
    }

    Ok(())
}

fn run_restore(root: PathBuf, backup_id: Option<String>, json: bool) -> Result<()> {
    let root = canonical_root(root)?;
    let report = restore_local_project_backup(&root, backup_id.as_deref())?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
        return Ok(());
    }

    println!(
        "Restored {} from {}",
        root.display(),
        report.restored_backup.label
    );
    println!("Backup ID: {}", report.restored_backup.id);
    if let Some(reason) = (!report.restored_backup.reason.is_empty())
        .then_some(report.restored_backup.reason.as_str())
    {
        println!("Backup reason: {reason}");
    }
    if let Some(rescue_backup) = report.rescue_backup.as_ref() {
        println!("Rescue backup: {}", rescue_backup.id);
        if let Some(restore_hint) = rescue_backup.restore_hint.as_deref() {
            println!("Undo restore with: {restore_hint}");
        }
    }

    Ok(())
}

fn canonical_root(root: PathBuf) -> Result<PathBuf> {
    root.canonicalize()
        .with_context(|| format!("failed to resolve project root {}", root.display()))
}

async fn run_places(port: u16, json: bool) -> Result<()> {
    let client = NyjoControlClient::new(port)?;
    let places = client.list_places().await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&places)?);
        return Ok(());
    }

    println!("Studio bridge connected: {}", places.connected);
    println!("Connected sessions: {}", places.connected_sessions);
    if let Some(target_session_id) = places.target_session_id.as_deref() {
        println!("Current target: {target_session_id}");
    } else if places.selection_required {
        println!("Current target: selection required");
    } else {
        println!("Current target: none");
    }

    if places.sessions.is_empty() {
        println!("No Studio bridge sessions are currently visible.");
        return Ok(());
    }

    println!("Places:");
    for session in places.sessions {
        println!("  {}", format_session_summary(&session));
    }

    Ok(())
}

async fn run_bridge_command(
    port: u16,
    kind: BridgeCommandKind,
    selector: BridgeTargetSelector,
    wait_ms: u64,
    json: bool,
) -> Result<()> {
    let client = NyjoControlClient::new(port)?;
    let places = client.list_places().await?;
    let target_session_id = resolve_target_session_id(&places, &selector)?;

    if wait_ms == 0 {
        let queued = client.queue_command(kind, target_session_id).await?;

        if json {
            println!("{}", serde_json::to_string_pretty(&queued)?);
        } else {
            println!(
                "Queued command #{}: {} -> {}",
                queued.command.id,
                kind.label(),
                format_command_target(&queued.command)
            );
        }

        return Ok(());
    }

    let report = client
        .run_command(kind, target_session_id, Duration::from_millis(wait_ms))
        .await?;

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        print_command_report(&report);
    }

    Ok(())
}

fn print_command_report(report: &BridgeCommandRunReport) {
    println!(
        "Command #{} finished in {} ms",
        report.command.id, report.waited_ms
    );
    println!(
        "Action: {} -> {}",
        report.command.kind.label(),
        format_command_target(&report.command)
    );
    println!("Status: {}", if report.result.ok { "ok" } else { "failed" });
    println!("Summary: {}", report.result.summary);

    if let Some(detail) = report.result.detail.as_deref() {
        println!();
        println!("{detail}");
    }

    if let Some(data) = report.result.data.as_ref() {
        print_result_flags(data);
    }
}

fn print_result_flags(data: &serde_json::Value) {
    let mut printed = false;

    if let Some(mode) = data.get("mode").and_then(serde_json::Value::as_str) {
        println!("Mode: {mode}");
        printed = true;
    }
    if let Some(applied) = data.get("applied").and_then(serde_json::Value::as_bool) {
        println!("Applied: {applied}");
        printed = true;
    }
    if let Some(blocked) = data.get("blocked").and_then(serde_json::Value::as_bool) {
        println!("Blocked: {blocked}");
        printed = true;
    }
    if let Some(backup) = data.get("backup") {
        if let Some(id) = backup.get("id").and_then(serde_json::Value::as_str) {
            println!("Backup: {id}");
            printed = true;
        }
        if let Some(restore_hint) = backup
            .get("restoreHint")
            .and_then(serde_json::Value::as_str)
        {
            println!("Restore: {restore_hint}");
            printed = true;
        }
    }

    if !printed {
        return;
    }

    if let Some(stats) = data.get("plan").and_then(|plan| plan.get("stats")) {
        if let Some(conflicts) = stats.get("conflicts").and_then(serde_json::Value::as_u64) {
            println!("Conflicts: {conflicts}");
        }
        if let Some(files_written) = stats
            .get("filesWritten")
            .and_then(serde_json::Value::as_u64)
        {
            println!("Files written: {files_written}");
        }
        if let Some(directories_written) = stats
            .get("directoriesWritten")
            .and_then(serde_json::Value::as_u64)
        {
            println!("Directories written: {directories_written}");
        }
    }
}

fn format_session_summary(session: &BridgeSessionSnapshot) -> String {
    let mut status = Vec::new();
    status.push(if session.connected {
        "connected".to_string()
    } else {
        "offline".to_string()
    });
    if session.selected {
        status.push("selected".to_string());
    }
    if let Some(place_id) = session.place_id {
        status.push(format!("place {place_id}"));
    }
    if let Some(extra) = session.status.as_deref() {
        status.push(extra.to_string());
    }

    format!(
        "{} | session {} | {}",
        session.label(),
        session.session_id,
        status.join(", ")
    )
}

fn format_command_target(command: &BridgeCommandEnvelope) -> String {
    match (&command.target_place_name, command.target_place_id) {
        (Some(place_name), Some(place_id)) => {
            format!(
                "{place_name} (place {place_id}, session {})",
                command.target_session_id
            )
        }
        (Some(place_name), None) => format!("{place_name} (session {})", command.target_session_id),
        (None, _) => format!("session {}", command.target_session_id),
    }
}
