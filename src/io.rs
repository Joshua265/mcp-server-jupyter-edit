use anyhow::{anyhow, Context, Result};
use fs2::FileExt;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};

use crate::notebook::Notebook;

const BACKUP_DIR_NAME: &str = ".jupyter-edit-backups";

fn get_allowed_dir() -> Result<PathBuf> {
    std::env::current_dir()
        .context("Cannot determine current working directory")
}

pub fn validate_path(path: impl AsRef<Path>) -> Result<PathBuf> {
    let p = path.as_ref();

    if p.extension().and_then(|e| e.to_str()) != Some("ipynb") {
        anyhow::bail!("Path must have .ipynb extension");
    }

    let allowed = get_allowed_dir()?.canonicalize().context(
        "Cannot canonicalize current working directory for path validation",
    )?;

    let candidate = if p.is_absolute() {
        p.to_path_buf()
    } else {
        allowed.join(p)
    };

    // Prefer canonicalizing the full path when it exists.
    // If it doesn't exist yet (e.g. writing a new notebook), canonicalize the
    // parent directory and then re-attach the filename.
    let resolved = match candidate.canonicalize() {
        Ok(canonical) => canonical,
        Err(_) => {
            let parent = candidate
                .parent()
                .ok_or_else(|| anyhow!("Invalid path: {}", candidate.display()))?;
            let filename = candidate
                .file_name()
                .ok_or_else(|| anyhow!("Invalid filename: {}", candidate.display()))?;

            let canonical_parent = parent.canonicalize().with_context(|| {
                format!(
                    "Cannot resolve parent directory for path: {}",
                    parent.display()
                )
            })?;

            canonical_parent.join(filename)
        }
    };

    if !resolved.starts_with(&allowed) {
        anyhow::bail!(
            "Path outside allowed directory: {} is not under {}",
            resolved.display(),
            allowed.display()
        );
    }

    Ok(resolved)
}

pub fn acquire_lock(path: impl AsRef<Path>) -> Result<File> {
    let p = path.as_ref();
    let lock_path = PathBuf::from(format!("{}.lock", p.display()));
    let file = File::create(&lock_path)
        .with_context(|| format!("Cannot create lock file: {}", lock_path.display()))?;
    file.lock_exclusive()
        .with_context(|| format!("Cannot acquire exclusive lock: {}", lock_path.display()))?;
    Ok(file)
}

pub fn read_notebook_file(path: impl AsRef<Path>) -> Result<Notebook> {
    let resolved = validate_path(path)?;
    debug!("Reading notebook from: {}", resolved.display());

    let _lock = acquire_lock(&resolved)?;

    let content = fs::read_to_string(&resolved)
        .with_context(|| format!("Cannot read file: {}", resolved.display()))?;

    let notebook: Notebook = serde_json::from_str(&content)
        .with_context(|| format!("Invalid JSON format in notebook file: {}", resolved.display()))?;

    info!("Successfully read notebook from: {}", resolved.display());
    Ok(notebook)
}

pub fn write_notebook_file(path: impl AsRef<Path>, notebook: &Notebook) -> Result<()> {
    let resolved = validate_path(path)?;
    debug!("Writing notebook to: {}", resolved.display());

    let _lock = acquire_lock(&resolved)?;

    if let Some(parent) = resolved.parent() {
        if !parent.exists() {
            anyhow::bail!("Directory does not exist: {}", parent.display());
        }
    }

    if resolved.exists() {
        backup_notebook(&resolved)?;
    }

    let content = serde_json::to_string_pretty(notebook)
        .context("Failed to serialize notebook to JSON")?;

    let temp_path = resolved.with_extension("ipynb.tmp");
    fs::write(&temp_path, content)
        .with_context(|| format!("Cannot write temporary file: {}", temp_path.display()))?;

    fs::rename(&temp_path, &resolved)
        .with_context(|| format!("Cannot rename temp file to: {}", resolved.display()))?;

    info!("Successfully wrote notebook to: {}", resolved.display());
    Ok(())
}

