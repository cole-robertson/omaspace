//! Chromium cookies on Linux: read and decrypt on the sender, re-encrypt with
//! the receiver's own key and merge into its `Cookies` database.
//!
//! Linux Chromium encrypts values with AES-128-CBC, IV = 16 spaces, key =
//! PBKDF2-HMAC-SHA1(secret, "saltysalt", 1 round, 16 bytes):
//! - `v10` values use the fixed secret "peanuts";
//! - `v11` values use the secret from libsecret (`application=chrome`, or
//!   `chromium` on Arch/Omarchy).
//!
//! Chrome 130+ (Cookies `meta.version` >= 24) prefixes the plaintext with
//! SHA-256(host_key) and drops rows whose prefix doesn't match.

use aes::cipher::{BlockDecryptMut, BlockEncryptMut, KeyIvInit, block_padding::Pkcs7};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::Path;
use zeroize::Zeroizing;

type Enc = cbc::Encryptor<aes::Aes128>;
type Dec = cbc::Decryptor<aes::Aes128>;

const IV: [u8; 16] = [b' '; 16];
const DIGEST_META_VERSION: i64 = 24;

/// One cookie in plaintext, for transfer. Same columns Chromium keys on.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Cookie {
    pub host_key: String,
    pub top_frame_site_key: String,
    pub name: String,
    pub value: String,
    pub path: String,
    pub expires_utc: i64,
    pub is_secure: bool,
    pub is_httponly: bool,
    pub samesite: i64,
    pub source_scheme: i64,
    pub source_port: i64,
    pub has_expires: bool,
    pub is_persistent: bool,
    pub priority: i64,
    /// Part of Chromium's unique key (`cookies_unique_index`), with the rest of
    /// (host, top frame site, name, path, scheme, port); must round-trip or a
    /// written cookie sits next to the real one instead of replacing it.
    #[serde(default)]
    pub has_cross_site_ancestor: bool,
    #[serde(default)]
    pub source_type: i64,
}

pub struct Keys {
    v10: Zeroizing<[u8; 16]>,
    v11: Option<Zeroizing<[u8; 16]>>,
}

fn derive(secret: &[u8]) -> Zeroizing<[u8; 16]> {
    let mut key = Zeroizing::new([0u8; 16]);
    pbkdf2::pbkdf2_hmac::<sha1::Sha1>(secret, b"saltysalt", 1, key.as_mut());
    key
}

impl Keys {
    /// This machine's keys. The libsecret lookup is attempted lazily by the
    /// caller only when a `v11` value is actually present.
    pub fn local() -> Keys {
        Keys {
            v10: derive(b"peanuts"),
            v11: libsecret_secret().map(|s| derive(&s)),
        }
    }

    #[cfg(test)]
    pub fn with_v11(secret: &[u8]) -> Keys {
        Keys {
            v10: derive(b"peanuts"),
            v11: Some(derive(secret)),
        }
    }

    fn decrypt(&self, blob: &[u8]) -> anyhow::Result<Vec<u8>> {
        let (key, body) = match blob.get(..3) {
            Some(b"v10") => (&self.v10, &blob[3..]),
            Some(b"v11") => (
                self.v11
                    .as_ref()
                    .context("v11 cookie but no libsecret key")?,
                &blob[3..],
            ),
            _ => anyhow::bail!("unsupported cookie encryption"),
        };
        Dec::new(key.as_ref().into(), &IV.into())
            .decrypt_padded_vec_mut::<Pkcs7>(body)
            .map_err(|_| anyhow::anyhow!("cookie did not decrypt"))
    }

    /// Encrypt for this machine: `v11` when a libsecret key exists (what a
    /// `--password-store=gnome-libsecret` Chromium, Omarchy's default, reads),
    /// else `v10`.
    fn encrypt(&self, plain: &[u8]) -> Vec<u8> {
        let (tag, key) = match &self.v11 {
            Some(k) => (b"v11", k),
            None => (b"v10", &self.v10),
        };
        let mut out = tag.to_vec();
        out.extend(
            Enc::new(key.as_ref().into(), &IV.into()).encrypt_padded_vec_mut::<Pkcs7>(plain),
        );
        out
    }
}

/// Chromium's libsecret password: `chrome`, then `chromium` (Arch/Omarchy).
fn libsecret_secret() -> Option<Zeroizing<Vec<u8>>> {
    for app in ["chrome", "chromium"] {
        let out = std::process::Command::new("secret-tool")
            .args(["lookup", "application", app])
            .output()
            .ok()?;
        let mut secret = out.stdout;
        while secret.last().is_some_and(|b| *b == b'\n') {
            secret.pop();
        }
        if out.status.success() && !secret.is_empty() {
            return Some(Zeroizing::new(secret));
        }
    }
    None
}

