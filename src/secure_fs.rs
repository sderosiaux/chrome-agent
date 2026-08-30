//! Permission enforcement for files and directories owned by chrome-agent.

use std::path::Path;

/// Create a tool-owned directory tree at 0700 and repair existing ancestors through
/// `.chrome-agent`. `DirBuilderExt::mode` closes the first-creation umask window.
pub fn create_private_dir_all(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::{DirBuilderExt, PermissionsExt};

        let mut builder = std::fs::DirBuilder::new();
        builder.recursive(true).mode(0o700).create(path)?;

        let mut owned = Vec::new();
        let mut found_root = false;
        for ancestor in path.ancestors() {
            owned.push(ancestor);
            if ancestor
                .file_name()
                .is_some_and(|name| name == ".chrome-agent")
            {
                found_root = true;
                break;
            }
        }
        if !found_root {
            owned.truncate(1);
        }
        for dir in owned {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(())
    }

    #[cfg(not(unix))]
    std::fs::create_dir_all(path)
}

/// Enforce 0600 on a file this tool wrote. Permission failure is an operation failure, not a
/// best-effort warning: the file may contain browser control endpoints or page data.
pub fn restrict_file(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
    }

    #[cfg(not(unix))]
    {
        let _ = path;
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    #[test]
    fn repairs_the_whole_private_tree_and_file() {
        let root = std::env::temp_dir().join(format!(
            "chrome-agent-secure-fs-{}-{}",
            std::process::id(),
            std::thread::current().name().unwrap_or("test")
        ));
        let private = root.join(".chrome-agent");
        let leaf = private.join("browsers").join("name");
        std::fs::create_dir_all(&leaf).unwrap();
        for dir in [&private, &private.join("browsers"), &leaf] {
            std::fs::set_permissions(dir, std::fs::Permissions::from_mode(0o755)).unwrap();
        }

        create_private_dir_all(&leaf).unwrap();
        for dir in [&private, &private.join("browsers"), &leaf] {
            assert_eq!(
                std::fs::metadata(dir).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }

        let file = leaf.join("state.json");
        std::fs::write(&file, "secret").unwrap();
        std::fs::set_permissions(&file, std::fs::Permissions::from_mode(0o644)).unwrap();
        restrict_file(&file).unwrap();
        assert_eq!(
            std::fs::metadata(&file).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
