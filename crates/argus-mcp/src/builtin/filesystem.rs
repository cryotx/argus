use std::path::{Component, Path, PathBuf};

use argus_core::types::{ArgusError, ArgusResult};
use argus_security::guardrails::GuardrailEngine;
use argus_security::secret_scanner::SecretScanner;

#[derive(Clone, Debug)]
pub struct FilesystemServer {
    root: PathBuf,
    guardrails: GuardrailEngine,
    secret_scanner: SecretScanner,
}

impl FilesystemServer {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self {
            root: root.into(),
            guardrails: GuardrailEngine::new(),
            secret_scanner: SecretScanner::new(),
        }
    }

    pub fn with_security_overrides<P1, S1, P2, S2, P3, S3>(
        root: impl Into<PathBuf>,
        protected_paths: P1,
        forbidden_markers: P2,
        secret_keywords: P3,
    ) -> Self
    where
        P1: IntoIterator<Item = S1>,
        S1: Into<String>,
        P2: IntoIterator<Item = S2>,
        S2: Into<String>,
        P3: IntoIterator<Item = S3>,
        S3: Into<String>,
    {
        Self {
            root: root.into(),
            guardrails: GuardrailEngine::with_additions(protected_paths, forbidden_markers),
            secret_scanner: SecretScanner::with_additional_keywords(secret_keywords),
        }
    }

    fn resolve_for_read(&self, rel: &str) -> ArgusResult<PathBuf> {
        let root = absolute_root(&self.root)?;
        let normalized_rel = normalize_relative_path(rel)?;
        let path = root.join(&normalized_rel);

        if path.exists() {
            let canonical = path
                .canonicalize()
                .map_err(|e| ArgusError::Security(format!("failed to canonicalize path: {e}")))?;
            if !canonical.starts_with(&root) {
                return Err(ArgusError::Security(
                    "path resolves outside filesystem root".to_string(),
                ));
            }
        }

        Ok(path)
    }

    fn resolve_for_write(&self, rel: &str) -> ArgusResult<PathBuf> {
        let root = absolute_root(&self.root)?;
        let normalized_rel = normalize_relative_path(rel)?;
        let path = root.join(&normalized_rel);

        if let Some(parent) = path.parent() {
            validate_parent_chain(parent, &root)?;
        }

        Ok(path)
    }

    pub async fn read_text(&self, rel: &str) -> ArgusResult<String> {
        let p = self.resolve_for_read(rel)?;
        let content = tokio::fs::read_to_string(p).await?;
        Ok(content)
    }

    pub async fn write_text(&self, rel: &str, content: &str) -> ArgusResult<()> {
        self.guardrails
            .enforce_paths([rel])
            .map_err(|e| ArgusError::Security(format!("guardrail violation: {e}")))?;

        let findings = self.secret_scanner.scan_text(content);
        if !findings.is_empty() {
            return Err(ArgusError::Security(format!(
                "secret scanner blocked write: {} findings",
                findings.len()
            )));
        }

        let p = self.resolve_for_write(rel)?;
        if let Some(parent) = Path::new(&p).parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        tokio::fs::write(p, content).await?;
        Ok(())
    }
}

fn absolute_root(root: &Path) -> ArgusResult<PathBuf> {
    if root.is_absolute() {
        return Ok(root.to_path_buf());
    }

    let cwd = std::env::current_dir()
        .map_err(|e| ArgusError::Security(format!("cannot resolve current directory: {e}")))?;
    Ok(cwd.join(root))
}

fn normalize_relative_path(rel: &str) -> ArgusResult<PathBuf> {
    let path = Path::new(rel);
    let mut normalized = PathBuf::new();

    for component in path.components() {
        match component {
            Component::Prefix(_) | Component::RootDir => {
                return Err(ArgusError::Security(
                    "absolute paths are not allowed".to_string(),
                ));
            }
            Component::CurDir => {}
            Component::ParentDir => {
                if !normalized.pop() {
                    return Err(ArgusError::Security("path escapes root".to_string()));
                }
            }
            Component::Normal(segment) => normalized.push(segment),
        }
    }

    if normalized.as_os_str().is_empty() {
        return Err(ArgusError::Security(
            "empty path is not allowed".to_string(),
        ));
    }

    Ok(normalized)
}

fn validate_parent_chain(parent: &Path, root: &Path) -> ArgusResult<()> {
    let mut current = root.to_path_buf();
    for segment in parent
        .strip_prefix(root)
        .map_err(|_| ArgusError::Security("path escapes root".to_string()))?
        .components()
    {
        if let Component::Normal(piece) = segment {
            current.push(piece);
            if let Ok(metadata) = std::fs::symlink_metadata(&current) {
                if metadata.file_type().is_symlink() {
                    return Err(ArgusError::Security(
                        "symlink paths are not allowed for writes".to_string(),
                    ));
                }
            }
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_normalize_rejects_escape() {
        let err = normalize_relative_path("../../etc/passwd").unwrap_err();
        assert!(format!("{err}").contains("path escapes root"));
    }

    #[test]
    fn test_normalize_rejects_absolute() {
        let err = normalize_relative_path("/etc/passwd").unwrap_err();
        assert!(format!("{err}").contains("absolute paths"));
    }

    #[test]
    fn test_normalize_collapses_dot_segments() {
        let normalized = normalize_relative_path("a/./b/../c.txt").unwrap();
        assert_eq!(normalized, PathBuf::from("a/c.txt"));
    }
}