pub fn backup_notebook(path: impl AsRef<Path>) -> Result<PathBuf> {
    let resolved = path.as_ref().to_path_buf();

    if !resolved.exists() {
        anyhow::bail!("Cannot backup, file does not exist: {}", resolved.display());
    }

    let allowed = get_allowed_dir()?;
    let backup_dir = allowed.join(BACKUP_DIR_NAME);

    if !backup_dir.exists() {
        fs::create_dir(&backup_dir)
            .with_context(|| format!("Cannot create backup directory: {}", backup_dir.display()))?;
    }

    let filename = resolved
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or_else(|| anyhow!("Invalid filename: {}", resolved.display()))?;

    let timestamp = chrono::Local::now().format("%Y%m%d_%H%M%S");
    let backup_filename = format!(
        "{}.{timestamp}.ipynb",
        filename
            .strip_suffix(".ipynb")
            .expect("validate_path enforces .ipynb extension"),
    );
    let backup_path = backup_dir.join(backup_filename);

    fs::copy(&resolved, &backup_path)
        .with_context(|| format!("Cannot create backup: {}", backup_path.display()))?;

    warn!("Created backup at: {}", backup_path.display());
    Ok(backup_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use tempfile::TempDir;

    fn create_test_notebook(path: impl AsRef<Path>) -> Result<()> {
        let notebook = Notebook::new();
        let content = serde_json::to_string_pretty(&notebook)?;
        fs::write(path, content)?;
        Ok(())
    }

    /// Guards against test parallelism causing set_current_dir to conflict
    static CWD_GUARD: Mutex<()> = Mutex::new(());

    #[test]
    fn test_validate_path_accepts_valid_ipynb() {
        let _guard = CWD_GUARD.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let ipynb_path = tmp.path().join("test.ipynb");
        File::create(&ipynb_path).unwrap();

        let result = validate_path("test.ipynb");
        assert!(result.is_ok(), "Should accept valid .ipynb path");
    }

    #[test]
    fn test_validate_path_rejects_wrong_extension() {
        let _guard = CWD_GUARD.lock().unwrap();
        let result = validate_path("notafile.txt");
        assert!(
            result.is_err(),
            "Should reject non-.ipynb extension"
        );
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains(".ipynb"),
            "Error should mention .ipynb requirement: {}",
            err
        );
    }

    #[test]
    fn test_validate_path_rejects_no_extension() {
        let _guard = CWD_GUARD.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let result = validate_path("noextension");
        assert!(result.is_err(), "Should reject path with no extension");
    }

    #[test]
    fn test_validate_path_rejects_directory_traversal() {
        let _guard = CWD_GUARD.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let result = validate_path("../../etc/passwd.ipynb");
        assert!(
            result.is_err(),
            "Should reject ../ directory traversal"
        );
    }

    #[test]
    fn test_validate_path_rejects_absolute_outside_cwd() {
        let _guard = CWD_GUARD.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let outside = PathBuf::from("/tmp/outside.ipynb");
        let result = validate_path(&outside);
        assert!(
            result.is_err(),
            "Should reject absolute path outside CWD"
        );
    }

    #[test]
    fn test_validate_path_accepts_path_in_subdir() {
        let _guard = CWD_GUARD.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let subdir = tmp.path().join("subdir");
        fs::create_dir(&subdir).unwrap();
        let ipynb_path = subdir.join("nested.ipynb");
        File::create(&ipynb_path).unwrap();

        let result = validate_path("subdir/nested.ipynb");
        assert!(
            result.is_ok(),
            "Should accept path within subdirectory"
        );
    }

    #[test]
    fn test_read_write_roundtrip() {
        use crate::notebook::{Cell, CellType};

        let _guard = CWD_GUARD.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let notebook_path = tmp.path().join("roundtrip.ipynb");
        create_test_notebook(&notebook_path).unwrap();

        let read = read_notebook_file(&notebook_path).unwrap();
        assert!(
            read.cells.is_empty(),
            "Default notebook should have no cells"
        );

        let mut new_notebook = Notebook::new();
        new_notebook
            .cells
            .push(Cell::new(CellType::Code, "print('hello')"));
        write_notebook_file(&notebook_path, &new_notebook).unwrap();

        let read2 = read_notebook_file(&notebook_path).unwrap();
        assert_eq!(read2.cells.len(), 1);
    }

    #[test]
    fn test_backup_goes_to_cwd_not_parent() {
        let _guard = CWD_GUARD.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let notebook_path = tmp.path().join("backup_test.ipynb");
        create_test_notebook(&notebook_path).unwrap();

        let backup_result = backup_notebook(&notebook_path).unwrap();

        // The backup should be within the allowed dir (tmp), not outside it
        assert!(
            backup_result.starts_with(tmp.path()),
            "Backup path {:?} should be within CWD {:?}",
            backup_result,
            tmp.path()
        );

        // The backup dir name should match convention
        let backup_dir = tmp.path().join(BACKUP_DIR_NAME);
        assert!(
            backup_dir.exists(),
            "Backup directory should be created in CWD"
        );
    }

    #[test]
    fn test_acquire_lock_creates_lock_file() {
        let _guard = CWD_GUARD.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let notebook_path = tmp.path().join("locktest.ipynb");
        File::create(&notebook_path).unwrap();

        let lock = acquire_lock(&notebook_path).unwrap();
        let lock_file_path = tmp.path().join("locktest.ipynb.lock");
        assert!(
            lock_file_path.exists(),
            "Lock file should be created"
        );

        drop(lock);
    }

    #[test]
    fn test_read_notebook_validates_path() {
        let _guard = CWD_GUARD.lock().unwrap();
        let result = read_notebook_file("../../etc/shadow.ipynb");
        assert!(
            result.is_err(),
            "Should reject path traversal on read"
        );
    }

    #[test]
    fn test_write_notebook_rejects_non_ipynb_extension() {
        let _guard = CWD_GUARD.lock().unwrap();
        let tmp = TempDir::new().unwrap();
        std::env::set_current_dir(tmp.path()).unwrap();

        let notebook = Notebook::new();
        let result = write_notebook_file("malicious.txt", &notebook);
        assert!(
            result.is_err(),
            "Should reject wrong extension on write"
        );
    }
}
