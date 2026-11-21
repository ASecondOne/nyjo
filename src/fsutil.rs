//! Small filesystem helpers shared by the server, backups and installer.

use std::fs;
use std::io::Write;
use std::path::Path;

use anyhow::{Context, Result};

/// Write `contents` to `target` via a sibling temp file + rename, so readers
/// (Studio, the parser, an editor) never observe a truncated file and a crash
/// mid-write leaves the previous version intact.
pub fn write_atomic(target: &Path, contents: impl AsRef<[u8]>) -> Result<()> {
    let parent = target
        .parent()
        .with_context(|| format!("{} has no parent directory", target.display()))?;
    let file_name = target
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("nyjo");
    let temp = parent.join(format!(".{file_name}.nyjo-tmp-{}", std::process::id()));
    let result = (|| -> Result<()> {
        let mut file = fs::File::create(&temp)
            .with_context(|| format!("failed to create temp file {}", temp.display()))?;
        file.write_all(contents.as_ref())
            .with_context(|| format!("failed to write temp file {}", temp.display()))?;
        file.sync_all()
            .with_context(|| format!("failed to flush temp file {}", temp.display()))?;
        if let Ok(existing) = fs::metadata(target) {
            let _ = fs::set_permissions(&temp, existing.permissions());
        }
        fs::rename(&temp, target).with_context(|| {
            format!(
                "failed to move {} into place at {}",
                temp.display(),
                target.display()
            )
        })
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::write_atomic;

    #[test]
    fn writes_and_replaces_without_leaving_temp_files() -> anyhow::Result<()> {
        let dir = tempfile::tempdir()?;
        let target = dir.path().join("a.lua");
        write_atomic(&target, "one")?;
        write_atomic(&target, "two")?;
        assert_eq!(std::fs::read_to_string(&target)?, "two");
        assert_eq!(std::fs::read_dir(dir.path())?.count(), 1);
        Ok(())
    }
}
