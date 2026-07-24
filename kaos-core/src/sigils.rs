//! The sigil catalog: personal Rebis programs plus embedded `std/`.
//!
//! A sigil is a named `.rebis` file under `~/.kaos/sigils`. Names take
//! `/`-separated folders, the same shape as module paths, so a sigil saved as
//! `team/reviews` is importable as `(# team/reviews)`.
//!
//! Personal entries are writable files; embedded standard modules are
//! browsable and loadable through the same typed catalog but remain read-only.
//! This layer has no opinion about display, so the terminal explorer and visual
//! editor browse one library rather than two.

use std::fs;
use std::path::{Path, PathBuf};

/// A saved program, without its contents.
#[derive(Clone, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub struct Entry {
    /// The qualified name, e.g. `team/reviews`. This is also its module path.
    pub name: String,
    /// Bytes on disk, for a listing that wants to show size.
    pub bytes: u64,
    /// Embedded `std/` entries can be opened but never overwritten or deleted.
    pub read_only: bool,
}

/// Why a sigil could not be read or written.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SigilError {
    /// The name is empty, absolute, or tries to climb out of the library.
    BadName(String),
    /// `std/` is the embedded standard library and is never written to.
    Reserved,
    Io(String),
}

impl std::fmt::Display for SigilError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadName(n) => write!(f, "'{n}' is not a usable sigil name"),
            Self::Reserved => {
                write!(
                    f,
                    "std/ is the embedded standard library — pick another name"
                )
            }
            Self::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for SigilError {}

/// The library on disk.
#[derive(Clone, Debug)]
pub struct Library {
    root: PathBuf,
}

impl Library {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// `~/.kaos/sigils`, beside the session store.
    pub fn default_library() -> Self {
        let base = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        Self::new(base.join(".kaos").join("sigils"))
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Resolve a name to its file, refusing anything that would escape the
    /// library. Names are data — a sigil called `../../.ssh/id_rsa` must not
    /// be able to reach outside.
    pub fn path(&self, name: &str) -> Result<PathBuf, SigilError> {
        let name = name.trim().trim_matches('/');
        if name.is_empty() {
            return Err(SigilError::BadName(name.to_string()));
        }
        let mut out = self.root.clone();
        for part in name.split('/') {
            if part.is_empty() || part == "." || part == ".." || part.contains('\\') {
                return Err(SigilError::BadName(name.to_string()));
            }
            out.push(part);
        }
        out.set_extension("rebis");
        Ok(out)
    }

    /// Every saved sigil, in stable qualified-name order.
    pub fn list(&self) -> Vec<Entry> {
        let mut out = Vec::new();
        walk(&self.root, &self.root, &mut out);
        out.sort();
        out
    }

    /// Sigils whose qualified name contains `query`, case-insensitively. An
    /// empty query lists everything, which is what an explorer opening for the
    /// first time wants.
    pub fn search(&self, query: &str) -> Vec<Entry> {
        let q = query.trim().to_ascii_lowercase();
        self.list()
            .into_iter()
            .filter(|e| q.is_empty() || e.name.to_ascii_lowercase().contains(&q))
            .collect()
    }

    /// Saved sigils followed by every embedded `std/` module.
    ///
    /// The standard library is part of the browsable catalog even though it
    /// never exists under the user's writable sigil directory.
    pub fn catalog(&self) -> Vec<Entry> {
        let mut entries = self.list();
        entries.extend(
            rebis_lang::std_modules()
                .iter()
                .map(|(name, source)| Entry {
                    name: (*name).to_string(),
                    bytes: source.len() as u64,
                    read_only: true,
                }),
        );
        entries
    }

    /// Search both personal sigils and the embedded standard library.
    pub fn search_catalog(&self, query: &str) -> Vec<Entry> {
        let query = query.trim().to_ascii_lowercase();
        self.catalog()
            .into_iter()
            .filter(|entry| query.is_empty() || entry.name.to_ascii_lowercase().contains(&query))
            .collect()
    }

    pub fn load(&self, name: &str) -> Result<String, SigilError> {
        let path = self.path(name)?;
        fs::read_to_string(path).map_err(|e| SigilError::Io(e.to_string()))
    }

    /// Load either a personal sigil or an exact embedded `std/` module.
    pub fn load_catalog(&self, name: &str) -> Result<String, SigilError> {
        let trimmed = name.trim().trim_matches('/');
        if is_reserved(trimmed) {
            return rebis_lang::std_modules()
                .iter()
                .find(|(module, _)| *module == trimmed)
                .map(|(_, source)| (*source).to_string())
                .ok_or_else(|| SigilError::Io(format!("embedded module not found: {trimmed}")));
        }
        self.load(trimmed)
    }

    /// Save, creating folders as needed. Refuses the reserved `std/` namespace:
    /// the language never consults the library for it, so a file there would be
    /// dead weight that looks load-bearing.
    pub fn save(&self, name: &str, source: &str) -> Result<PathBuf, SigilError> {
        let trimmed = name.trim().trim_matches('/');
        if is_reserved(trimmed) {
            return Err(SigilError::Reserved);
        }
        let path = self.path(trimmed)?;
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| SigilError::Io(e.to_string()))?;
        }
        fs::write(&path, source).map_err(|e| SigilError::Io(e.to_string()))?;
        Ok(path)
    }

    pub fn delete(&self, name: &str) -> Result<(), SigilError> {
        let trimmed = name.trim().trim_matches('/');
        if is_reserved(trimmed) {
            return Err(SigilError::Reserved);
        }
        let path = self.path(trimmed)?;
        fs::remove_file(path).map_err(|e| SigilError::Io(e.to_string()))
    }

    pub fn exists(&self, name: &str) -> bool {
        self.path(name).is_ok_and(|p| p.is_file())
    }
}

