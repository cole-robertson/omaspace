//! The daemon: HTTP on this machine's Tailscale IPv4 address only, every
//! request identified with tailscaled whois and refused unless it comes from
//! one of your own devices.

use crate::capture::{self, Scope};
use crate::restore;
use crate::snapshot::Snapshot;
use crate::tailnet::{self, Identity};
use serde_json::json;
use std::io::Read;
use std::path::{Path, PathBuf};
use tiny_http::{Header, Method, Request, Response, Server};

pub const PORT: u16 = 7787;
const MAX_BODY: u64 = 4 * 1024 * 1024;

pub struct Daemon {
    pub me: Identity,
    pub home: PathBuf,
    pub stash: PathBuf,
    pub desktop: bool,
}

pub fn serve(daemon: Daemon) -> anyhow::Result<()> {
    let addr = format!("{}:{PORT}", tailnet::my_ipv4()?);
    let server = Server::http(&addr).map_err(|e| anyhow::anyhow!("binding {addr}: {e}"))?;
    std::fs::create_dir_all(&daemon.stash)?;
    eprintln!(
        "omaspace serving {addr} as {} (desktop: {})",
        daemon.me.name, daemon.desktop
    );
    for request in server.incoming_requests() {
        let caller = request.remote_addr().map(|a| a.to_string());
        let verdict = caller
            .as_deref()
            .ok_or_else(|| anyhow::anyhow!("no peer address"))
            .and_then(tailnet::whois);
        let response = match verdict {
            Ok(id) if tailnet::trusted(&daemon.me, &id) => handle(&daemon, request, &id),
            Ok(id) => {
                eprintln!("refused {} ({:?})", id.name, caller);
                reply(request, 403, json!({"error": "not one of your devices"}))
            }
            Err(e) => reply(
                request,
                403,
                json!({"error": format!("unidentified caller: {e}")}),
            ),
        };
        if let Err(e) = response {
            eprintln!("response failed: {e}");
        }
    }
    Ok(())
}

fn reply(request: Request, status: u16, body: serde_json::Value) -> std::io::Result<()> {
    let header = Header::from_bytes("Content-Type", "application/json").unwrap();
    request.respond(
        Response::from_string(body.to_string())
            .with_status_code(status)
            .with_header(header),
    )
}

fn handle(daemon: &Daemon, mut request: Request, caller: &Identity) -> std::io::Result<()> {
    let url = request.url().to_string();
    let (path, query) = url.split_once('?').unwrap_or((&url, ""));
    let method = request.method().clone();
    eprintln!("{method} {path} from {}", caller.name);
    // Files: streamed, not JSON-buffered.
    if path.starts_with("/v1/files/") {
        let r = files(daemon, &mut request, &method, path, query);
        return match r {
            Ok(FileReply::Json(v)) => reply(request, 200, v),
            Ok(FileReply::Stream(file, offset, len)) => {
                let f = std::fs::File::open(&file)?;
                let mut f = std::io::BufReader::new(f);
                std::io::Seek::seek(&mut f, std::io::SeekFrom::Start(offset))?;
                let header =
                    Header::from_bytes("Content-Type", "application/octet-stream").unwrap();
                request.respond(Response::new(
                    200.into(),
                    vec![header],
                    f,
                    Some((len - offset) as usize),
                    None,
                ))
            }
            Err(e) => reply(request, 400, json!({ "error": e.to_string() })),
        };
    }
    let result: anyhow::Result<(u16, serde_json::Value)> = (|| match (&method, path) {
        (Method::Get, "/v1/hello") => Ok((
            200,
            json!({
                "name": daemon.me.name, "version": env!("CARGO_PKG_VERSION"), "desktop": daemon.desktop,
            }),
        )),
        (Method::Get, "/v1/workspaces") => {
            anyhow::ensure!(daemon.desktop, "this machine has no desktop session");
            Ok((200, crate::hypr::workspaces_summary()?))
        }
        (Method::Get, "/v1/snapshot") => {
            anyhow::ensure!(daemon.desktop, "this machine has no desktop session");
            let scope = scope_from(query)?;
            // The caller asks for sensitive items; this machine's own policy
            // still has to allow them (the data leaves *this* machine).
            let local = crate::profile::policy_allowed(&daemon.home);
            let asked: Vec<String> = query
                .split('&')
                .find_map(|kv| kv.strip_prefix("allow="))
                .unwrap_or("")
                .split(',')
                .filter(|i| local.iter().any(|l| l == i))
                .map(String::from)
                .collect();
            let (snapshot, skipped) =
                capture::capture_with(scope, &daemon.me.name, &daemon.home, &asked)?;
            let skipped: Vec<_> = skipped
                .iter()
                .map(|s| json!({"title": s.title, "reason": s.reason}))
                .collect();
            Ok((200, json!({ "snapshot": snapshot, "skipped": skipped })))
        }
        (Method::Post, "/v1/restore") => {
            anyhow::ensure!(daemon.desktop, "this machine has no desktop session");
            let snapshot: Snapshot = serde_json::from_str(&read_body(&mut request)?)?;
            Ok((
                200,
                serde_json::to_value(restore::restore(&snapshot, &daemon.home)?)?,
            ))
        }
        (Method::Get, "/v1/stash") => Ok((200, json!(list_stash(&daemon.stash)?))),
        (Method::Put, p) if p.starts_with("/v1/stash/") => {
            let id = stash_id(p)?;
            let snapshot: Snapshot = serde_json::from_str(&read_body(&mut request)?)?;
            snapshot.validate()?;
            std::fs::write(
                daemon.stash.join(format!("{id}.json")),
                serde_json::to_vec(&snapshot)?,
            )?;
            Ok((
                200,
                json!({ "stored": id, "windows": snapshot.window_count() }),
            ))
        }
        (Method::Get, p) if p.starts_with("/v1/stash/") => {
            let file = daemon.stash.join(format!("{}.json", stash_id(p)?));
            Ok((200, serde_json::from_slice(&std::fs::read(file)?)?))
        }
        _ => Ok((404, json!({ "error": "not found" }))),
    })();
    match result {
        Ok((status, body)) => reply(request, status, body),
        Err(e) => reply(request, 400, json!({ "error": e.to_string() })),
    }
}

