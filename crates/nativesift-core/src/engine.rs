use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use nativesift_platform::ChangeEvent;
use parking_lot::RwLock;
use rayon::prelude::*;

use crate::content::{ContentHit, ContentIndex};
use crate::error::EngineError;
use crate::extract::extract_text;
use crate::fsutil::{is_ignored_path, walk_files};
use crate::paths::PathIndex;

const SCAN_CHUNK: usize = 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PathHit {
    pub path: String,
    pub score: u32,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct IndexStats {
    pub files: usize,
    pub text_documents: usize,
    pub elapsed: Duration,
}

enum Plan {
    Upsert { path: String, text: Option<String> },
    Remove(PathBuf),
}

pub struct Engine {
    paths: RwLock<PathIndex>,
    content: ContentIndex,
}

impl Engine {
    pub fn new() -> Result<Self, EngineError> {
        Ok(Self {
            paths: RwLock::new(PathIndex::default()),
            content: ContentIndex::new()?,
        })
    }

    pub fn path_count(&self) -> usize {
        self.paths.read().len()
    }

    pub fn index_tree(&self, root: &Path) -> Result<IndexStats, EngineError> {
        let started = Instant::now();
        let files = walk_files(root);
        let mut stats = IndexStats::default();
        for chunk in files.chunks(SCAN_CHUNK) {
            let plans: Vec<Plan> = chunk.par_iter().map(|path| plan_file(path)).collect();
            let (indexed, texts) = self.apply_plans(plans)?;
            stats.files += indexed;
            stats.text_documents += texts;
        }
        self.content.commit()?;
        stats.elapsed = started.elapsed();
        Ok(stats)
    }

    pub fn apply_events(&self, events: Vec<ChangeEvent>) -> Result<(), EngineError> {
        let mut removals = Vec::new();
        let mut touched: HashMap<PathBuf, bool> = HashMap::new();
        let mut rebuild_roots = Vec::new();

        for event in events {
            match event {
                ChangeEvent::Created(path) => {
                    *touched.entry(path).or_default() = true;
                }
                ChangeEvent::Modified(path) => {
                    touched.entry(path).or_default();
                }
                ChangeEvent::Deleted(path) => removals.push(path),
                ChangeEvent::Renamed { from, to } => {
                    removals.push(from);
                    *touched.entry(to).or_default() = true;
                }
                ChangeEvent::Overflow(root) => rebuild_roots.push(root),
            }
        }

        removals.retain(|path| !is_ignored_path(path));
        touched.retain(|path, _| !is_ignored_path(path));
        self.remove_paths(removals);

        let plans: Vec<Plan> = touched
            .into_par_iter()
            .flat_map(|(path, may_walk)| plan_path(&path, may_walk))
            .collect();
        self.apply_plans(plans)?;

        for root in rebuild_roots {
            self.remove_paths(vec![root.clone()]);
            self.index_tree(&root)?;
        }

        self.content.commit()?;
        Ok(())
    }

    pub fn search_paths(&self, query: &str, limit: usize) -> Vec<PathHit> {
        self.paths
            .read()
            .search(query, limit)
            .into_iter()
            .map(|hit| PathHit {
                path: hit.path.to_string(),
                score: hit.score,
            })
            .collect()
    }

    pub fn search_content(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<ContentHit>, EngineError> {
        Ok(self.content.search(query, limit)?)
    }

    fn apply_plans(&self, plans: Vec<Plan>) -> Result<(usize, usize), EngineError> {
        let mut removals = Vec::new();
        let mut files = 0;
        let mut texts = 0;

        for plan in plans {
            match plan {
                Plan::Remove(path) => removals.push(path),
                Plan::Upsert { path, text } => {
                    let is_new = self.paths.write().insert(&path);
                    files += 1;
                    match text {
                        Some(text) => {
                            texts += 1;
                            if is_new {
                                self.content.add(&path, &text)?;
                            } else {
                                self.content.replace(&path, &text)?;
                            }
                        }
                        None if !is_new => self.content.remove(&path),
                        None => {}
                    }
                }
            }
        }

        self.remove_paths(removals);
        Ok((files, texts))
    }

    fn remove_paths(&self, paths: Vec<PathBuf>) {
        if paths.is_empty() {
            return;
        }
        let mut removed = Vec::new();
        let mut prefixes = HashSet::new();
        {
            let mut index = self.paths.write();
            for path in paths {
                let text = path.to_string_lossy().into_owned();
                if index.remove(&text) {
                    removed.push(text);
                } else {
                    prefixes.insert(text);
                }
            }
            if !prefixes.is_empty() {
                removed.extend(index.remove_under(&prefixes));
            }
        }
        for path in &removed {
            self.content.remove(path);
        }
    }
}

fn plan_file(path: &Path) -> Plan {
    let text = extract_text(path).ok().flatten();
    Plan::Upsert {
        path: path.to_string_lossy().into_owned(),
        text,
    }
}

fn plan_path(path: &Path, may_walk_directory: bool) -> Vec<Plan> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => vec![plan_file(path)],
        Ok(metadata) if metadata.is_dir() => {
            if may_walk_directory {
                walk_files(path)
                    .par_iter()
                    .map(|file| plan_file(file))
                    .collect()
            } else {
                Vec::new()
            }
        }
        _ => vec![Plan::Remove(path.to_path_buf())],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fixture {
        dir: tempfile::TempDir,
        engine: Engine,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().unwrap();
            fs::create_dir_all(dir.path().join("src")).unwrap();
            fs::create_dir_all(dir.path().join(".git")).unwrap();
            fs::create_dir_all(dir.path().join("docs")).unwrap();
            fs::write(
                dir.path().join("src/main.rs"),
                "fn main() { println!(\"hello world\"); }",
            )
            .unwrap();
            fs::write(dir.path().join("src/engine.rs"), "pub struct Engine;").unwrap();
            fs::write(
                dir.path().join("docs/guide.md"),
                "# Guide\nfuzzy search explained",
            )
            .unwrap();
            fs::write(dir.path().join("docs/photo.png"), [0u8, 1, 2, 3]).unwrap();
            fs::write(dir.path().join(".git/HEAD"), "ref: refs/heads/main").unwrap();
            let engine = Engine::new().unwrap();
            engine.index_tree(dir.path()).unwrap();
            Self { dir, engine }
        }

        fn path(&self, relative: &str) -> PathBuf {
            relative
                .split('/')
                .fold(self.dir.path().to_path_buf(), |path, part| path.join(part))
        }

        fn hits_named(&self, query: &str, file_name: &str) -> usize {
            self.engine
                .search_paths(query, 50)
                .iter()
                .filter(|hit| {
                    Path::new(&hit.path)
                        .file_name()
                        .is_some_and(|name| name == file_name)
                })
                .count()
        }

        fn content_paths(&self, query: &str) -> Vec<String> {
            self.engine
                .search_content(query, 10)
                .unwrap()
                .into_iter()
                .map(|hit| hit.path)
                .collect()
        }
    }

    #[test]
    fn initial_scan_indexes_paths_and_text_but_skips_ignored_dirs() {
        let fixture = Fixture::new();
        assert_eq!(fixture.engine.path_count(), 4);
        assert_eq!(fixture.hits_named("HEAD", "HEAD"), 0);
        assert_eq!(fixture.hits_named("main.rs", "main.rs"), 1);
        assert_eq!(fixture.content_paths("hello").len(), 1);
        assert!(fixture
            .content_paths("fuzzy")
            .iter()
            .any(|path| path.ends_with("guide.md")));
    }

    #[test]
    fn binary_files_are_findable_by_name_only() {
        let fixture = Fixture::new();
        assert_eq!(fixture.hits_named("photo", "photo.png"), 1);
    }

    #[test]
    fn created_and_modified_files_are_picked_up() {
        let fixture = Fixture::new();
        let created = fixture.path("src/new_module.rs");
        fs::write(&created, "pub fn brandnew() {}").unwrap();
        fixture
            .engine
            .apply_events(vec![ChangeEvent::Created(created.clone())])
            .unwrap();
        assert_eq!(fixture.hits_named("new_module", "new_module.rs"), 1);
        assert_eq!(fixture.content_paths("brandnew").len(), 1);

        fs::write(&created, "pub fn rewritten() {}").unwrap();
        fixture
            .engine
            .apply_events(vec![ChangeEvent::Modified(created)])
            .unwrap();
        assert!(fixture.content_paths("brandnew").is_empty());
        assert_eq!(fixture.content_paths("rewritten").len(), 1);
    }

    #[test]
    fn deleted_files_disappear_from_both_indexes() {
        let fixture = Fixture::new();
        let target = fixture.path("src/main.rs");
        fs::remove_file(&target).unwrap();
        fixture
            .engine
            .apply_events(vec![ChangeEvent::Deleted(target)])
            .unwrap();
        assert_eq!(fixture.hits_named("main.rs", "main.rs"), 0);
        assert!(fixture.content_paths("hello").is_empty());
    }

    #[test]
    fn renamed_files_move_to_the_new_path() {
        let fixture = Fixture::new();
        let from = fixture.path("src/engine.rs");
        let to = fixture.path("src/core.rs");
        fs::rename(&from, &to).unwrap();
        fixture
            .engine
            .apply_events(vec![ChangeEvent::Renamed { from, to }])
            .unwrap();
        assert_eq!(fixture.hits_named("engine.rs", "engine.rs"), 0);
        assert_eq!(fixture.hits_named("core.rs", "core.rs"), 1);
        assert_eq!(fixture.content_paths("engine").len(), 1);
        assert!(fixture.content_paths("engine")[0].ends_with("core.rs"));
    }

    #[test]
    fn renamed_directories_carry_their_contents() {
        let fixture = Fixture::new();
        let from = fixture.path("docs");
        let to = fixture.path("manual");
        fs::rename(&from, &to).unwrap();
        fixture
            .engine
            .apply_events(vec![ChangeEvent::Renamed { from, to }])
            .unwrap();
        let hits = fixture.content_paths("fuzzy");
        assert_eq!(hits.len(), 1);
        assert!(hits[0].contains("manual"));
        assert!(fixture
            .engine
            .search_paths("guide.md", 50)
            .iter()
            .all(|hit| {
                !Path::new(&hit.path)
                    .components()
                    .any(|part| part.as_os_str() == "docs")
            }));
        assert_eq!(fixture.hits_named("manual guide", "guide.md"), 1);
    }

    #[test]
    fn deleted_directories_remove_everything_below() {
        let fixture = Fixture::new();
        let target = fixture.path("docs");
        fs::remove_dir_all(&target).unwrap();
        fixture
            .engine
            .apply_events(vec![ChangeEvent::Deleted(target)])
            .unwrap();
        assert_eq!(fixture.engine.path_count(), 2);
        assert!(fixture.content_paths("fuzzy").is_empty());
    }

    #[test]
    fn created_directories_are_walked() {
        let fixture = Fixture::new();
        let nested = fixture.path("vendor/lib");
        fs::create_dir_all(&nested).unwrap();
        fs::write(nested.join("util.rs"), "pub fn helper() {}").unwrap();
        fixture
            .engine
            .apply_events(vec![ChangeEvent::Created(fixture.path("vendor"))])
            .unwrap();
        assert_eq!(fixture.hits_named("util.rs", "util.rs"), 1);
        assert_eq!(fixture.content_paths("helper").len(), 1);
    }

    #[test]
    fn event_order_within_a_batch_does_not_corrupt_state() {
        let fixture = Fixture::new();
        let flicker = fixture.path("src/flicker.rs");
        fs::write(&flicker, "pub fn flicker() {}").unwrap();
        fs::remove_file(&flicker).unwrap();
        fixture
            .engine
            .apply_events(vec![
                ChangeEvent::Created(flicker.clone()),
                ChangeEvent::Modified(flicker.clone()),
                ChangeEvent::Deleted(flicker),
            ])
            .unwrap();
        assert_eq!(fixture.hits_named("flicker", "flicker.rs"), 0);
        assert!(fixture.content_paths("flicker").is_empty());
    }

    #[test]
    fn events_inside_ignored_directories_are_dropped() {
        let fixture = Fixture::new();
        let ignored = fixture.path(".git/objects_note.txt");
        fs::write(&ignored, "secret").unwrap();
        fixture
            .engine
            .apply_events(vec![ChangeEvent::Created(ignored)])
            .unwrap();
        assert_eq!(fixture.hits_named("objects_note", "objects_note.txt"), 0);
    }

    #[test]
    fn overflow_triggers_a_full_rescan() {
        let fixture = Fixture::new();
        fs::write(fixture.path("src/missed.rs"), "pub fn missed() {}").unwrap();
        fs::remove_file(fixture.path("src/main.rs")).unwrap();
        fixture
            .engine
            .apply_events(vec![ChangeEvent::Overflow(
                fixture.dir.path().to_path_buf(),
            )])
            .unwrap();
        assert_eq!(fixture.hits_named("missed.rs", "missed.rs"), 1);
        assert_eq!(fixture.hits_named("main.rs", "main.rs"), 0);
        assert_eq!(fixture.engine.path_count(), 4);
        assert_eq!(fixture.content_paths("missed").len(), 1);
        assert!(fixture.content_paths("hello").is_empty());
    }
}
