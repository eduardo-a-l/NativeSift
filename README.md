# NativeSift

Instant, lightweight, native desktop search across file names and file contents.
Rust for indexing, scoring and (soon) the UI; C++ for operating system integration.

> **Status: early development.** The search engine core, the Linux watcher and a
> headless CLI work today. The floating search bar, global hotkey and PDF extraction
> are not built yet. See [Roadmap](#roadmap).

## How it works

```
 kernel journal / notifications
   NTFS USN journal | FSEvents | inotify
              |
   C++ platform layer (crates/nativesift-platform/cpp)
              |   cxx bridge: raw create / modify / delete / rename events
              v
   Rust pipeline (crates/nativesift-core)
     debounce + batch -> reconcile against the real file system -> parallel extract
              |                                   |
       path index (Vec + map)              Tantivy in-memory index
       parallel fuzzy scoring              full-text, prefix-as-you-type
              \_________________________________/
                              |
                   query as the user types
```

- **Change tracking.** C++ talks to the operating system directly and streams raw
  events to Rust through [`cxx`](https://cxx.rs). Rust never has to poll or re-walk a
  tree to learn that something changed.
- **Reconciliation.** Events are batched and de-duplicated, then each touched path is
  checked against the file system before the index is updated. Reordered, duplicated
  or dropped events (including kernel queue overflow, which triggers a rescan) cannot
  leave the index in a wrong state.
- **Fuzzy path search.** [`nucleo-matcher`](https://crates.io/crates/nucleo-matcher)
  scores every path in parallel across all cores with `rayon`.
- **Content search.** [Tantivy](https://github.com/quickwit-oss/tantivy) holds an
  in-memory full-text index. The last word of a query matches as a prefix, so results
  update while typing.
- **Large files.** Text files of 1 MiB or more are memory-mapped instead of read.
  Binary files are detected and indexed by name only.
- **Actions.** `nativesift-core` includes an action runner that opens a file with the
  system handler or runs a shell command.

## Platform support

| Platform | Mechanism | Status |
|----------|-----------|--------|
| Linux | `inotify` | Implemented and tested |
| macOS | FSEvents (CoreServices) | Implemented, verified only by CI |
| Windows | NTFS USN change journal | Implemented, type-checked, verified only by CI |

Platform notes:

- **Linux:** `inotify` needs one watch per directory, so the initial setup walks the
  directory tree (not the files). Very large trees may need a higher
  `fs.inotify.max_user_watches`. `fanotify` is a planned alternative.
- **Windows:** reading the USN journal requires opening the volume, which needs
  administrator rights. Only local drive-letter paths are supported.
- **macOS:** FSEvents reports resolved real paths, so roots are canonicalized.

## Build and run

Requirements: a recent stable Rust toolchain and a C++17 compiler (GCC or Clang on
Linux, Xcode command line tools on macOS, MSVC on Windows).

```sh
cargo build --release
cargo run --release -p nativesift-cli -- ~/projects
```

At the prompt, type to fuzzy-search paths, `:c <text>` to search file contents, and
`:q` to quit.

```sh
cargo test --workspace
```

## Repository layout

| Path | Purpose |
|------|---------|
| `crates/nativesift-platform` | C++ watchers and the `cxx` bridge |
| `crates/nativesift-core` | Indexing, search, pipeline, actions |
| `crates/nativesift-cli` | Headless prompt for trying the engine |

## Known limitations

- The index lives in memory and is rebuilt on every start.
- Files that shrink while being memory-mapped can terminate the process (`SIGBUS`).
- Directories named `.git`, `.hg`, `.svn` and `node_modules` are skipped.
- Paths that are not valid UTF-8 are indexed lossily.

## Roadmap

- [ ] Floating search window and global hotkey (GUI framework: egui)
- [ ] PDF text extraction
- [ ] Copy-path and terminal actions in the UI
- [ ] Persistent index and fast startup
- [ ] Initial enumeration straight from the NTFS MFT
- [ ] `fanotify` backend on Linux

## License

Licensed under either of

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
