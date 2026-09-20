use std::path::Path;

pub fn sanitize_file_paths(paths: &[String]) -> Result<Vec<String>, String> {
    let mut out = Vec::with_capacity(paths.len());
    for p in paths {
        let sanitized = validate_file_path(p)?;
        out.push(sanitized);
    }
    Ok(out)
}

fn validate_file_path(path: &str) -> Result<String, String> {
    if path.is_empty() {
        return Err("empty file path".into());
    }
    if path.contains('\0') {
        return Err("file path contains null byte".into());
    }
    if path.len() > 4096 {
        let preview: String = path.chars().take(40).collect();
        return Err(format!("file path exceeds 4096 chars: {preview}..."));
    }

    // Repo paths are protocol data, not host-native paths. Accept both separators
    // at the boundary and emit one stable representation on every operating system.
    let portable = path.replace('\\', "/");
    let bytes = portable.as_bytes();
    if portable.starts_with('/')
        || (bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':')
    {
        return Err(format!("absolute paths not allowed: {path}"));
    }
    let p = Path::new(&portable);

    if p.is_absolute() {
        return Err(format!("absolute paths not allowed: {path}"));
    }

    for component in p.components() {
        match component {
            std::path::Component::ParentDir => {
                return Err(format!("path traversal (`..`) not allowed: {path}"));
            }
            std::path::Component::Normal(seg) => {
                let s = seg.to_string_lossy();
                if s.starts_with('.') && s != "." {
                    // Allow dotfiles like .env, .gitignore — but not .. (caught above)
                }
            }
            std::path::Component::RootDir | std::path::Component::Prefix(_) => {
                return Err(format!("absolute paths not allowed: {path}"));
            }
            std::path::Component::CurDir => {}
        }
    }

    let normalized = p
        .components()
        .filter(|c| !matches!(c, std::path::Component::CurDir))
        .collect::<std::path::PathBuf>();
    if normalized.as_os_str().is_empty() {
        return Err(format!("file path resolves to empty: {path}"));
    }

    Ok(normalized.to_string_lossy().replace('\\', "/"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allows_relative_paths() {
        assert!(validate_file_path("src/main.rs").is_ok());
        assert!(validate_file_path("a/b/c/d.txt").is_ok());
        assert!(validate_file_path(".gitignore").is_ok());
        assert!(validate_file_path("src/.env").is_ok());
    }

    #[test]
    fn rejects_traversal() {
        assert!(validate_file_path("../etc/passwd").is_err());
        assert!(validate_file_path("src/../../secret").is_err());
        assert!(validate_file_path("a/b/../../../etc/shadow").is_err());
        assert!(validate_file_path(r"..\etc\passwd").is_err());
        assert!(validate_file_path(r"src\..\secret").is_err());
        assert!(validate_file_path(r"a/b\..\../secret").is_err());
    }

    #[test]
    fn rejects_absolute() {
        assert!(validate_file_path("/etc/passwd").is_err());
        assert!(validate_file_path("/home/user/.ssh/id_rsa").is_err());
        assert!(validate_file_path(r"C:\Windows\system32").is_err());
        assert!(validate_file_path(r"C:relative\secret").is_err());
        assert!(validate_file_path(r"\\server\share\secret").is_err());
    }

    #[test]
    fn rejects_null_bytes() {
        assert!(validate_file_path("src/main\0.rs").is_err());
    }

    #[test]
    fn normalizes_dot_segments() {
        let result = validate_file_path("./src/./main.rs").unwrap();
        assert_eq!(result, "src/main.rs");
        assert_eq!(validate_file_path(r".\src\main.rs").unwrap(), "src/main.rs");
        assert_eq!(
            validate_file_path("src//nested///file.rs").unwrap(),
            "src/nested/file.rs"
        );
    }

    #[test]
    fn rejects_empty() {
        assert!(validate_file_path("").is_err());
        assert!(validate_file_path("././").is_err());
    }

    #[test]
    fn oversized_unicode_path_returns_an_error_without_panicking() {
        assert!(validate_file_path(&"é".repeat(4097)).is_err());
    }
}