fn meta_version(conn: &rusqlite::Connection) -> i64 {
    conn.query_row("SELECT value FROM meta WHERE key = 'version'", [], |r| {
        r.get::<_, String>(0)
    })
    .ok()
    .and_then(|v| v.parse().ok())
    .unwrap_or(0)
}

/// Whether `host_key` belongs to one of `sites` (registrable hosts like
/// `github.com`): exact, or a subdomain, with or without a leading dot.
pub fn host_matches(host_key: &str, sites: &[String]) -> bool {
    if sites.iter().any(|s| s == crate::profile::ALL_SITES) {
        return true;
    }
    let host = host_key.trim_start_matches('.');
    sites
        .iter()
        .any(|site| host == site || host.ends_with(&format!(".{site}")))
}

/// An empty Cookies database with the same schema and meta version as this
/// machine's Chromium uses (copied from the running profile's own database,
/// so it always matches the installed Chromium), for a brand-new profile.
pub fn create_empty(db: &Path) -> anyhow::Result<()> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .context("HOME")?;
    let template = crate::omarchy::browsers(&home)
        .iter()
        .map(|b| crate::omarchy::running_data_dir(b, &home).join("Default/Cookies"))
        .find(|p| p.is_file())
        .context("no Chromium profile with a Cookies database to copy the schema from")?;
    let copy = tempfile::NamedTempFile::new()?;
    std::fs::copy(&template, copy.path())?;
    let src = rusqlite::Connection::open(copy.path())?;
    let schema: Vec<String> = src
        .prepare(
            "SELECT sql FROM sqlite_master WHERE sql IS NOT NULL AND name NOT LIKE 'sqlite_%'",
        )?
        .query_map([], |r| r.get(0))?
        .collect::<Result<_, _>>()?;
    let meta: Vec<(String, String)> = src
        .prepare("SELECT key, value FROM meta")?
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))?
        .collect::<Result<_, _>>()?;
    let dst = rusqlite::Connection::open(db)?;
    for sql in schema {
        dst.execute_batch(&sql)?;
    }
    for (k, v) in meta {
        dst.execute(
            "INSERT INTO meta (key, value) VALUES (?1, ?2)",
            rusqlite::params![k, v],
        )?;
    }
    Ok(())
}

/// Read the cookies for `sites` from a profile's `Cookies` database (copied
/// first; Chromium holds it open). Rows that fail to decrypt fail the read:
/// a silently partial set would look like a working sign-in and not be one.
pub fn read(profile: &Path, sites: &[String], keys: &Keys) -> anyhow::Result<Vec<Cookie>> {
    let db = profile.join("Cookies");
    if !db.is_file() {
        return Ok(Vec::new());
    }
    let copy = tempfile::NamedTempFile::new()?;
    std::fs::copy(&db, copy.path())?;
    let conn = rusqlite::Connection::open(copy.path())?;
    let digest = meta_version(&conn) >= DIGEST_META_VERSION;
    let mut stmt = conn.prepare(
        "SELECT host_key, top_frame_site_key, name, value, encrypted_value, path, expires_utc, is_secure,
                is_httponly, samesite, source_scheme, source_port, has_expires, is_persistent, priority,
                has_cross_site_ancestor, source_type
         FROM cookies",
    )?;
    let mut out = Vec::new();
    let mut rows = stmt.query([])?;
    while let Some(row) = rows.next()? {
        let host_key: String = row.get(0)?;
        if !host_matches(&host_key, sites) {
            continue;
        }
        let plain_value: String = row.get(3)?;
        let encrypted: Vec<u8> = row.get(4)?;
        let value = if encrypted.is_empty() {
            plain_value
        } else {
            let mut plain = keys
                .decrypt(&encrypted)
                .with_context(|| format!("cookie for {host_key}"))?;
            if digest {
                let d = Sha256::digest(host_key.as_bytes());
                if plain.starts_with(&d) {
                    plain.drain(..d.len());
                }
            }
            String::from_utf8(plain)
                .with_context(|| format!("cookie for {host_key} is not text"))?
        };
        out.push(Cookie {
            host_key,
            top_frame_site_key: row.get(1)?,
            name: row.get(2)?,
            value,
            path: row.get(5)?,
            expires_utc: row.get(6)?,
            is_secure: row.get(7)?,
            is_httponly: row.get(8)?,
            samesite: row.get(9)?,
            source_scheme: row.get(10)?,
            source_port: row.get(11)?,
            has_expires: row.get(12)?,
            is_persistent: row.get(13)?,
            priority: row.get(14)?,
            has_cross_site_ancestor: row.get(15)?,
            source_type: row.get(16)?,
        });
    }
    Ok(out)
}

