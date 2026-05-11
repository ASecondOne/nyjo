use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, bail};

const DEFAULT_IGNORE_CONTENTS: &str = "target/\n";

const DEFAULT_DIRECTORIES: &[&str] = &[
    "Lighting",
    "ReplicatedFirst",
    "ReplicatedStorage",
    "ServerStorage",
    "ServerScriptService",
    "SoundService",
    "StarterGui",
    "StarterPlayer",
    "StarterPlayer/StarterPlayerScripts",
    "StarterPlayer/StarterCharacterScripts",
    "Teams",
    "TextChatService",
    "Workspace",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitReport {
    pub root: PathBuf,
    pub created_paths: Vec<PathBuf>,
    pub reused_paths: Vec<PathBuf>,
}

pub fn init_project(root: &Path, force: bool) -> Result<InitReport> {
    if root.exists() && !root.is_dir() {
        bail!(
            "project root {} exists and is not a directory",
            root.display()
        );
    }

    fs::create_dir_all(root)
        .with_context(|| format!("failed to create project root {}", root.display()))?;

    let root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve project root {}", root.display()))?;

    let mut created_paths = Vec::new();
    let mut reused_paths = Vec::new();

    for relative in DEFAULT_DIRECTORIES {
        let directory = root.join(relative);
        if directory.exists() {
            if directory.is_dir() {
                reused_paths.push(directory);
                continue;
            }

            bail!(
                "cannot initialize {} because {} exists and is not a directory",
                root.display(),
                directory.display()
            );
        }

        fs::create_dir_all(&directory)
            .with_context(|| format!("failed to create directory {}", directory.display()))?;
        created_paths.push(directory);
    }

    let ignore_path = root.join(".nyjoignore");
    if ignore_path.exists() {
        if force {
            fs::write(&ignore_path, DEFAULT_IGNORE_CONTENTS)
                .with_context(|| format!("failed to write {}", ignore_path.display()))?;
            created_paths.push(ignore_path);
        } else {
            reused_paths.push(ignore_path);
        }
    } else {
        fs::write(&ignore_path, DEFAULT_IGNORE_CONTENTS)
            .with_context(|| format!("failed to write {}", ignore_path.display()))?;
        created_paths.push(ignore_path);
    }

    Ok(InitReport {
        root,
        created_paths,
        reused_paths,
    })
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;
    use tempfile::tempdir;

    use super::{DEFAULT_IGNORE_CONTENTS, init_project};

    #[test]
    fn creates_default_project_shape() -> Result<()> {
        let dir = tempdir()?;
        let root = dir.path().join("game");

        let report = init_project(&root, false)?;

        assert!(report.root.ends_with("game"));
        assert!(root.join("ReplicatedStorage").is_dir());
        assert!(root.join("ReplicatedFirst").is_dir());
        assert!(root.join("Lighting").is_dir());
        assert!(root.join("ServerStorage").is_dir());
        assert!(root.join("ServerScriptService").is_dir());
        assert!(root.join("SoundService").is_dir());
        assert!(root.join("StarterGui").is_dir());
        assert!(root.join("StarterPlayer/StarterPlayerScripts").is_dir());
        assert!(root.join("StarterPlayer/StarterCharacterScripts").is_dir());
        assert!(root.join("Teams").is_dir());
        assert!(root.join("TextChatService").is_dir());
        assert!(root.join("Workspace").is_dir());
        assert_eq!(
            fs::read_to_string(root.join(".nyjoignore"))?,
            DEFAULT_IGNORE_CONTENTS
        );

        Ok(())
    }

    #[test]
    fn keeps_existing_ignore_file_without_force() -> Result<()> {
        let dir = tempdir()?;
        let root = dir.path().join("game");
        fs::create_dir_all(&root)?;
        fs::write(root.join(".nyjoignore"), "custom/\n")?;

        let report = init_project(&root, false)?;

        assert!(
            report
                .reused_paths
                .iter()
                .any(|path| path.ends_with(".nyjoignore"))
        );
        assert_eq!(fs::read_to_string(root.join(".nyjoignore"))?, "custom/\n");

        Ok(())
    }

    #[test]
    fn overwrites_ignore_file_with_force() -> Result<()> {
        let dir = tempdir()?;
        let root = dir.path().join("game");
        fs::create_dir_all(&root)?;
        fs::write(root.join(".nyjoignore"), "custom/\n")?;

        init_project(&root, true)?;

        assert_eq!(
            fs::read_to_string(root.join(".nyjoignore"))?,
            DEFAULT_IGNORE_CONTENTS
        );

        Ok(())
    }
}
