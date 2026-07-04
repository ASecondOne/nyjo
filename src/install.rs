use std::env;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

const PLUGIN_FILE_NAME: &str = "Nyjo.lua";
const PLUGIN_TEMPLATE: &str = include_str!("../assets/Nyjo.lua");

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    pub packaged_plugin_path: PathBuf,
    pub installed_paths: Vec<PathBuf>,
    pub discovered_roots: Vec<PathBuf>,
    pub searched_users_dir: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct VinegarRootDiscovery {
    searched_users_dir: Option<PathBuf>,
    discovered_roots: Vec<PathBuf>,
}

pub fn install_plugin(output_dir: &Path, port: u16) -> Result<InstallReport> {
    let discovery = discover_default_vinegar_roots()?;
    install_plugin_with_discovery(output_dir, port, discovery)
}

fn install_plugin_with_discovery(
    output_dir: &Path,
    port: u16,
    discovery: VinegarRootDiscovery,
) -> Result<InstallReport> {
    let output_dir = normalize_output_dir(output_dir)?;
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create output directory {}", output_dir.display()))?;

    let plugin_source = render_plugin_source(port);
    let packaged_plugin_path = output_dir.join(PLUGIN_FILE_NAME);
    fs::write(&packaged_plugin_path, plugin_source).with_context(|| {
        format!(
            "failed to write packaged plugin {}",
            packaged_plugin_path.display()
        )
    })?;

    let mut installed_paths = Vec::with_capacity(discovery.discovered_roots.len());
    for root in &discovery.discovered_roots {
        let plugins_dir = root.join("Plugins");
        fs::create_dir_all(&plugins_dir).with_context(|| {
            format!(
                "failed to create plugin directory {}",
                plugins_dir.display()
            )
        })?;
        let target = plugins_dir.join(PLUGIN_FILE_NAME);
        fs::copy(&packaged_plugin_path, &target).with_context(|| {
            format!(
                "failed to install plugin into Vinegar plugin directory {}",
                target.display()
            )
        })?;
        installed_paths.push(target);
    }

    Ok(InstallReport {
        packaged_plugin_path,
        installed_paths,
        discovered_roots: discovery.discovered_roots,
        searched_users_dir: discovery.searched_users_dir,
    })
}

fn normalize_output_dir(output_dir: &Path) -> Result<PathBuf> {
    if output_dir.is_absolute() {
        return Ok(output_dir.to_path_buf());
    }

    Ok(env::current_dir()
        .context("failed to read current working directory")?
        .join(output_dir))
}

fn render_plugin_source(port: u16) -> String {
    PLUGIN_TEMPLATE.replace("{{DEFAULT_PORT}}", &port.to_string())
}

fn discover_default_vinegar_roots() -> Result<VinegarRootDiscovery> {
    discover_vinegar_roots_from_home(env::var_os("HOME").map(PathBuf::from))
}

fn discover_vinegar_roots_from_home(home: Option<PathBuf>) -> Result<VinegarRootDiscovery> {
    let Some(home) = home else {
        return Ok(VinegarRootDiscovery {
            searched_users_dir: None,
            discovered_roots: Vec::new(),
        });
    };

    let users_root =
        home.join(".var/app/org.vinegarhq.Vinegar/data/vinegar/prefixes/studio/drive_c/users");
    let discovered_roots = discover_vinegar_roots_from_users_dir(&users_root)?;

    Ok(VinegarRootDiscovery {
        searched_users_dir: Some(users_root),
        discovered_roots,
    })
}

fn discover_vinegar_roots_from_users_dir(users_root: &Path) -> Result<Vec<PathBuf>> {
    if !users_root.exists() {
        return Ok(Vec::new());
    }

    let mut roots = Vec::new();
    for entry in fs::read_dir(users_root).with_context(|| {
        format!(
            "failed to read Vinegar users directory {}",
            users_root.display()
        )
    })? {
        let entry = entry?;
        let roblox_root = entry.path().join("AppData/Local/Roblox");
        if roblox_root.is_dir() {
            roots.push(roblox_root);
        }
    }

    roots.sort();
    roots.dedup();
    Ok(roots)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;
    use tempfile::tempdir;

    use super::{
        PLUGIN_FILE_NAME, VinegarRootDiscovery, discover_vinegar_roots_from_home,
        discover_vinegar_roots_from_users_dir, install_plugin_with_discovery, render_plugin_source,
    };

    #[test]
    fn renders_plugin_with_requested_port() {
        let source = render_plugin_source(40123);
        assert!(source.contains("40123"));
        assert!(!source.contains("{{DEFAULT_PORT}}"));
    }

    #[test]
    fn discovers_vinegar_roblox_roots() -> Result<()> {
        let dir = tempdir()?;
        let users_root = dir.path().join("users");
        fs::create_dir_all(users_root.join("alice/AppData/Local/Roblox"))?;
        fs::create_dir_all(users_root.join("bob/AppData/Local/Roblox"))?;
        fs::create_dir_all(users_root.join("public/Documents"))?;

        let roots = discover_vinegar_roots_from_users_dir(&users_root)?;

        assert_eq!(roots.len(), 2);
        assert!(
            roots
                .iter()
                .all(|path| path.ends_with("AppData/Local/Roblox"))
        );
        Ok(())
    }

    #[test]
    fn packages_plugin_even_without_vinegar_roots() -> Result<()> {
        let dir = tempdir()?;
        let output_dir = dir.path().join("dist");
        let searched_users_dir = dir.path().join("users");

        let report = install_plugin_with_discovery(
            &output_dir,
            34872,
            VinegarRootDiscovery {
                searched_users_dir: Some(searched_users_dir.clone()),
                discovered_roots: Vec::new(),
            },
        )?;

        assert_eq!(
            report.packaged_plugin_path,
            output_dir.join(PLUGIN_FILE_NAME)
        );
        assert!(report.packaged_plugin_path.is_file());
        assert!(report.installed_paths.is_empty());
        assert!(report.discovered_roots.is_empty());
        assert_eq!(report.searched_users_dir, Some(searched_users_dir));

        Ok(())
    }

    #[test]
    fn missing_home_skips_auto_install_discovery() -> Result<()> {
        let discovery = discover_vinegar_roots_from_home(None)?;

        assert!(discovery.discovered_roots.is_empty());
        assert_eq!(discovery.searched_users_dir, None);

        Ok(())
    }

    #[test]
    fn plugin_file_name_is_stable() {
        assert_eq!(PLUGIN_FILE_NAME, "Nyjo.lua");
    }
}