enum FileReply {
    Json(serde_json::Value),
    /// Stream this file from this offset; total length.
    Stream(std::path::PathBuf, u64, u64),
}

fn param(query: &str, key: &str) -> Option<String> {
    query
        .split('&')
        .find_map(|kv| kv.strip_prefix(&format!("{key}=")))
        .map(percent_decode)
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'%' if i + 2 < b.len() => {
                if let Ok(v) = u8::from_str_radix(&s[i + 1..i + 3], 16) {
                    out.push(v);
                    i += 3;
                    continue;
                }
                out.push(b'%');
            }
            b'+' => out.push(b' '),
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn files(
    daemon: &Daemon,
    request: &mut Request,
    method: &Method,
    path: &str,
    query: &str,
) -> anyhow::Result<FileReply> {
    let target = crate::xfer::resolve(
        &daemon.home,
        &param(query, "path").unwrap_or_else(|| "~/Downloads".into()),
    )?;
    let portable = |p: &std::path::Path| crate::snapshot::to_portable(p, &daemon.home);
    match (method, path) {
        (Method::Get, "/v1/files/list") => {
            if !target.exists() && target == daemon.home.join("Downloads") {
                std::fs::create_dir_all(&target)?;
            }
            let entries = crate::xfer::list(&target)?;
            Ok(FileReply::Json(
                json!({ "path": portable(&target), "entries": entries }),
            ))
        }
        (Method::Get, "/v1/files/stat") => {
            let m = std::fs::metadata(&target);
            Ok(FileReply::Json(json!({
                "path": portable(&target),
                "exists": m.is_ok(),
                "dir": m.as_ref().is_ok_and(|m| m.is_dir()),
                "size": m.as_ref().map_or(0, |m| m.len()),
                "received": crate::xfer::received(&target),
            })))
        }
        (Method::Get, "/v1/files/hash") => Ok(FileReply::Json(
            json!({ "sha256": crate::xfer::hash_file(&target)? }),
        )),
        (Method::Get, "/v1/files/get") => {
            let len = std::fs::metadata(&target)?.len();
            let offset: u64 = param(query, "offset")
                .and_then(|o| o.parse().ok())
                .unwrap_or(0)
                .min(len);
            anyhow::ensure!(target.is_file(), "not a file");
            Ok(FileReply::Stream(target, offset, len))
        }
        (Method::Put, "/v1/files/put") => {
            let offset: u64 = param(query, "offset")
                .and_then(|o| o.parse().ok())
                .unwrap_or(0);
            let got = crate::xfer::put_chunk(&target, offset, request.as_reader())?;
            Ok(FileReply::Json(json!({ "received": got })))
        }
        (Method::Post, "/v1/files/commit") => {
            let size: u64 = param(query, "size")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let sha = param(query, "sha256").unwrap_or_default();
            let replace = param(query, "replace").as_deref() == Some("1");
            let landed = crate::xfer::commit(&target, size, &sha, replace)?;
            if let Some(mtime) = param(query, "mtime").and_then(|v| v.parse::<u64>().ok()) {
                let t = std::time::UNIX_EPOCH + std::time::Duration::from_secs(mtime);
                let _ = std::fs::File::options()
                    .write(true)
                    .open(&landed)
                    .and_then(|f| f.set_modified(t));
            }
            Ok(FileReply::Json(json!({ "path": portable(&landed) })))
        }
        (Method::Post, "/v1/files/delete") => {
            anyhow::ensure!(target.is_file(), "only files can be deleted");
            std::fs::remove_file(&target)?;
            Ok(FileReply::Json(json!({ "deleted": portable(&target) })))
        }
        (Method::Post, "/v1/files/mkdir") => {
            std::fs::create_dir_all(&target)?;
            Ok(FileReply::Json(json!({ "path": portable(&target) })))
        }
        _ => anyhow::bail!("unknown files endpoint"),
    }
}

fn read_body(request: &mut Request) -> anyhow::Result<String> {
    let mut body = String::new();
    request
        .as_reader()
        .take(MAX_BODY + 1)
        .read_to_string(&mut body)?;
    anyhow::ensure!(body.len() as u64 <= MAX_BODY, "snapshot too large");
    Ok(body)
}

pub fn scope_from(query: &str) -> anyhow::Result<Scope> {
    if let Some(address) = query.split('&').find_map(|kv| kv.strip_prefix("window=")) {
        anyhow::ensure!(
            address.starts_with("0x") && address[2..].chars().all(|c| c.is_ascii_hexdigit()),
            "invalid window address"
        );
        return Ok(Scope::Window(address.to_string()));
    }
    let value = query
        .split('&')
        .find_map(|kv| kv.strip_prefix("workspace="))
        .unwrap_or("active");
    Ok(match value {
        "active" => Scope::Active,
        "all" => Scope::All,
        n => Scope::One(n.parse()?),
    })
}

fn stash_id(path: &str) -> anyhow::Result<&str> {
    let id = path.trim_start_matches("/v1/stash/");
    anyhow::ensure!(
        !id.is_empty()
            && id.len() <= 128
            && id
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c))
            && !id.starts_with('.'),
        "invalid stash id"
    );
    Ok(id)
}

