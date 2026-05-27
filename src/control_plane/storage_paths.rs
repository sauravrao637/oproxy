use std::path::{Component, Path, PathBuf};

fn storage_root(storage_path: &Path) -> Result<PathBuf, String> {
    std::fs::create_dir_all(storage_path)
        .map_err(|e| format!("failed to create storage directory: {e}"))?;
    storage_path
        .canonicalize()
        .map_err(|e| format!("failed to resolve storage directory: {e}"))
}

fn is_safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && path
            .components()
            .all(|component| matches!(component, Component::Normal(_) | Component::CurDir))
}

pub(super) fn resolve_storage_file_for_write(
    storage_path: &Path,
    requested: &str,
) -> Result<PathBuf, String> {
    let root = storage_root(storage_path)?;
    let requested = Path::new(requested);
    if requested.as_os_str().is_empty() {
        return Err("path is required".to_string());
    }

    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        if !is_safe_relative_path(requested) {
            return Err("path must stay inside storage directory".to_string());
        }
        root.join(requested)
    };

    let Some(file_name) = candidate.file_name() else {
        return Err("path must include a file name".to_string());
    };
    let parent = candidate
        .parent()
        .ok_or_else(|| "path must include a parent directory".to_string())?;
    std::fs::create_dir_all(parent)
        .map_err(|e| format!("failed to create parent directory: {e}"))?;
    let parent = parent
        .canonicalize()
        .map_err(|e| format!("failed to resolve parent directory: {e}"))?;
    if !parent.starts_with(&root) {
        return Err("path must stay inside storage directory".to_string());
    }

    let file = parent.join(file_name);
    if file.exists() {
        let resolved = file
            .canonicalize()
            .map_err(|e| format!("failed to resolve file: {e}"))?;
        if !resolved.starts_with(&root) || !resolved.is_file() {
            return Err("path must stay inside storage directory".to_string());
        }
    }

    Ok(file)
}

pub(super) fn resolve_storage_file_for_read(
    storage_path: &Path,
    requested: &str,
) -> Result<PathBuf, String> {
    let root = storage_root(storage_path)?;
    let requested = Path::new(requested);
    if requested.as_os_str().is_empty() {
        return Err("path is required".to_string());
    }
    let candidate = if requested.is_absolute() {
        requested.to_path_buf()
    } else {
        if !is_safe_relative_path(requested) {
            return Err("path must stay inside storage directory".to_string());
        }
        root.join(requested)
    };
    let file = candidate
        .canonicalize()
        .map_err(|e| format!("failed to resolve file: {e}"))?;
    if !file.starts_with(&root) {
        return Err("path must stay inside storage directory".to_string());
    }
    if !file.is_file() {
        return Err("path must reference a file".to_string());
    }
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn storage_file_write_rejects_path_traversal() {
        let dir =
            std::env::temp_dir().join(format!("oproxy_management_path_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();

        let result = resolve_storage_file_for_write(&dir, "../outside.json");

        assert!(result.is_err());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn storage_file_read_rejects_absolute_path_outside_storage() {
        let dir =
            std::env::temp_dir().join(format!("oproxy_management_path_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let outside = std::env::temp_dir().join(format!(
            "oproxy_management_outside_{}.json",
            uuid::Uuid::new_v4()
        ));
        std::fs::write(&outside, "{}").unwrap();

        let result = resolve_storage_file_for_read(&dir, outside.to_str().unwrap());

        assert!(result.is_err());
        let _ = std::fs::remove_file(outside);
        let _ = std::fs::remove_dir_all(dir);
    }
}