fn is_reserved(name: &str) -> bool {
    name == "std" || name.starts_with("std/")
}

fn walk(root: &Path, dir: &Path, out: &mut Vec<Entry>) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(root, &path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rebis") {
            let Ok(rel) = path.strip_prefix(root) else {
                continue;
            };
            let name = rel
                .with_extension("")
                .components()
                .map(|c| c.as_os_str().to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("/");
            let bytes = entry.metadata().map(|m| m.len()).unwrap_or(0);
            out.push(Entry {
                name,
                bytes,
                read_only: false,
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> Library {
        let dir =
            std::env::temp_dir().join(format!("kaos-sigils-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Library::new(dir)
    }

    #[test]
    fn save_load_round_trips() {
        let lib = temp("crud");
        lib.save("repair", "(-> \"a\" \"b\")").unwrap();
        assert_eq!(lib.load("repair").unwrap(), "(-> \"a\" \"b\")");
        assert!(lib.exists("repair"));
        lib.delete("repair").unwrap();
        assert!(!lib.exists("repair"));
        let _ = fs::remove_dir_all(lib.root());
    }

    #[test]
    fn folders_become_qualified_names() {
        let lib = temp("folders");
        lib.save("team/reviews", "\"x\"").unwrap();
        lib.save("solo", "\"y\"").unwrap();
        let names: Vec<String> = lib.list().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["solo".to_string(), "team/reviews".to_string()]);
        // The qualified name is also the module path.
        assert_eq!(lib.load("team/reviews").unwrap(), "\"x\"");
        let _ = fs::remove_dir_all(lib.root());
    }

    #[test]
    fn names_cannot_escape_the_library() {
        let lib = temp("escape");
        for bad in ["../outside", "a/../../b", "..", "", "   ", "/"] {
            assert!(
                matches!(lib.path(bad), Err(SigilError::BadName(_))),
                "{bad:?} should be refused"
            );
        }
        // A legitimate nested name still resolves inside the root.
        let ok = lib.path("team/reviews").unwrap();
        assert!(ok.starts_with(lib.root()));
    }

    #[test]
    fn the_std_namespace_is_read_only() {
        let lib = temp("std");
        assert_eq!(lib.save("std", "x"), Err(SigilError::Reserved));
        assert_eq!(lib.save("std/loops", "x"), Err(SigilError::Reserved));
        assert_eq!(lib.delete("std/loops"), Err(SigilError::Reserved));
        // A name that merely starts with the letters is fine.
        assert!(lib.save("standard", "x").is_ok());
        let _ = fs::remove_dir_all(lib.root());
    }

    #[test]
    fn catalog_includes_and_loads_the_embedded_standard_library() {
        let lib = temp("catalog-std");
        lib.save("personal", "\"mine\"").unwrap();

        let catalog = lib.catalog();
        assert!(catalog
            .iter()
            .any(|entry| entry.name == "personal" && !entry.read_only));
        assert!(catalog
            .iter()
            .any(|entry| entry.name == "std/flow" && entry.read_only));
        assert_eq!(
            catalog.iter().filter(|entry| entry.read_only).count(),
            rebis_lang::std_modules().len()
        );
        assert_eq!(
            lib.load_catalog("std/flow").unwrap(),
            rebis_lang::std_modules()
                .iter()
                .find(|(name, _)| *name == "std/flow")
                .unwrap()
                .1
        );
        assert_eq!(lib.search_catalog("std/spr").len(), 1);
        let _ = fs::remove_dir_all(lib.root());
    }

    #[test]
    fn search_is_case_insensitive_and_matches_folders() {
        let lib = temp("search");
        lib.save("team/Reviews", "x").unwrap();
        lib.save("repair-loop", "x").unwrap();
        assert_eq!(lib.search("review").len(), 1);
        assert_eq!(lib.search("TEAM").len(), 1);
        assert_eq!(lib.search("").len(), 2, "an empty query lists everything");
        assert!(lib.search("nothing").is_empty());
        let _ = fs::remove_dir_all(lib.root());
    }

    #[test]
    fn listing_a_missing_library_is_empty_not_an_error() {
        assert!(Library::new("/nonexistent/kaos/sigils").list().is_empty());
    }

    #[test]
    fn non_rebis_files_are_ignored() {
        let lib = temp("ignore");
        lib.save("real", "x").unwrap();
        fs::write(lib.root().join("notes.txt"), "not a sigil").unwrap();
        fs::write(lib.root().join("real.output"), "sidecar").unwrap();
        let names: Vec<String> = lib.list().into_iter().map(|e| e.name).collect();
        assert_eq!(names, vec!["real".to_string()]);
        let _ = fs::remove_dir_all(lib.root());
    }
}