fn list_stash(dir: &Path) -> anyhow::Result<Vec<serde_json::Value>> {
    let mut out = Vec::new();
    for entry in std::fs::read_dir(dir)?.flatten() {
        if let Ok(snapshot) = serde_json::from_slice::<Snapshot>(&std::fs::read(entry.path())?) {
            out.push(json!({
                "id": entry.path().file_stem().map(|s| s.to_string_lossy().into_owned()),
                "source": snapshot.source, "taken_at": snapshot.taken_at, "windows": snapshot.window_count(),
            }));
        }
    }
    out.sort_by_key(|v| std::cmp::Reverse(v["taken_at"].as_u64()));
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stash_ids_cannot_escape_the_stash_directory() {
        assert!(stash_id("/v1/stash/bravo-1790000000").is_ok());
        assert!(stash_id("/v1/stash/../../.ssh/authorized_keys").is_err());
        assert!(stash_id("/v1/stash/.hidden").is_err());
        assert!(stash_id("/v1/stash/").is_err());
    }

    #[test]
    fn workspace_scope_parses_from_the_query() {
        assert!(matches!(scope_from("").unwrap(), Scope::Active));
        assert!(matches!(scope_from("workspace=all").unwrap(), Scope::All));
        assert!(matches!(scope_from("workspace=3").unwrap(), Scope::One(3)));
        assert!(
            matches!(scope_from("window=0x5d4bc1d1ce90").unwrap(), Scope::Window(a) if a == "0x5d4bc1d1ce90")
        );
        assert!(scope_from("window=$(rm -rf)").is_err());
        assert!(scope_from("workspace=x").is_err());
    }
}
