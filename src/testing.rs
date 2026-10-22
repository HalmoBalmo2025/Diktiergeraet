//! Hilfen für Tests: ein temporärer Ordner, der beim Wegfallen samt Inhalt
//! verschwindet, und Dateien darin. Tests berühren nie die echten Dateien
//! des Nutzers (testing.md).

use std::fs;
use std::io::Write as _;
use std::path::{Path, PathBuf};

/// Ein eigener Ordner unter dem temporären Ordner des Systems; fällt er
/// weg, wird er gelöscht.
#[derive(Debug)]
pub(crate) struct TempDir {
    path: PathBuf,
}

impl TempDir {
    /// Ein frischer, leerer Ordner; `name` macht ihn je Test eindeutig.
    pub(crate) fn new(name: &str) -> Self {
        let path = std::env::temp_dir().join(format!("diktator-{name}-{}", std::process::id()));
        // Reste eines abgebrochenen Laufs desselben Tests.
        remove(&path);
        fs::create_dir_all(&path).unwrap();
        Self { path }
    }

    /// Der Ordner.
    pub(crate) fn path(&self) -> &Path {
        &self.path
    }

    /// Legt die Datei `name` mit `content` neu an.
    pub(crate) fn file(&self, name: &str, content: impl AsRef<[u8]>) -> PathBuf {
        let path = self.path.join(name);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::File::create_new(&path)
            .unwrap()
            .write_all(content.as_ref())
            .unwrap();
        path
    }

    /// Ein ausführbares Shell-Skript `name` mit `body`.
    pub(crate) fn script(&self, name: &str, body: &str) -> PathBuf {
        let path = self.file(name, format!("#!/bin/sh\n{body}"));
        fs::set_permissions(&path, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
        path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        remove(&self.path);
    }
}

/// Löscht `path` samt Inhalt, falls es ihn gibt.
#[expect(
    clippy::disallowed_methods,
    reason = "Tests räumen nur ihren eigenen temporären Ordner auf"
)]
fn remove(path: &Path) {
    match fs::remove_dir_all(path) {
        // Fehlt der Ordner, ist nichts zu tun.
        Ok(()) | Err(_) => {}
    }
}
