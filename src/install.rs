//! Packaging and installing the Nyjo Studio plugin.
//!
//! Supported Linux Studio launchers:
//!
//! * **TuxBlox** (default, preferred). Everything lives under `~/.tuxblox`
//!   (or `$TUXBLOX_ROOT`). Roblox's own data folder is
//!   `C:\users\<account>\AppData\Local\Roblox` inside the virtual drive at
//!   `$TUXBLOX_PREFIX` / `~/.tuxblox/runtime/pfx`. TuxBlox keeps that folder
//!   outside the drive (`~/.tuxblox/roblox`, or XDG dirs with the experimental
//!   Linux integration flag) and links it back in, so writing through the
//!   in-drive path always lands where Studio looks.
//!   See <https://tuxblox.net/docs/advanced/files-and-folders>.
//! * **Vinegar** (legacy Flatpak layout) as a fallback.
//! * An explicit `--plugins-dir` override for anything else.

use std::collections::BTreeSet;
use std::env;
use std::fmt;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

pub const PLUGIN_FILE_NAME: &str = "Nyjo.lua";
const PLUGIN_TEMPLATE: &str = include_str!("../assets/Nyjo.lua");
const PORT_PLACEHOLDER: &str = "{{DEFAULT_PORT}}";
/// Windows accounts every Wine prefix has that never hold a real profile.
const SKIPPED_PREFIX_USERS: &[&str] = &["Public", "Default", "All Users", "Default User"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum StudioLauncher {
    TuxBlox,
    Vinegar,
    Custom,
}

impl fmt::Display for StudioLauncher {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::TuxBlox => "TuxBlox",
            Self::Vinegar => "Vinegar",
            Self::Custom => "custom",
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DiscoveredRoot {
    pub launcher: StudioLauncher,
    /// The Roblox data folder (`.../AppData/Local/Roblox`) or, for
    /// [`StudioLauncher::Custom`], the plugins folder itself.
    pub roblox_dir: PathBuf,
}

impl DiscoveredRoot {
    pub fn plugins_dir(&self) -> PathBuf {
        match self.launcher {
            StudioLauncher::Custom => self.roblox_dir.clone(),
            _ => self.roblox_dir.join("Plugins"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallReport {
    pub packaged_plugin_path: PathBuf,
    pub installed_paths: Vec<(StudioLauncher, PathBuf)>,
    pub discovered_roots: Vec<DiscoveredRoot>,
    pub searched_dirs: Vec<PathBuf>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RootDiscovery {
    pub searched_dirs: Vec<PathBuf>,
    pub discovered_roots: Vec<DiscoveredRoot>,
}

#[derive(Debug, Clone, Default)]
pub struct InstallOptions {
    /// Install into this plugins folder instead of auto-discovering one.
    pub plugins_dir: Option<PathBuf>,
    /// Only package the plugin; never touch any Studio folder.
    pub no_auto_install: bool,
}

pub fn install_plugin(output_dir: &Path, port: u16) -> Result<InstallReport> {
    install_plugin_with_options(output_dir, port, &InstallOptions::default())
}

pub fn install_plugin_with_options(
    output_dir: &Path,
    port: u16,
    options: &InstallOptions,
) -> Result<InstallReport> {
    let discovery = if options.no_auto_install {
        RootDiscovery::default()
    } else if let Some(plugins_dir) = &options.plugins_dir {
        RootDiscovery {
            searched_dirs: vec![plugins_dir.clone()],
            discovered_roots: vec![DiscoveredRoot {
                launcher: StudioLauncher::Custom,
                roblox_dir: plugins_dir.clone(),
            }],
        }
    } else {
        discover_default_roots(&EnvSnapshot::from_process())?
    };
    install_plugin_with_discovery(output_dir, port, discovery)
}

fn install_plugin_with_discovery(
    output_dir: &Path,
    port: u16,
    discovery: RootDiscovery,
) -> Result<InstallReport> {
    if port == 0 {
        bail!("port must be between 1 and 65535");
    }

    let output_dir = normalize_output_dir(output_dir)?;
    fs::create_dir_all(&output_dir)
        .with_context(|| format!("failed to create output directory {}", output_dir.display()))?;

    let plugin_source = render_plugin_source(port)?;
    let packaged_plugin_path = output_dir.join(PLUGIN_FILE_NAME);
    write_atomically(&packaged_plugin_path, plugin_source.as_bytes()).with_context(|| {
        format!(
            "failed to write packaged plugin {}",
            packaged_plugin_path.display()
        )
    })?;

    let mut installed_paths = Vec::with_capacity(discovery.discovered_roots.len());
    let mut seen_targets = BTreeSet::new();
    for root in &discovery.discovered_roots {
        let plugins_dir = root.plugins_dir();
        fs::create_dir_all(&plugins_dir).with_context(|| {
            format!(
                "failed to create {} plugin directory {}",
                root.launcher,
                plugins_dir.display()
            )
        })?;
        // The TuxBlox in-drive path is a link to ~/.tuxblox/roblox; dedupe on
        // the real location so one folder is never written twice.
        let real_dir = plugins_dir
            .canonicalize()
            .unwrap_or_else(|_| plugins_dir.clone());
        if !seen_targets.insert(real_dir) {
            continue;
        }
        let target = plugins_dir.join(PLUGIN_FILE_NAME);
        write_atomically(&target, plugin_source.as_bytes()).with_context(|| {
            format!(
                "failed to install plugin into {} plugin directory {}",
                root.launcher,
                target.display()
            )
        })?;
        installed_paths.push((root.launcher, target));
    }

    Ok(InstallReport {
        packaged_plugin_path,
        installed_paths,
        discovered_roots: discovery.discovered_roots,
        searched_dirs: discovery.searched_dirs,
    })
}

/// Write via a temp file + rename so Studio never loads a half-written plugin.
fn write_atomically(target: &Path, contents: &[u8]) -> Result<()> {
    let parent = target
        .parent()
        .with_context(|| format!("{} has no parent directory", target.display()))?;
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("nyjo");
    let temp = parent.join(format!(".{file_name}.{}.tmp", std::process::id()));
    {
        let mut file = fs::File::create(&temp)
            .with_context(|| format!("failed to create {}", temp.display()))?;
        file.write_all(contents)?;
        file.sync_all()?;
    }
    fs::rename(&temp, target).inspect_err(|_| {
        let _ = fs::remove_file(&temp);
    })?;
    Ok(())
}

fn normalize_output_dir(output_dir: &Path) -> Result<PathBuf> {
    if output_dir.is_absolute() {
        return Ok(output_dir.to_path_buf());
    }

    Ok(env::current_dir()
        .context("failed to read current working directory")?
        .join(output_dir))
}

pub fn render_plugin_source(port: u16) -> Result<String> {
    if !PLUGIN_TEMPLATE.contains(PORT_PLACEHOLDER) {
        bail!("plugin template is missing the {PORT_PLACEHOLDER} placeholder");
    }
    Ok(PLUGIN_TEMPLATE.replace(PORT_PLACEHOLDER, &port.to_string()))
}

/// The bits of the environment discovery depends on, so tests can fake them.
#[derive(Debug, Clone, Default)]
struct EnvSnapshot {
    home: Option<PathBuf>,
    tuxblox_root: Option<PathBuf>,
    tuxblox_prefix: Option<PathBuf>,
}

impl EnvSnapshot {
    fn from_process() -> Self {
        Self {
            home: env::var_os("HOME").map(PathBuf::from),
            tuxblox_root: env::var_os("TUXBLOX_ROOT").map(PathBuf::from),
            tuxblox_prefix: env::var_os("TUXBLOX_PREFIX").map(PathBuf::from),
        }
    }

    /// TuxBlox ignores a relative `TUXBLOX_ROOT`, so do the same.
    fn tuxblox_root(&self) -> Option<PathBuf> {
        if let Some(root) = self.tuxblox_root.as_ref().filter(|p| p.is_absolute()) {
            return Some(root.clone());
        }
        self.home.as_ref().map(|home| home.join(".tuxblox"))
    }

    fn tuxblox_prefix(&self) -> Option<PathBuf> {
        if let Some(prefix) = self.tuxblox_prefix.as_ref().filter(|p| p.is_absolute()) {
            return Some(prefix.clone());
        }
        self.tuxblox_root().map(|root| root.join("runtime"))
    }
}

fn discover_default_roots(env: &EnvSnapshot) -> Result<RootDiscovery> {
    let mut discovery = RootDiscovery::default();

    // TuxBlox first: it is the launcher Nyjo targets by default.
    if let Some(prefix) = env.tuxblox_prefix() {
        // TUXBLOX_PREFIX may point at runtime/ or straight at the Wine prefix.
        for users_dir in [
            prefix.join("pfx/drive_c/users"),
            prefix.join("drive_c/users"),
        ] {
            if discovery.searched_dirs.contains(&users_dir) {
                continue;
            }
            discovery.searched_dirs.push(users_dir.clone());
            for roblox_dir in discover_roblox_dirs_in_users_dir(&users_dir)? {
                discovery.discovered_roots.push(DiscoveredRoot {
                    launcher: StudioLauncher::TuxBlox,
                    roblox_dir,
                });
            }
        }
    }

    if let Some(home) = &env.home {
        let users_dir =
            home.join(".var/app/org.vinegarhq.Vinegar/data/vinegar/prefixes/studio/drive_c/users");
        discovery.searched_dirs.push(users_dir.clone());
        for roblox_dir in discover_roblox_dirs_in_users_dir(&users_dir)? {
            discovery.discovered_roots.push(DiscoveredRoot {
                launcher: StudioLauncher::Vinegar,
                roblox_dir,
            });
        }
    }

    Ok(discovery)
}

fn discover_roblox_dirs_in_users_dir(users_root: &Path) -> Result<Vec<PathBuf>> {
    if !users_root.is_dir() {
        return Ok(Vec::new());
    }

    let mut roots = Vec::new();
    for entry in fs::read_dir(users_root).with_context(|| {
        format!(
            "failed to read prefix users directory {}",
            users_root.display()
        )
    })? {
        let entry = entry?;
        let name = entry.file_name();
        if SKIPPED_PREFIX_USERS
            .iter()
            .any(|skipped| name.to_str() == Some(skipped))
        {
            continue;
        }
        let roblox_root = entry.path().join("AppData/Local/Roblox");
        // is_dir() follows the link TuxBlox puts here.
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
    use std::path::PathBuf;

    use anyhow::Result;
    use tempfile::tempdir;

    use super::{
        DiscoveredRoot, EnvSnapshot, InstallOptions, PLUGIN_FILE_NAME, RootDiscovery,
        StudioLauncher, discover_default_roots, discover_roblox_dirs_in_users_dir,
        install_plugin_with_discovery, install_plugin_with_options, render_plugin_source,
    };

    #[test]
    fn renders_plugin_with_requested_port() -> Result<()> {
        let source = render_plugin_source(40123)?;
        assert!(source.contains("40123"));
        assert!(!source.contains("{{DEFAULT_PORT}}"));
        Ok(())
    }

    #[test]
    fn discovers_prefix_roblox_roots_and_skips_public() -> Result<()> {
        let dir = tempdir()?;
        let users_root = dir.path().join("users");
        fs::create_dir_all(users_root.join("alice/AppData/Local/Roblox"))?;
        fs::create_dir_all(users_root.join("bob/AppData/Local/Roblox"))?;
        fs::create_dir_all(users_root.join("Public/AppData/Local/Roblox"))?;
        fs::create_dir_all(users_root.join("carol/Documents"))?;

        let roots = discover_roblox_dirs_in_users_dir(&users_root)?;

        assert_eq!(roots.len(), 2);
        assert!(
            roots
                .iter()
                .all(|path| path.ends_with("AppData/Local/Roblox"))
        );
        assert!(
            !roots
                .iter()
                .any(|path| path.starts_with(users_root.join("Public")))
        );
        Ok(())
    }

    #[cfg(unix)]
    #[test]
    fn discovers_tuxblox_layout_through_roblox_symlink() -> Result<()> {
        // Mirrors a real TuxBlox 3.x install: the in-drive Roblox folder is a
        // symlink to ~/.tuxblox/roblox.
        let home = tempdir()?;
        let tux = home.path().join(".tuxblox");
        let data = tux.join("roblox");
        fs::create_dir_all(&data)?;
        let local = tux.join("runtime/pfx/drive_c/users/tim/AppData/Local");
        fs::create_dir_all(&local)?;
        std::os::unix::fs::symlink(&data, local.join("Roblox"))?;
        fs::create_dir_all(tux.join("runtime/pfx/drive_c/users/Public"))?;

        let env = EnvSnapshot {
            home: Some(home.path().to_path_buf()),
            ..EnvSnapshot::default()
        };
        let discovery = discover_default_roots(&env)?;

        assert_eq!(
            discovery.discovered_roots,
            vec![DiscoveredRoot {
                launcher: StudioLauncher::TuxBlox,
                roblox_dir: local.join("Roblox"),
            }]
        );

        let out = tempdir()?;
        let report = install_plugin_with_discovery(out.path(), 34872, discovery)?;
        assert_eq!(report.installed_paths.len(), 1);
        // Written through the link, so it lands in ~/.tuxblox/roblox/Plugins.
        assert!(data.join("Plugins").join(PLUGIN_FILE_NAME).is_file());
        Ok(())
    }

    #[test]
    fn honours_absolute_tuxblox_root_and_ignores_relative_one() {
        let env = EnvSnapshot {
            home: Some(PathBuf::from("/home/tim")),
            tuxblox_root: Some(PathBuf::from("/opt/tuxblox")),
            tuxblox_prefix: None,
        };
        assert_eq!(
            env.tuxblox_prefix(),
            Some(PathBuf::from("/opt/tuxblox/runtime"))
        );

        let relative = EnvSnapshot {
            home: Some(PathBuf::from("/home/tim")),
            tuxblox_root: Some(PathBuf::from("relative/tux")),
            tuxblox_prefix: None,
        };
        assert_eq!(
            relative.tuxblox_prefix(),
            Some(PathBuf::from("/home/tim/.tuxblox/runtime"))
        );
    }

    #[test]
    fn packages_plugin_even_without_studio_roots() -> Result<()> {
        let dir = tempdir()?;
        let output_dir = dir.path().join("dist");

        let report = install_plugin_with_discovery(&output_dir, 34872, RootDiscovery::default())?;

        assert_eq!(
            report.packaged_plugin_path,
            output_dir.join(PLUGIN_FILE_NAME)
        );
        assert!(report.packaged_plugin_path.is_file());
        assert!(report.installed_paths.is_empty());
        Ok(())
    }

    #[test]
    fn explicit_plugins_dir_is_used_verbatim() -> Result<()> {
        let dir = tempdir()?;
        let plugins = dir.path().join("MyPlugins");
        let report = install_plugin_with_options(
            &dir.path().join("dist"),
            34872,
            &InstallOptions {
                plugins_dir: Some(plugins.clone()),
                no_auto_install: false,
            },
        )?;
        assert_eq!(
            report.installed_paths,
            vec![(StudioLauncher::Custom, plugins.join(PLUGIN_FILE_NAME))]
        );
        Ok(())
    }

    #[test]
    fn rejects_port_zero() {
        let dir = tempdir().expect("tempdir");
        assert!(install_plugin_with_discovery(dir.path(), 0, RootDiscovery::default()).is_err());
    }

    #[test]
    fn missing_home_skips_auto_install_discovery() -> Result<()> {
        let discovery = discover_default_roots(&EnvSnapshot::default())?;
        assert!(discovery.discovered_roots.is_empty());
        Ok(())
    }
}
