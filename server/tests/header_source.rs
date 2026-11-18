//! Header/source matching on a real directory tree.

use std::fs;
use std::path::{Path, PathBuf};

use cmantic_ls::config::Config;
use cmantic_ls::header_source::{HeaderSourceCache, find_matching_header_source};

struct TempDir(PathBuf);

impl TempDir {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("cmantic-ls-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        Self(dir.canonicalize().unwrap())
    }

    fn file(&self, relative: &str) -> PathBuf {
        let path = self.0.join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "").unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn find(path: &Path, roots: &[PathBuf]) -> Option<PathBuf> {
    find_matching_header_source(path, &Config::default(), roots)
}

#[test]
fn same_directory_wins() {
    let dir = TempDir::new("same-dir");
    let header = dir.file("lib/foo.hpp");
    let source = dir.file("lib/foo.cc");
    dir.file("src/foo.cpp");
    assert_eq!(
        find(&header, std::slice::from_ref(&dir.0)),
        Some(source.clone())
    );
    assert_eq!(find(&source, std::slice::from_ref(&dir.0)), Some(header));
}

#[test]
fn include_and_src_directories() {
    let dir = TempDir::new("include-src");
    let header = dir.file("include/foo.h");
    let source = dir.file("src/foo.cpp");
    dir.file("tests/fixtures/deep/foo.cpp");
    // Excluded directories are never searched (`build/` would otherwise tie with `src/`).
    dir.file("build/foo.cpp");
    dir.file(".cache/foo.cpp");
    dir.file("node_modules/foo.cpp");
    let roots = vec![dir.0.clone()];
    assert_eq!(find(&header, &roots), Some(source.clone()));
    assert_eq!(find(&source, &roots), Some(header));
}

#[test]
fn nested_include_directory_uses_workspace_roots() {
    let dir = TempDir::new("nested");
    let header = dir.file("include/project/widget.h");
    let source = dir.file("src/widget.cpp");
    let roots = vec![dir.0.clone()];
    assert_eq!(find(&header, &roots), Some(source));
}

#[test]
fn no_match_and_unknown_extensions() {
    let dir = TempDir::new("none");
    let header = dir.file("include/alone.h");
    let text = dir.file("notes.txt");
    let roots = vec![dir.0.clone()];
    assert_eq!(find(&header, &roots), None);
    assert_eq!(find(&text, &roots), None);
}

#[test]
fn cache_drops_deleted_files() {
    let dir = TempDir::new("cache");
    let header = dir.file("a/foo.h");
    let source = dir.file("a/foo.cpp");
    let roots = vec![dir.0.clone()];
    let config = Config::default();
    let mut cache = HeaderSourceCache::default();
    assert_eq!(cache.get(&header, &config, &roots), Some(source.clone()));
    assert_eq!(cache.get(&source, &config, &roots), Some(header.clone()));
    fs::remove_file(&source).unwrap();
    let moved = dir.file("src/foo.cpp");
    assert_eq!(cache.get(&header, &config, &roots), Some(moved));
}
