use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::{CommandFactory, Parser, Subcommand};
use nyjo::{init_project, install_plugin, scan_project, server, summarize_tree};

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
    println!(
        "Installed into {} Vinegar plugin folder(s):",
        report.installed_paths.len()
    );
    for path in report.installed_paths {
        println!("  + {}", path.display());
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

fn canonical_root(root: PathBuf) -> Result<PathBuf> {
    root.canonicalize()
        .with_context(|| format!("failed to resolve project root {}", root.display()))
}