/// Merge `cookies` into a profile's `Cookies` database, encrypted with this
/// machine's key. The profile's browser must not be running. Other cookies are
/// left alone; a cookie with the same (host, name, path) is replaced.
pub fn write(profile: &Path, cookies: &[Cookie], keys: &Keys) -> anyhow::Result<usize> {
    let db = profile.join("Cookies");
    anyhow::ensure!(
        db.is_file(),
        "{} has no Cookies database yet; open the browser once first",
        profile.display()
    );
    let conn = rusqlite::Connection::open(&db)?;
    let digest = meta_version(&conn) >= DIGEST_META_VERSION;
    let now = chrome_now();
    let tx = conn.unchecked_transaction()?;
    for c in cookies {
        let mut plain = Vec::new();
        if digest {
            plain.extend(Sha256::digest(c.host_key.as_bytes()));
        }
        plain.extend(c.value.as_bytes());
        tx.execute(
            "INSERT OR REPLACE INTO cookies (creation_utc, host_key, top_frame_site_key, name, value,
                encrypted_value, path, expires_utc, is_secure, is_httponly, last_access_utc, has_expires,
                is_persistent, priority, samesite, source_scheme, source_port, last_update_utc,
                source_type, has_cross_site_ancestor)
             VALUES (?1, ?2, ?3, ?4, '', ?5, ?6, ?7, ?8, ?9, ?1, ?10, ?11, ?12, ?13, ?14, ?15, ?1, ?16, ?17)",
            rusqlite::params![
                now, c.host_key, c.top_frame_site_key, c.name, keys.encrypt(&plain), c.path, c.expires_utc,
                c.is_secure, c.is_httponly, c.has_expires, c.is_persistent, c.priority, c.samesite,
                c.source_scheme, c.source_port, c.source_type, c.has_cross_site_ancestor,
            ],
        )?;
    }
    tx.commit()?;
    Ok(cookies.len())
}

fn chrome_now() -> i64 {
    const UNIX_TO_CHROME_MICROS: i64 = 11_644_473_600_000_000;
    let unix = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_micros() as i64)
        .unwrap_or(0);
    unix + UNIX_TO_CHROME_MICROS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn values_round_trip_through_both_key_versions() {
        let with_v11 = Keys::with_v11(b"sekrit");
        let blob = with_v11.encrypt(b"session=abc");
        assert_eq!(&blob[..3], b"v11");
        assert_eq!(with_v11.decrypt(&blob).unwrap(), b"session=abc");

        let only_v10 = Keys {
            v10: derive(b"peanuts"),
            v11: None,
        };
        let blob = only_v10.encrypt(b"x");
        assert_eq!(&blob[..3], b"v10");
        assert_eq!(
            with_v11.decrypt(&blob).unwrap(),
            b"x",
            "any machine reads v10"
        );
    }

    #[test]
    fn a_different_machine_key_cannot_read_the_value() {
        let blob = Keys::with_v11(b"machine-a").encrypt(b"session=abc");
        assert!(
            Keys::with_v11(b"machine-b").decrypt(&blob).is_err()
                || Keys::with_v11(b"machine-b").decrypt(&blob).unwrap() != b"session=abc"
        );
    }

    #[test]
    fn only_cookies_for_the_workspace_sites_are_selected() {
        let sites = vec!["github.com".to_string()];
        assert!(host_matches(".github.com", &sites));
        assert!(host_matches("api.github.com", &sites));
        assert!(!host_matches("notgithub.com", &sites));
        assert!(!host_matches(".google.com", &sites));
    }
}

#[cfg(test)]
mod live {
    /// `OMASPACE_LIVE_PROFILE=<profile dir> cargo test live_` decrypts the
    /// real profile's example.com cookie with this machine's keys.
    #[test]
    fn live_profile_cookie_decrypts() {
        let Ok(profile) = std::env::var("OMASPACE_LIVE_PROFILE") else {
            return;
        };
        let cookies = super::read(
            std::path::Path::new(&profile),
            &["example.com".into()],
            &super::Keys::local(),
        )
        .unwrap();
        let names: Vec<_> = cookies
            .iter()
            .map(|c| format!("{}={}", c.name, c.value))
            .collect();
        eprintln!("live cookies: {names:?}");
        assert!(!cookies.is_empty());
    }
}
