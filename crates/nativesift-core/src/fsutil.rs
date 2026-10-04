use std::ffi::OsStr;
use std::io;
use std::path::{Path, PathBuf};

use walkdir::WalkDir;

const IGNORED_NAMES: [&str; 4] = [".git", ".hg", ".svn", "node_modules"];

pub fn is_ignored_name(name: &OsStr) -> bool {
    name.to_str()
        .is_some_and(|name| IGNORED_NAMES.contains(&name))
}

pub fn is_ignored_path(path: &Path) -> bool {
    path.components()
        .any(|component| is_ignored_name(component.as_os_str()))
}

pub fn walk_files(root: &Path) -> Vec<PathBuf> {
    WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_entry(|entry| entry.depth() == 0 || !is_ignored_name(entry.file_name()))
        .filter_map(Result::ok)
        .filter(|entry| entry.file_type().is_file())
        .map(walkdir::DirEntry::into_path)
        .collect()
}

#[cfg(windows)]
pub fn normalize_root(path: &Path) -> io::Result<PathBuf> {
    std::path::absolute(path)
}

#[cfg(not(windows))]
pub fn normalize_root(path: &Path) -> io::Result<PathBuf> {
    std::fs::canonicalize(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignores_vcs_and_dependency_directories() {
        assert!(is_ignored_path(Path::new("/repo/.git/config")));
        assert!(is_ignored_path(Path::new(
            "/repo/web/node_modules/pkg/index.js"
        )));
        assert!(!is_ignored_path(Path::new("/repo/src/main.rs")));
        assert!(!is_ignored_path(Path::new(
            "/repo/.github/workflows/ci.yml"
        )));
    }

    #[test]
    fn walk_skips_ignored_directories() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join(".git")).unwrap();
        std::fs::create_dir_all(dir.path().join("src")).unwrap();
        std::fs::write(dir.path().join(".git").join("HEAD"), "ref").unwrap();
        std::fs::write(dir.path().join("src").join("lib.rs"), "fn main() {}").unwrap();

        let files = walk_files(dir.path());

        assert_eq!(files.len(), 1);
        assert!(files[0].ends_with("lib.rs"));
    }
}
