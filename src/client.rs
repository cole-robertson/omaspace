//! Talking to other machines' daemons over the tailnet.

use crate::server::PORT;
use crate::snapshot::Snapshot;
use crate::tailnet;
use anyhow::Context;
use serde_json::Value;

pub struct PeerInfo {
    pub name: String,
    pub desktop: bool,
    pub version: String,
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(60)))
        .http_status_as_error(false)
        .build()
        .into()
}

/// Resolve a peer name to its Tailscale IPv4 address; only trusted peers.
fn address(name: &str) -> anyhow::Result<String> {
    let me = tailnet::me()?;
    let peer = tailnet::peers()?
        .into_iter()
        .find(|p| p.host.eq_ignore_ascii_case(name))
        .with_context(|| format!("{name} is not on your tailnet"))?;
    anyhow::ensure!(
        tailnet::peer_trusted(&me, &peer),
        "{name} is not one of your devices"
    );
    anyhow::ensure!(peer.online, "{name} is offline");
    let ip = peer
        .ips
        .into_iter()
        .find(|ip| ip.contains('.'))
        .context("peer has no IPv4")?;
    Ok(format!("http://{ip}:{PORT}"))
}

fn check(mut response: ureq::http::Response<ureq::Body>) -> anyhow::Result<Value> {
    let status = response.status();
    let body: Value = response.body_mut().read_json()?;
    anyhow::ensure!(
        status.is_success(),
        "{}",
        body["error"].as_str().unwrap_or("request failed")
    );
    Ok(body)
}

pub fn get(peer: &str, path: &str) -> anyhow::Result<Value> {
    check(agent().get(format!("{}{path}", address(peer)?)).call()?)
}

fn send_json(
    peer: &str,
    method: &str,
    path: &str,
    body: &impl serde::Serialize,
) -> anyhow::Result<Value> {
    let url = format!("{}{path}", address(peer)?);
    let request = match method {
        "PUT" => agent().put(url),
        _ => agent().post(url),
    };
    check(request.send_json(body)?)
}

/// This machine and every desktop peer, each with its workspaces and windows,
/// for the Spaces panel. A peer that doesn't answer is listed with its error
/// rather than left out, so the panel can say why.
pub fn spaces() -> anyhow::Result<Value> {
    let me = tailnet::me()?;
    let mut machines = vec![serde_json::json!({
        "name": me.name, "here": true, "workspaces": crate::hypr::workspaces_summary()?,
    })];
    let synced = crate::sync::folders();
    for peer in peers()?.into_iter().filter(|p| p.desktop) {
        let mut entry = match get(&peer.name, "/v1/workspaces") {
            Ok(w) => serde_json::json!({"name": peer.name, "here": false, "workspaces": w}),
            Err(e) => serde_json::json!({"name": peer.name, "here": false, "error": e.to_string()}),
        };
        // Folders kept in sync with this machine, and whether any is failing.
        let mine: Vec<_> = synced.iter().filter(|f| f.peer == peer.name).collect();
        entry["synced"] = serde_json::json!(mine.len());
        entry["sync_errors"] = serde_json::json!(
            mine.iter()
                .filter(|f| crate::sync::status(f).last_error.is_some())
                .count()
        );
        machines.push(entry);
    }
    Ok(serde_json::json!({"machines": machines}))
}

