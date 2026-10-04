use std::env;
use std::io::{self, BufRead, Write};
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::Instant;

use nativesift_core::{normalize_root, Engine, WatchService};

const RESULT_LIMIT: usize = 10;

const USAGE: &str = "usage: nativesift [ROOT]

Indexes ROOT (default: current directory), keeps the index live, and starts a
search prompt.

  <text>      fuzzy search file paths
  :c <text>   full-text search file contents
  :q          quit";

fn main() -> ExitCode {
    env_logger::init();
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("error: {message}");
            ExitCode::FAILURE
        }
    }
}

fn run() -> Result<(), String> {
    let root_arg = env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    if matches!(root_arg.to_str(), Some("-h" | "--help")) {
        println!("{USAGE}");
        return Ok(());
    }

    let root = normalize_root(&root_arg)
        .map_err(|error| format!("cannot resolve {}: {error}", root_arg.display()))?;
    let engine = Arc::new(Engine::new().map_err(|error| error.to_string())?);
    let _service =
        WatchService::start(Arc::clone(&engine), &root).map_err(|error| error.to_string())?;

    eprintln!("indexing {} ...", root.display());
    let stats = engine
        .index_tree(&root)
        .map_err(|error| error.to_string())?;
    eprintln!(
        "indexed {} files ({} with text) in {:.2?}",
        stats.files, stats.text_documents, stats.elapsed
    );

    prompt_loop(&engine)
}

fn prompt_loop(engine: &Engine) -> Result<(), String> {
    let stdin = io::stdin();
    let mut stdout = io::stdout();
    loop {
        write!(stdout, "> ").map_err(|error| error.to_string())?;
        stdout.flush().map_err(|error| error.to_string())?;

        let mut line = String::new();
        let read = stdin
            .lock()
            .read_line(&mut line)
            .map_err(|error| error.to_string())?;
        if read == 0 {
            return Ok(());
        }
        let line = line.trim();

        if line == ":q" {
            return Ok(());
        }
        let started = Instant::now();
        if let Some(query) = line.strip_prefix(":c ") {
            match engine.search_content(query, RESULT_LIMIT) {
                Ok(hits) => {
                    for hit in &hits {
                        println!("{:>8.3}  {}", hit.score, hit.path);
                    }
                    println!("{} content hits in {:.2?}", hits.len(), started.elapsed());
                }
                Err(error) => eprintln!("search failed: {error}"),
            }
        } else if !line.is_empty() {
            let hits = engine.search_paths(line, RESULT_LIMIT);
            for hit in &hits {
                println!("{:>8}  {}", hit.score, hit.path);
            }
            println!("{} path hits in {:.2?}", hits.len(), started.elapsed());
        }
    }
}
