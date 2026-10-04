use std::ffi::OsStr;
use std::fs::File;
use std::io::{self, Read};
use std::path::Path;

use memmap2::Mmap;

const MAX_TEXT_BYTES: u64 = 16 * 1024 * 1024;
const MMAP_THRESHOLD_BYTES: u64 = 1024 * 1024;
const BINARY_SNIFF_BYTES: usize = 8192;

const TEXT_EXTENSIONS: &[&str] = &[
    "md", "markdown", "txt", "rst", "adoc", "org", "tex", "rs", "c", "h", "cc", "cpp", "cxx",
    "hpp", "hh", "hxx", "m", "mm", "cs", "java", "kt", "kts", "scala", "go", "py", "rb", "php",
    "pl", "lua", "swift", "dart", "js", "mjs", "cjs", "jsx", "ts", "tsx", "vue", "svelte", "html",
    "htm", "css", "scss", "sass", "less", "json", "jsonc", "toml", "yaml", "yml", "xml", "ini",
    "cfg", "conf", "env", "sql", "sh", "bash", "zsh", "fish", "ps1", "bat", "cmd", "cmake",
    "gradle", "csv", "tsv", "log", "proto", "graphql", "tf", "nix", "zig", "hs", "ex", "exs",
    "erl", "clj", "ml", "r", "jl",
];

const TEXT_FILE_NAMES: &[&str] = &[
    "makefile",
    "dockerfile",
    "readme",
    "license",
    "changelog",
    "cmakelists.txt",
    "justfile",
];

pub fn is_text_candidate(path: &Path) -> bool {
    if let Some(extension) = path.extension().and_then(OsStr::to_str) {
        return TEXT_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str());
    }
    path.file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| TEXT_FILE_NAMES.contains(&name.to_ascii_lowercase().as_str()))
}

pub fn bytes_to_text(bytes: &[u8]) -> Option<String> {
    let sniff = &bytes[..bytes.len().min(BINARY_SNIFF_BYTES)];
    if sniff.contains(&0) {
        return None;
    }
    Some(String::from_utf8_lossy(bytes).into_owned())
}

pub fn extract_text(path: &Path) -> io::Result<Option<String>> {
    if !is_text_candidate(path) {
        return Ok(None);
    }
    let mut file = File::open(path)?;
    let length = file.metadata()?.len();
    if length == 0 || length > MAX_TEXT_BYTES {
        return Ok(None);
    }
    if length >= MMAP_THRESHOLD_BYTES {
        let map = unsafe { Mmap::map(&file)? };
        return Ok(bytes_to_text(&map));
    }
    let mut buffer = Vec::with_capacity(length as usize);
    file.read_to_end(&mut buffer)?;
    Ok(bytes_to_text(&buffer))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_source_and_doc_files() {
        assert!(is_text_candidate(Path::new("src/main.rs")));
        assert!(is_text_candidate(Path::new("README.MD")));
        assert!(is_text_candidate(Path::new("Makefile")));
        assert!(!is_text_candidate(Path::new("photo.png")));
        assert!(!is_text_candidate(Path::new("archive.zip")));
    }

    #[test]
    fn rejects_binary_content() {
        assert!(bytes_to_text(b"plain text").is_some());
        assert!(bytes_to_text(b"abc\0def").is_none());
    }

    #[test]
    fn extracts_small_and_memory_mapped_files() {
        let dir = tempfile::tempdir().unwrap();
        let small = dir.path().join("small.txt");
        std::fs::write(&small, "small file").unwrap();
        assert_eq!(extract_text(&small).unwrap().as_deref(), Some("small file"));

        let large = dir.path().join("large.md");
        let body = "word ".repeat(300_000);
        std::fs::write(&large, &body).unwrap();
        assert_eq!(
            extract_text(&large).unwrap().as_deref(),
            Some(body.as_str())
        );
    }

    #[test]
    fn skips_empty_and_unknown_files() {
        let dir = tempfile::tempdir().unwrap();
        let empty = dir.path().join("empty.txt");
        std::fs::write(&empty, "").unwrap();
        assert!(extract_text(&empty).unwrap().is_none());

        let unknown = dir.path().join("data.bin");
        std::fs::write(&unknown, "text").unwrap();
        assert!(extract_text(&unknown).unwrap().is_none());
    }
}