pub fn peers() -> anyhow::Result<Vec<PeerInfo>> {
    let me = tailnet::me()?;
    let mut out = Vec::new();
    for peer in tailnet::peers()? {
        if !peer.online || !tailnet::peer_trusted(&me, &peer) || peer.os != "linux" {
            continue;
        }
        let Some(ip) = peer.ips.iter().find(|ip| ip.contains('.')) else {
            continue;
        };
        let quick: ureq::Agent = ureq::Agent::config_builder()
            .timeout_global(Some(std::time::Duration::from_millis(1500)))
            .build()
            .into();
        if let Ok(mut r) = quick.get(format!("http://{ip}:{PORT}/v1/hello")).call()
            && let Ok(hello) = r.body_mut().read_json::<Value>()
        {
            out.push(PeerInfo {
                name: peer.host.clone(),
                desktop: hello["desktop"].as_bool().unwrap_or(false),
                version: hello["version"].as_str().unwrap_or("?").into(),
            });
        }
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

pub fn snapshot_from(peer: &str, query: &str) -> anyhow::Result<(Snapshot, usize)> {
    let body = get(peer, &format!("/v1/snapshot?{query}"))?;
    let skipped = body["skipped"].as_array().map_or(0, Vec::len);
    Ok((serde_json::from_value(body["snapshot"].clone())?, skipped))
}

pub fn restore_on(peer: &str, snapshot: &Snapshot) -> anyhow::Result<Value> {
    send_json(peer, "POST", "/v1/restore", snapshot)
}

pub fn stash_put(hub: &str, snapshot: &Snapshot) -> anyhow::Result<Value> {
    send_json(hub, "PUT", &format!("/v1/stash/{}", snapshot.id), snapshot)
}

pub fn stash_list(hub: &str) -> anyhow::Result<Value> {
    get(hub, "/v1/stash")
}

pub fn stash_get(hub: &str, id: &str) -> anyhow::Result<Snapshot> {
    Ok(serde_json::from_value(get(
        hub,
        &format!("/v1/stash/{id}"),
    )?)?)
}

pub fn print_report(report: &Value, skipped_at_capture: usize) {
    for line in report["profile"].as_array().into_iter().flatten() {
        println!("browser   {}", line.as_str().unwrap_or(""));
    }
    for window in report["windows"].as_array().into_iter().flatten() {
        let detail = window["reason"]
            .as_str()
            .or(window["error"].as_str())
            .unwrap_or("");
        println!(
            "{:<9} ws{:<3} {} {}",
            window["result"].as_str().unwrap_or("?"),
            window["workspace"],
            window["title"].as_str().unwrap_or(""),
            detail
        );
    }
    println!(
        "restored {}, skipped {}, failed {}{}",
        report["restored"],
        report["skipped"],
        report["failed"],
        if skipped_at_capture > 0 {
            format!(" (+{skipped_at_capture} not capturable)")
        } else {
            String::new()
        }
    );
}

/// A restore with any failed window exits non-zero; skips are reported but expected.
pub fn fail_if_partial(report: &Value) -> anyhow::Result<()> {
    anyhow::ensure!(
        report["failed"].as_u64() == Some(0),
        "{} window(s) failed to restore",
        report["failed"]
    );
    Ok(())
}

// ---- files ------------------------------------------------------------------

/// Percent-encode a path for a query string.
pub fn enc(s: &str) -> String {
    s.bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

fn big_agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(std::time::Duration::from_secs(600)))
        .http_status_as_error(false)
        .build()
        .into()
}

pub fn files_list(peer: &str, path: &str) -> anyhow::Result<Value> {
    get(peer, &format!("/v1/files/list?path={}", enc(path)))
}

pub fn files_stat(peer: &str, path: &str) -> anyhow::Result<Value> {
    get(peer, &format!("/v1/files/stat?path={}", enc(path)))
}

/// Upload one local file to `remote` on `peer`, resuming a partial upload,
/// and verify it. Returns where it landed.
pub fn put_file(
    peer: &str,
    local: &std::path::Path,
    remote: &str,
    replace: bool,
    progress: &mut dyn FnMut(u64, u64),
) -> anyhow::Result<String> {
    use std::io::{Read, Seek, SeekFrom};
    let base = address(peer)?;
    let size = std::fs::metadata(local)?.len();
    let sha = crate::xfer::hash_file(local)?;
    let mut offset = files_stat(peer, remote)?["received"].as_u64().unwrap_or(0);
    if offset > size {
        offset = 0;
    }
    let mut f = std::fs::File::open(local)?;
    f.seek(SeekFrom::Start(offset))?;
    let mut buf = vec![0u8; crate::xfer::CHUNK];
    progress(offset, size);
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 && offset > 0 {
            break;
        }
        let url = format!("{base}/v1/files/put?path={}&offset={offset}", enc(remote));
        let r = check(big_agent().put(url).send(&buf[..n])?)?;
        offset = r["received"].as_u64().unwrap_or(offset + n as u64);
        progress(offset, size);
        if n == 0 {
            break;
        }
    }
    let mtime = std::fs::metadata(local)?
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map_or(0, |d| d.as_secs());
    let url = format!(
        "{base}/v1/files/commit?path={}&size={size}&sha256={sha}&mtime={mtime}&replace={}",
        enc(remote),
        u8::from(replace)
    );
    let r = check(big_agent().post(url).send_empty()?)?;
    Ok(r["path"].as_str().unwrap_or(remote).to_string())
}

/// Download `remote` from `peer` into `local`, resuming a partial file, and
/// verify the SHA-256 against the source.
pub fn get_file(
    peer: &str,
    remote: &str,
    local: &std::path::Path,
    progress: &mut dyn FnMut(u64, u64),
) -> anyhow::Result<()> {
    use std::io::Write;
    let base = address(peer)?;
    let stat = files_stat(peer, remote)?;
    anyhow::ensure!(
        stat["exists"].as_bool() == Some(true) && stat["dir"].as_bool() == Some(false),
        "{remote} is not a file on {peer}"
    );
    let size = stat["size"].as_u64().unwrap_or(0);
    let part = crate::xfer::part_path(local);
    let mut offset = std::fs::metadata(&part).map_or(0, |m| m.len()).min(size);
    if let Some(parent) = local.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut out = crate::xfer::open_no_follow(&part, false)?;
    out.set_len(offset)?;
    std::io::Seek::seek(&mut out, std::io::SeekFrom::Start(offset))?;
    progress(offset, size);
    if offset < size {
        let mut resp = big_agent()
            .get(format!(
                "{base}/v1/files/get?path={}&offset={offset}",
                enc(remote)
            ))
            .call()?;
        anyhow::ensure!(
            resp.status().is_success(),
            "download failed: {}",
            resp.status()
        );
        let mut reader = resp.body_mut().as_reader();
        let mut buf = vec![0u8; 1 << 20];
        loop {
            let n = std::io::Read::read(&mut reader, &mut buf)?;
            if n == 0 {
                break;
            }
            out.write_all(&buf[..n])?;
            offset += n as u64;
            progress(offset, size);
        }
    }
    out.flush()?;
    drop(out);
    let want = get(peer, &format!("/v1/files/hash?path={}", enc(remote)))?["sha256"]
        .as_str()
        .unwrap_or("")
        .to_string();
    let got = crate::xfer::hash_file(&part)?;
    if got != want {
        let _ = std::fs::remove_file(&part);
        anyhow::bail!("checksum mismatch downloading {remote}; discarded");
    }
    std::fs::rename(&part, crate::xfer::free_name(local))?;
    Ok(())
}

pub fn post_empty(peer: &str, path: &str) -> anyhow::Result<Value> {
    check(
        agent()
            .post(format!("{}{path}", address(peer)?))
            .send_empty()?,
    )
}
