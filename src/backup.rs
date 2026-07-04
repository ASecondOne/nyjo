use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::Value;

const BACKUP_ROOT_DIR_NAME: &str = ".nyjo-backups";
const LOCAL_BACKUP_DIR_NAME: &str = "local";
const STUDIO_BACKUP_DIR_NAME: &str = "studio";
const BACKUP_MANIFEST_FILE_NAME: &str = "backup.json";
const LOCAL_SNAPSHOT_DIR_NAME: &str = "snapshot";
const STUDIO_TREE_FILE_NAME: &str = "tree.json";
const MAX_BACKUPS_PER_KIND: usize = 12;
const RESERVED_ROOT_NAMES: &[&str] = &[BACKUP_ROOT_DIR_NAME, ".git"];

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum BackupKind {
    LocalProject,
    StudioTree,
}

impl BackupKind {
    fn slug(self) -> &'static str {
        match self {
            Self::LocalProject => LOCAL_BACKUP_DIR_NAME,
            Self::StudioTree => STUDIO_BACKUP_DIR_NAME,
        }
    }

    fn id_prefix(self) -> &'static str {
        match self {
            Self::LocalProject => "local",
            Self::StudioTree => "studio",
        }
    }

    fn label_prefix(self) -> &'static str {
        match self {
            Self::LocalProject => "Local backup",
            Self::StudioTree => "Studio backup",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct BackupRecord {
    pub id: String,
    pub kind: BackupKind,
    pub label: String,
    pub created_at_ms: u64,
    pub reason: String,
    pub path: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place_id: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub restore_hint: Option<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct StudioBackupIdentity {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub place_id: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct StudioBackupSnapshot {
    pub backup: BackupRecord,
    pub tree: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct LocalRestoreReport {
    pub restored_backup: BackupRecord,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rescue_backup: Option<BackupRecord>,
}

pub fn create_local_project_backup(
    project_root: &Path,
    reason: impl Into<String>,
    detail: Option<Value>,
) -> Result<BackupRecord> {
    let reason = reason.into();
    let created_at_ms = now_ms();
    let (id, backup_dir, relative_path) =
        prepare_backup_directory(project_root, BackupKind::LocalProject, created_at_ms)?;
    let snapshot_dir = backup_dir.join(LOCAL_SNAPSHOT_DIR_NAME);
    fs::create_dir_all(&snapshot_dir).with_context(|| {
        format!(
            "failed to create backup snapshot {}",
            snapshot_dir.display()
        )
    })?;
    copy_root_entries(project_root, &snapshot_dir, true)?;

    let backup = BackupRecord {
        id: id.clone(),
        kind: BackupKind::LocalProject,
        label: build_label(BackupKind::LocalProject, &id, None),
        created_at_ms,
        reason,
        path: relative_path,
        session_id: None,
        place_name: None,
        place_id: None,
        detail,
        restore_hint: Some(build_local_restore_hint(project_root, &id)),
    };
    write_manifest(&backup_dir, &backup)?;
    prune_backups(project_root, BackupKind::LocalProject, MAX_BACKUPS_PER_KIND)?;
    Ok(backup)
}

pub fn list_local_project_backups(project_root: &Path) -> Result<Vec<BackupRecord>> {
    list_backups(project_root, BackupKind::LocalProject)
}

pub fn latest_local_project_backup(project_root: &Path) -> Result<Option<BackupRecord>> {
    Ok(list_local_project_backups(project_root)?.into_iter().next())
}

pub fn restore_local_project_backup(
    project_root: &Path,
    backup_id: Option<&str>,
) -> Result<LocalRestoreReport> {
    let backup = resolve_local_backup(project_root, backup_id)?;
    let rescue_backup = if project_has_restorable_entries(project_root)? {
        Some(create_local_project_backup(
            project_root,
            format!("before restoring {}", backup.id),
            Some(serde_json::json!({
                "restoringBackupId": backup.id,
            })),
        )?)
    } else {
        None
    };

    let snapshot_dir =
        backup_dir(project_root, backup.kind, &backup.id).join(LOCAL_SNAPSHOT_DIR_NAME);
    if !snapshot_dir.is_dir() {
        bail!(
            "local backup {} is missing snapshot directory {}",
            backup.id,
            snapshot_dir.display()
        );
    }

    clear_project_root_for_restore(project_root)?;
    copy_root_entries(&snapshot_dir, project_root, false)?;

    Ok(LocalRestoreReport {
        restored_backup: backup,
        rescue_backup,
    })
}

pub fn create_studio_tree_backup(
    project_root: &Path,
    identity: StudioBackupIdentity,
    reason: impl Into<String>,
    tree: &Value,
    detail: Option<Value>,
) -> Result<BackupRecord> {
    let reason = reason.into();
    let created_at_ms = now_ms();
    let (id, backup_dir, relative_path) =
        prepare_backup_directory(project_root, BackupKind::StudioTree, created_at_ms)?;
    let tree_path = backup_dir.join(STUDIO_TREE_FILE_NAME);
    fs::write(&tree_path, serde_json::to_string_pretty(tree)?)
        .with_context(|| format!("failed to write studio backup tree {}", tree_path.display()))?;

    let backup = BackupRecord {
        id: id.clone(),
        kind: BackupKind::StudioTree,
        label: build_label(BackupKind::StudioTree, &id, identity.place_name.as_deref()),
        created_at_ms,
        reason,
        path: relative_path,
        session_id: identity.session_id,
        place_name: identity.place_name,
        place_id: identity.place_id,
        detail,
        restore_hint: None,
    };
    write_manifest(&backup_dir, &backup)?;
    prune_backups(project_root, BackupKind::StudioTree, MAX_BACKUPS_PER_KIND)?;
    Ok(backup)
}

pub fn read_studio_tree_backup(
    project_root: &Path,
    backup_id: &str,
) -> Result<StudioBackupSnapshot> {
    let backup_dir = backup_dir(project_root, BackupKind::StudioTree, backup_id);
    let backup = read_manifest(&backup_dir)?;
    let tree_path = backup_dir.join(STUDIO_TREE_FILE_NAME);
    let tree =
        serde_json::from_str::<Value>(&fs::read_to_string(&tree_path).with_context(|| {
            format!("failed to read studio backup tree {}", tree_path.display())
        })?)
        .with_context(|| {
            format!(
                "failed to decode studio backup tree {}",
                tree_path.display()
            )
        })?;

    Ok(StudioBackupSnapshot { backup, tree })
}

fn resolve_local_backup(project_root: &Path, backup_id: Option<&str>) -> Result<BackupRecord> {
    let backups = list_local_project_backups(project_root)?;
    if backups.is_empty() {
        bail!(
            "no local backups are available under {}",
            backup_root(project_root).display()
        );
    }

    if let Some(backup_id) = backup_id.map(str::trim).filter(|value| !value.is_empty()) {
        if let Some(backup) = backups.into_iter().find(|backup| backup.id == backup_id) {
            return Ok(backup);
        }

        let available = list_local_project_backups(project_root)?
            .into_iter()
            .take(5)
            .map(|backup| backup.id)
            .collect::<Vec<_>>()
            .join(", ");
        bail!("local backup {backup_id} was not found; available backups: {available}");
    }

    backups
        .into_iter()
        .next()
        .context("no local backups are available")
}

fn list_backups(project_root: &Path, kind: BackupKind) -> Result<Vec<BackupRecord>> {
    let backups_dir = backups_kind_root(project_root, kind);
    if !backups_dir.is_dir() {
        return Ok(Vec::new());
    }

    let mut backups = Vec::new();
    for entry in fs::read_dir(&backups_dir)
        .with_context(|| format!("failed to read backup directory {}", backups_dir.display()))?
    {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let manifest_path = entry.path().join(BACKUP_MANIFEST_FILE_NAME);
        if !manifest_path.is_file() {
            continue;
        }

        let Ok(contents) = fs::read_to_string(&manifest_path) else {
            continue;
        };
        let Ok(backup) = serde_json::from_str::<BackupRecord>(&contents) else {
            continue;
        };
        if backup.kind == kind {
            backups.push(backup);
        }
    }

    backups.sort_by(|left, right| {
        right
            .created_at_ms
            .cmp(&left.created_at_ms)
            .then_with(|| right.id.cmp(&left.id))
    });
    Ok(backups)
}

fn prune_backups(project_root: &Path, kind: BackupKind, keep: usize) -> Result<()> {
    let backups = list_backups(project_root, kind)?;
    for backup in backups.into_iter().skip(keep) {
        let path = backup_dir(project_root, kind, &backup.id);
        if path.exists() {
            fs::remove_dir_all(&path)
                .with_context(|| format!("failed to remove old backup {}", path.display()))?;
        }
    }

    Ok(())
}

fn prepare_backup_directory(
    project_root: &Path,
    kind: BackupKind,
    created_at_ms: u64,
) -> Result<(String, PathBuf, String)> {
    let backups_dir = backups_kind_root(project_root, kind);
    fs::create_dir_all(&backups_dir)
        .with_context(|| format!("failed to create backup root {}", backups_dir.display()))?;

    let base_id = format!("{}-{created_at_ms}", kind.id_prefix());
    for attempt in 0..1024 {
        let id = if attempt == 0 {
            base_id.clone()
        } else {
            format!("{base_id}-{attempt}")
        };
        let backup_dir = backups_dir.join(&id);
        if backup_dir.exists() {
            continue;
        }

        fs::create_dir_all(&backup_dir).with_context(|| {
            format!("failed to create backup directory {}", backup_dir.display())
        })?;
        let relative_path = backup_dir
            .strip_prefix(project_root)
            .unwrap_or(&backup_dir)
            .display()
            .to_string();
        return Ok((id, backup_dir, relative_path));
    }

    bail!(
        "failed to allocate a unique {} backup directory under {}",
        kind.slug(),
        backups_dir.display()
    )
}

fn write_manifest(backup_dir: &Path, backup: &BackupRecord) -> Result<()> {
    let manifest_path = backup_dir.join(BACKUP_MANIFEST_FILE_NAME);
    fs::write(&manifest_path, serde_json::to_string_pretty(backup)?).with_context(|| {
        format!(
            "failed to write backup manifest {}",
            manifest_path.display()
        )
    })
}

fn read_manifest(backup_dir: &Path) -> Result<BackupRecord> {
    let manifest_path = backup_dir.join(BACKUP_MANIFEST_FILE_NAME);
    serde_json::from_str::<BackupRecord>(
        &fs::read_to_string(&manifest_path).with_context(|| {
            format!("failed to read backup manifest {}", manifest_path.display())
        })?,
    )
    .with_context(|| {
        format!(
            "failed to decode backup manifest {}",
            manifest_path.display()
        )
    })
}

fn copy_root_entries(
    source_root: &Path,
    destination_root: &Path,
    exclude_reserved: bool,
) -> Result<()> {
    fs::create_dir_all(destination_root).with_context(|| {
        format!(
            "failed to create destination directory {}",
            destination_root.display()
        )
    })?;

    for entry in fs::read_dir(source_root)
        .with_context(|| format!("failed to read source directory {}", source_root.display()))?
    {
        let entry = entry?;
        let file_name = entry.file_name();
        if exclude_reserved && should_skip_reserved_root_name(file_name.as_os_str()) {
            continue;
        }

        copy_path_recursive(&entry.path(), &destination_root.join(&file_name))?;
    }

    Ok(())
}

fn copy_path_recursive(source: &Path, destination: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(source)
        .with_context(|| format!("failed to read metadata for {}", source.display()))?;

    if metadata.is_file() {
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent).with_context(|| {
                format!("failed to create destination parent {}", parent.display())
            })?;
        }
        fs::copy(source, destination).with_context(|| {
            format!(
                "failed to copy file {} -> {}",
                source.display(),
                destination.display()
            )
        })?;
        fs::set_permissions(destination, metadata.permissions()).with_context(|| {
            format!(
                "failed to preserve permissions on {}",
                destination.display()
            )
        })?;
        return Ok(());
    }

    if metadata.is_dir() {
        fs::create_dir_all(destination).with_context(|| {
            format!(
                "failed to create copied directory {}",
                destination.display()
            )
        })?;
        fs::set_permissions(destination, metadata.permissions()).with_context(|| {
            format!(
                "failed to preserve permissions on {}",
                destination.display()
            )
        })?;
        for entry in fs::read_dir(source)
            .with_context(|| format!("failed to read directory {}", source.display()))?
        {
            let entry = entry?;
            let file_name = entry.file_name();
            copy_path_recursive(&entry.path(), &destination.join(&file_name))?;
        }
        return Ok(());
    }

    bail!(
        "unsupported filesystem entry in backup flow: {}",
        source.display()
    )
}

fn project_has_restorable_entries(project_root: &Path) -> Result<bool> {
    for entry in fs::read_dir(project_root)
        .with_context(|| format!("failed to read project root {}", project_root.display()))?
    {
        let entry = entry?;
        if should_skip_reserved_root_name(entry.file_name().as_os_str()) {
            continue;
        }
        return Ok(true);
    }

    Ok(false)
}

fn clear_project_root_for_restore(project_root: &Path) -> Result<()> {
    for entry in fs::read_dir(project_root)
        .with_context(|| format!("failed to read project root {}", project_root.display()))?
    {
        let entry = entry?;
        if should_skip_reserved_root_name(entry.file_name().as_os_str()) {
            continue;
        }

        remove_path_recursive(&entry.path())?;
    }

    Ok(())
}

fn remove_path_recursive(path: &Path) -> Result<()> {
    let metadata = fs::symlink_metadata(path)
        .with_context(|| format!("failed to read metadata for {}", path.display()))?;
    if metadata.is_dir() {
        fs::remove_dir_all(path)
            .with_context(|| format!("failed to remove directory {}", path.display()))?;
        return Ok(());
    }
    if metadata.is_file() {
        fs::remove_file(path)
            .with_context(|| format!("failed to remove file {}", path.display()))?;
        return Ok(());
    }

    bail!(
        "unsupported filesystem entry during restore: {}",
        path.display()
    )
}

fn should_skip_reserved_root_name(name: &OsStr) -> bool {
    let Some(name) = name.to_str() else {
        return false;
    };
    RESERVED_ROOT_NAMES.contains(&name)
}

fn build_label(kind: BackupKind, id: &str, place_name: Option<&str>) -> String {
    match place_name.map(str::trim).filter(|value| !value.is_empty()) {
        Some(place_name) => format!("{} {} ({place_name})", kind.label_prefix(), id),
        None => format!("{} {}", kind.label_prefix(), id),
    }
}

fn build_local_restore_hint(project_root: &Path, backup_id: &str) -> String {
    format!(
        "nyjo restore --root {} --backup-id {}",
        shell_quote(&project_root.display().to_string()),
        shell_quote(backup_id)
    )
}

fn shell_quote(value: impl AsRef<str>) -> String {
    format!(
        "\"{}\"",
        value.as_ref().replace('\\', "\\\\").replace('"', "\\\"")
    )
}

fn backup_root(project_root: &Path) -> PathBuf {
    project_root.join(BACKUP_ROOT_DIR_NAME)
}

fn backups_kind_root(project_root: &Path, kind: BackupKind) -> PathBuf {
    backup_root(project_root).join(kind.slug())
}

fn backup_dir(project_root: &Path, kind: BackupKind, backup_id: &str) -> PathBuf {
    backups_kind_root(project_root, kind).join(backup_id)
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use std::fs;

    use anyhow::Result;
    use tempfile::tempdir;

    use super::{
        StudioBackupIdentity, create_local_project_backup, create_studio_tree_backup,
        read_studio_tree_backup, restore_local_project_backup,
    };

    #[test]
    fn creates_and_restores_local_project_backup() -> Result<()> {
        let dir = tempdir()?;
        fs::create_dir_all(dir.path().join("ReplicatedStorage"))?;
        fs::write(
            dir.path().join("ReplicatedStorage/Shared.lua"),
            "return 'before'",
        )?;
        fs::write(dir.path().join(".nyjoignore"), "ignored.file\n")?;

        let backup = create_local_project_backup(dir.path(), "before apply pull", None)?;
        fs::write(
            dir.path().join("ReplicatedStorage/Shared.lua"),
            "return 'after'",
        )?;
        fs::write(dir.path().join("Temp.lua"), "print('remove me')")?;
        fs::remove_file(dir.path().join(".nyjoignore"))?;

        let report = restore_local_project_backup(dir.path(), Some(&backup.id))?;

        assert_eq!(report.restored_backup.id, backup.id);
        assert_eq!(
            fs::read_to_string(dir.path().join("ReplicatedStorage/Shared.lua"))?,
            "return 'before'"
        );
        assert_eq!(
            fs::read_to_string(dir.path().join(".nyjoignore"))?,
            "ignored.file\n"
        );
        assert!(!dir.path().join("Temp.lua").exists());
        assert!(report.rescue_backup.is_some());

        Ok(())
    }

    #[test]
    fn stores_and_reads_studio_tree_backups() -> Result<()> {
        let dir = tempdir()?;
        let tree = serde_json::json!({
            "name": "Studio",
            "className": "DataModel",
            "children": [
                {
                    "name": "Workspace",
                    "className": "Workspace",
                    "children": []
                }
            ]
        });

        let backup = create_studio_tree_backup(
            dir.path(),
            StudioBackupIdentity {
                session_id: Some("session-1".to_string()),
                place_name: Some("Lobby".to_string()),
                place_id: Some(123),
            },
            "before push",
            &tree,
            None,
        )?;
        let snapshot = read_studio_tree_backup(dir.path(), &backup.id)?;

        assert_eq!(snapshot.backup.id, backup.id);
        assert_eq!(snapshot.backup.place_id, Some(123));
        assert_eq!(snapshot.tree, tree);

        Ok(())
    }
}
