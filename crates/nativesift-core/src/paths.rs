use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};
use rayon::prelude::*;

const MATCH_CHUNK: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathMatch {
    pub path: Arc<str>,
    pub score: u32,
}

#[derive(Default)]
pub struct PathIndex {
    paths: Vec<Arc<str>>,
    positions: HashMap<Arc<str>, usize>,
}

impl PathIndex {
    pub fn len(&self) -> usize {
        self.paths.len()
    }

    #[cfg(test)]
    pub fn contains(&self, path: &str) -> bool {
        self.positions.contains_key(path)
    }

    pub fn insert(&mut self, path: &str) -> bool {
        if self.positions.contains_key(path) {
            return false;
        }
        let shared: Arc<str> = Arc::from(path);
        self.positions.insert(Arc::clone(&shared), self.paths.len());
        self.paths.push(shared);
        true
    }

    pub fn remove(&mut self, path: &str) -> bool {
        let Some(index) = self.positions.remove(path) else {
            return false;
        };
        self.paths.swap_remove(index);
        if let Some(moved) = self.paths.get(index) {
            self.positions.insert(Arc::clone(moved), index);
        }
        true
    }

    pub fn remove_under(&mut self, prefixes: &HashSet<String>) -> Vec<String> {
        let doomed: Vec<Arc<str>> = self
            .paths
            .par_iter()
            .filter(|path| {
                Path::new(path.as_ref()).ancestors().any(|ancestor| {
                    ancestor
                        .to_str()
                        .is_some_and(|text| prefixes.contains(text))
                })
            })
            .cloned()
            .collect();
        for path in &doomed {
            self.remove(path);
        }
        doomed.into_iter().map(|path| path.to_string()).collect()
    }

    pub fn search(&self, query: &str, limit: usize) -> Vec<PathMatch> {
        if limit == 0 || query.trim().is_empty() {
            return Vec::new();
        }
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);

        let best = self
            .paths
            .par_chunks(MATCH_CHUNK)
            .map(|chunk| {
                let mut matcher = Matcher::new(Config::DEFAULT.match_paths());
                let mut buffer = Vec::new();
                let mut hits = Vec::new();
                for path in chunk {
                    let haystack = Utf32Str::new(path, &mut buffer);
                    if let Some(score) = pattern.score(haystack, &mut matcher) {
                        hits.push(PathMatch {
                            path: Arc::clone(path),
                            score,
                        });
                    }
                }
                keep_best(hits, limit)
            })
            .reduce(Vec::new, |mut left, right| {
                left.extend(right);
                keep_best(left, limit)
            });
        keep_best(best, limit)
    }
}

fn keep_best(mut hits: Vec<PathMatch>, limit: usize) -> Vec<PathMatch> {
    hits.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.path.len().cmp(&b.path.len()))
            .then_with(|| a.path.cmp(&b.path))
    });
    hits.truncate(limit);
    hits
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> PathIndex {
        let mut index = PathIndex::default();
        for path in [
            "/home/u/project/src/main.rs",
            "/home/u/project/src/lib.rs",
            "/home/u/project/README.md",
            "/home/u/photos/holiday/beach.jpg",
            "/home/u/notes/mainframe-notes.txt",
        ] {
            index.insert(path);
        }
        index
    }

    #[test]
    fn insert_is_idempotent() {
        let mut index = PathIndex::default();
        assert!(index.insert("/a/b"));
        assert!(!index.insert("/a/b"));
        assert_eq!(index.len(), 1);
    }

    #[test]
    fn ranks_better_matches_first() {
        let hits = sample().search("main", 5);
        assert!(hits.len() >= 2);
        assert_eq!(&*hits[0].path, "/home/u/project/src/main.rs");
    }

    #[test]
    fn multiple_terms_must_all_match() {
        let hits = sample().search("proj readme", 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(&*hits[0].path, "/home/u/project/README.md");
    }

    #[test]
    fn smart_case_is_respected() {
        let hits = sample().search("README", 5);
        assert_eq!(hits.len(), 1);
    }

    #[test]
    fn empty_query_and_zero_limit_return_nothing() {
        let index = sample();
        assert!(index.search("", 5).is_empty());
        assert!(index.search("   ", 5).is_empty());
        assert!(index.search("main", 0).is_empty());
    }

    #[test]
    fn limit_is_enforced() {
        assert_eq!(sample().search("e", 2).len(), 2);
    }

    #[test]
    fn remove_keeps_positions_consistent() {
        let mut index = sample();
        assert!(index.remove("/home/u/project/src/main.rs"));
        assert!(!index.remove("/home/u/project/src/main.rs"));
        assert_eq!(index.len(), 4);
        for path in [
            "/home/u/project/src/lib.rs",
            "/home/u/project/README.md",
            "/home/u/photos/holiday/beach.jpg",
            "/home/u/notes/mainframe-notes.txt",
        ] {
            assert!(index.contains(path), "missing {path}");
            assert!(index.remove(path));
        }
        assert_eq!(index.len(), 0);
    }

    #[test]
    fn remove_under_drops_whole_subtrees() {
        let mut index = sample();
        let prefixes: HashSet<String> = ["/home/u/project".to_string()].into_iter().collect();
        let mut removed = index.remove_under(&prefixes);
        removed.sort();
        assert_eq!(removed.len(), 3);
        assert_eq!(index.len(), 2);
        assert!(index.contains("/home/u/photos/holiday/beach.jpg"));
    }

    #[test]
    fn remove_under_does_not_match_sibling_prefixes() {
        let mut index = PathIndex::default();
        index.insert("/data/app/file.txt");
        index.insert("/data/app-backup/file.txt");
        let prefixes: HashSet<String> = ["/data/app".to_string()].into_iter().collect();
        index.remove_under(&prefixes);
        assert!(index.contains("/data/app-backup/file.txt"));
        assert!(!index.contains("/data/app/file.txt"));
    }
}
