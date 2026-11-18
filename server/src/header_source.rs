//! Finding the matching header/source file (port of `HeaderSourceCache.ts`).
//!
//! Searches the file's directory, then its parent directory's subtree, then the workspace
//! roots, and picks the candidate whose directory is closest to the file's directory.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::time::{Duration, Instant};

use crate::config::Config;

/// Directory names that are never searched.
fn is_excluded_dir(name: &str) -> bool {
    name.starts_with('.') || name == "node_modules" || name == "target" || name.starts_with("build")
}

const MAX_DEPTH: usize = 12;
const MAX_ENTRIES: usize = 50_000;

/// How long a failed search is remembered before searching again.
const NEGATIVE_TTL: Duration = Duration::from_secs(30);

/// Caches matches in both directions; entries are dropped when a file disappears.
/// Failed searches are remembered for a short time.
#[derive(Debug, Default)]
pub struct HeaderSourceCache {
    cache: HashMap<PathBuf, PathBuf>,
    not_found: HashMap<PathBuf, Instant>,
}

impl HeaderSourceCache {
    pub fn get(&mut self, path: &Path, config: &Config, roots: &[PathBuf]) -> Option<PathBuf> {
        if let Some(cached) = self.cache.get(path) {
            if cached.is_file() {
                return Some(cached.clone());
            }
            let cached = cached.clone();
            self.cache.remove(path);
            self.cache.remove(&cached);
        }
        if self
            .not_found
            .get(path)
            .is_some_and(|at| at.elapsed() < NEGATIVE_TTL)
        {
            return None;
        }
        let Some(found) = find_matching_header_source(path, config, roots) else {
            self.not_found.insert(path.to_path_buf(), Instant::now());
            return None;
        };
        self.not_found.remove(path);
        self.cache.insert(path.to_path_buf(), found.clone());
        self.cache.insert(found.clone(), path.to_path_buf());
        Some(found)
    }

    pub fn clear(&mut self) {
        self.cache.clear();
        self.not_found.clear();
    }
}

/// File names (in extension order) that would match `path`.
fn candidate_names(path: &Path, config: &Config) -> Vec<String> {
    let (Some(stem), Some(ext)) = (
        path.file_stem().and_then(|s| s.to_str()),
        path.extension().and_then(|s| s.to_str()),
    ) else {
        return Vec::new();
    };
    let other = if config.header_extensions.iter().any(|e| e == ext) {
        &config.source_extensions
    } else if config.source_extensions.iter().any(|e| e == ext) {
        &config.header_extensions
    } else {
        return Vec::new();
    };
    other.iter().map(|e| format!("{stem}.{e}")).collect()
}

pub fn find_matching_header_source(
    path: &Path,
    config: &Config,
    roots: &[PathBuf],
) -> Option<PathBuf> {
    let names = candidate_names(path, config);
    if names.is_empty() {
        return None;
    }
    let directory = path.parent()?;

    for name in &names {
        let candidate = directory.join(name);
        if candidate.is_file() {
            return Some(candidate);
        }
    }

    // Only walk the parent directory if it is inside a workspace root (or no roots are known),
    // so a header at the root of a project doesn't trigger a search of the whole disk.
    if let Some(parent) = directory.parent()
        && (roots.is_empty() || roots.iter().any(|r| parent.starts_with(r)))
    {
        let mut found = Vec::new();
        walk(parent, &names, 0, &mut 0, &mut found);
        if let Some(best) = best_match(directory, &found) {
            return Some(best);
        }
    }

    let mut found = Vec::new();
    for root in roots {
        walk(root, &names, 0, &mut 0, &mut found);
    }
    best_match(directory, &found)
}

fn walk(dir: &Path, names: &[String], depth: usize, entries: &mut usize, found: &mut Vec<PathBuf>) {
    if depth > MAX_DEPTH || *entries > MAX_ENTRIES {
        return;
    }
    let Ok(read_dir) = fs::read_dir(dir) else {
        return;
    };
    let mut subdirs = Vec::new();
    for entry in read_dir.flatten() {
        *entries += 1;
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        if file_type.is_dir() {
            if !is_excluded_dir(name) {
                subdirs.push(entry.path());
            }
        } else if names.iter().any(|n| n == name) {
            found.push(entry.path());
        }
    }
    subdirs.sort();
    for subdir in subdirs {
        walk(&subdir, names, depth + 1, entries, found);
    }
}

fn best_match(directory: &Path, candidates: &[PathBuf]) -> Option<PathBuf> {
    let mut best: Option<(usize, &PathBuf)> = None;
    for candidate in candidates {
        let Some(parent) = candidate.parent() else {
            continue;
        };
        let diff = compare_directory_paths(parent, directory);
        if best.is_none_or(|(d, _)| diff < d) {
            best = Some((diff, candidate));
        }
    }
    best.map(|(_, path)| path.clone())
}

/// Distance between two directories (`util.compareDirectoryPaths`): lower is closer.
pub fn compare_directory_paths(a: &Path, b: &Path) -> usize {
    let segments = |p: &Path| -> Vec<String> {
        p.components()
            .filter_map(|c| match c {
                Component::Normal(s) => Some(s.to_string_lossy().into_owned()),
                _ => None,
            })
            .collect()
    };
    let a = segments(a);
    let b = segments(b);
    let min = a.len().min(b.len());

    let leading = a.iter().zip(&b).take_while(|(x, y)| x == y).count();
    let mut trailing = 0;
    for i in 1..min.saturating_sub(leading) {
        if a[a.len() - i] != b[b.len() - i] {
            break;
        }
        trailing += 1;
    }
    let common = leading + trailing;
    let coefficient = (a.len() - common).max(b.len() - common);
    if a.len() != b.len() {
        coefficient + 1
    } else {
        coefficient
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_distance() {
        let d = |a: &str, b: &str| compare_directory_paths(Path::new(a), Path::new(b));
        assert_eq!(d("/p/src", "/p/src"), 0);
        assert!(d("/p/src", "/p/include") < d("/p/other/deep/src", "/p/include"));
        assert!(d("/p/src/foo", "/p/include/foo") < d("/p/src/bar", "/p/include/foo"));
    }
}
