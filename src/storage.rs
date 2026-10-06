//! Chromium localStorage (`<profile>/Local Storage/leveldb`), read and merged
//! per origin. Keys, as Chromium writes them:
//!
//! ```text
//! VERSION                       "1"
//! META:<origin>                 per-origin metadata (protobuf)
//! METAACCESS:<origin>           last access (protobuf)
//! _<origin> 0x00 <fmt><key>     value = <fmt><bytes>
//! ```
//! `<fmt>` is 0x01 (Latin-1) or 0x00 (UTF-16LE). Values are carried as raw
//! bytes, so both encodings round-trip unchanged.
//!
//! Reading uses a private copy (a running browser holds the LevelDB lock).
//! Writing needs the browser for that profile to be closed.

use base64::Engine;
use rusty_leveldb::{DB, LdbIterator, Options};
use serde::{Deserialize, Serialize};
use std::path::Path;

/// One localStorage entry: the full LevelDB key and value, base64 for JSON.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub origin: String,
    pub key: String,
    pub value: String,
}

fn b64(bytes: &[u8]) -> String {
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

fn unb64(text: &str) -> anyhow::Result<Vec<u8>> {
    Ok(base64::engine::general_purpose::STANDARD.decode(text)?)
}

fn store(profile: &Path) -> std::path::PathBuf {
    profile.join("Local Storage/leveldb")
}

/// `_https://github.com\0…` → `https://github.com`
fn origin_of(key: &[u8]) -> Option<&str> {
    let rest = key.strip_prefix(b"_")?;
    let end = rest.iter().position(|b| *b == 0)?;
    std::str::from_utf8(&rest[..end]).ok()
}

/// Whether an origin (`https://app.example.com`, `file://`) belongs to the
/// workspace: one of `sites` by host, or a local file page.
pub fn origin_matches(origin: &str, sites: &[String]) -> bool {
    if sites.iter().any(|s| s == crate::profile::ALL_SITES) {
        return true;
    }
    if origin.starts_with("file://") {
        return sites.iter().any(|s| s == "file");
    }
    let host = origin
        .split("://")
        .nth(1)
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");
    crate::cookies::host_matches(host, sites)
}

/// All data entries for `sites` (plus each origin's `META:` key so the
/// receiver sees a well-formed origin).
pub fn read(profile: &Path, sites: &[String]) -> anyhow::Result<Vec<Entry>> {
    let src = store(profile);
    if !src.is_dir() {
        return Ok(Vec::new());
    }
    let copy = tempfile::tempdir()?;
    for file in std::fs::read_dir(&src)?.flatten() {
        if file.file_name() != "LOCK" {
            std::fs::copy(file.path(), copy.path().join(file.file_name()))?;
        }
    }
    let mut db = DB::open(copy.path(), Options::default())
        .map_err(|e| anyhow::anyhow!("opening localStorage: {e}"))?;
    let mut iter = db.new_iter().map_err(|e| anyhow::anyhow!("{e}"))?;
    let mut out = Vec::new();
    while iter.advance() {
        let Some((key, value)) = iter.current() else {
            continue;
        };
        let (key, value) = (key.to_vec(), value.to_vec());
        let origin = if let Some(o) = origin_of(&key) {
            o.to_string()
        } else if let Some(o) = key
            .strip_prefix(b"META:")
            .and_then(|o| std::str::from_utf8(o).ok())
        {
            o.to_string()
        } else {
            continue;
        };
        if origin_matches(&origin, sites) {
            out.push(Entry {
                origin,
                key: b64(&key),
                value: b64(&value),
            });
        }
    }
    Ok(out)
}

/// Merge entries into the profile's localStorage. The browser for this profile
/// must be closed. Existing keys for the same origin and name are replaced;
/// everything else stays.
pub fn write(profile: &Path, entries: &[Entry]) -> anyhow::Result<usize> {
    let dir = store(profile);
    std::fs::create_dir_all(&dir)?;
    let options = Options {
        create_if_missing: true,
        ..Options::default()
    };
    let mut db = DB::open(&dir, options)
        .map_err(|e| anyhow::anyhow!("opening localStorage (is the browser closed?): {e}"))?;
    if db.get(b"VERSION").is_none() {
        db.put(b"VERSION", b"1")
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    for entry in entries {
        let key = unb64(&entry.key)?;
        anyhow::ensure!(
            key.starts_with(b"_") || key.starts_with(b"META:"),
            "refusing a non-localStorage key"
        );
        db.put(&key, &unb64(&entry.value)?)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
    }
    db.flush().map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(entries.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data_key(origin: &str, name: &str) -> Vec<u8> {
        let mut key = format!("_{origin}").into_bytes();
        key.push(0);
        key.push(1);
        key.extend(name.as_bytes());
        key
    }

    #[test]
    fn entries_for_workspace_sites_round_trip_and_merge() {
        let src = tempfile::tempdir().unwrap();
        {
            let o = Options {
                create_if_missing: true,
                ..Options::default()
            };
            let mut db = DB::open(src.path().join("Local Storage/leveldb"), o).unwrap();
            db.put(b"VERSION", b"1").unwrap();
            db.put(&data_key("https://app.example.com", "todo"), b"\x01[\"a\"]")
                .unwrap();
            db.put(&data_key("https://other.test", "secret"), b"\x01x")
                .unwrap();
            db.put(&data_key("file://", "agent-todo"), b"\x01[\"b\"]")
                .unwrap();
            db.flush().unwrap();
        }
        let sites = vec!["example.com".to_string(), "file".to_string()];
        let entries = read(src.path(), &sites).unwrap();
        assert_eq!(entries.len(), 2, "other.test stays behind");

        let dst = tempfile::tempdir().unwrap();
        {
            let o = Options {
                create_if_missing: true,
                ..Options::default()
            };
            let mut db = DB::open(dst.path().join("Local Storage/leveldb"), o).unwrap();
            db.put(&data_key("https://mine.test", "keep"), b"\x01k")
                .unwrap();
            db.flush().unwrap();
        }
        write(dst.path(), &entries).unwrap();
        let mut db =
            DB::open(dst.path().join("Local Storage/leveldb"), Options::default()).unwrap();
        assert_eq!(
            db.get(&data_key("https://app.example.com", "todo"))
                .unwrap(),
            b"\x01[\"a\"]".to_vec()
        );
        assert_eq!(
            db.get(&data_key("file://", "agent-todo")).unwrap(),
            b"\x01[\"b\"]".to_vec()
        );
        assert_eq!(
            db.get(&data_key("https://mine.test", "keep")).unwrap(),
            b"\x01k".to_vec(),
            "receiver's data stays"
        );
    }
}
