mod agent;
mod apps;
mod capture;
mod cdp;
mod client;
mod cookies;
mod files;
mod hypr;
mod input;
mod mcp;
mod omarchy;
mod profile;
mod restore;
mod server;
mod setup;
mod snapshot;
mod snss;
mod sock;
mod storage;
mod stream;
mod sync;
mod tailnet;
mod view;
mod xfer;

use anyhow::Context;
use std::path::PathBuf;

const USAGE: &str = "omaspace — move Omarchy workspaces between your machines over Tailscale

  omaspace peers
  omaspace snapshot [--workspace N|all] [--out FILE]
  omaspace send <peer> [--workspace N|all|--window ADDR] [--to-workspace N] [--with-files]
                                               capture here, reopen there
  omaspace pull <peer> [--workspace N|all] [--with-files]   capture there, reopen here
      --with-files also copies the project folders the terminals/editors are in
      --allow-sensitive cookies   also bring sign-ins for the open sites (off by
                                  default; or set allow_sensitive in
                                  ~/.config/omaspace/policy.json)
  omaspace spaces                              this machine and your others: workspaces, windows (JSON)
  omaspace stash <hub> [--workspace N|all]     keep a snapshot on a hub
  omaspace stashes <hub>
  omaspace restore <FILE | hub:ID>
  omaspace put <peer> <file|dir>... [--to ~/Downloads]   copy files to a machine
  omaspace get <peer> <path>... [--to .]                  copy files from a machine
  omaspace ls <peer> [path] [--names]
  omaspace sync add <peer> <folder> [remote folder]   keep a folder the same on both machines
  omaspace sync list | remove <folder> [peer]          folders kept in sync; stop one
  omaspace sync <peer> <folder> [remote folder] [--once]   sync in the foreground
  omaspace serve
  omaspace setup                               user service + Omarchy keybindings
  omaspace mcp                                 agent tools over stdio
  omaspace view                                live Omarchy view for browsers ($XDG_RUNTIME_DIR/omaspace-view.sock,
                                               publish with tailscale serve --https=7788 unix:<socket>)";

fn home() -> anyhow::Result<PathBuf> {
    std::env::var_os("HOME")
        .map(PathBuf::from)
        .context("HOME is not set")
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

/// Sensitive items allowed for this transfer: `--allow-sensitive a,b` plus
/// the user's policy file.
fn allowed(args: &[String]) -> anyhow::Result<Vec<String>> {
    let mut out = profile::policy_allowed(&home()?);
    if let Some(list) = flag(args, "--allow-sensitive") {
        for item in list.split(',') {
            anyhow::ensure!(
                profile::SENSITIVE.contains(&item),
                "unknown sensitive item {item:?} (cookies, login_data, history)"
            );
            out.push(item.to_string());
        }
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn workspace_query(args: &[String]) -> String {
    match flag(args, "--window") {
        Some(address) => format!("window={address}"),
        None => format!(
            "workspace={}",
            flag(args, "--workspace").unwrap_or_else(|| "active".into())
        ),
    }
}

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let rest = args.get(1..).unwrap_or_default();
    match args.first().map(String::as_str) {
        Some("serve") => {
            // Folders kept in sync run here, so they survive a restart.
            std::thread::spawn(sync::run_saved);
            server::serve(server::Daemon {
                me: tailnet::me()?,
                home: home()?,
                stash: home()?.join(".local/state/omaspace/stash"),
                desktop: hypr::available(),
            })
        }
        Some("peers") => {
            for peer in client::peers()? {
                println!(
                    "{:<14} {:<8} {}",
                    peer.name,
                    if peer.desktop { "desktop" } else { "hub" },
                    peer.version
                );
            }
            Ok(())
        }
        Some("snapshot") => {
            let scope = server::scope_from(&workspace_query(rest))?;
            let (snapshot, skipped) = capture::capture(scope, &tailnet::me()?.name, &home()?)?;
            for s in &skipped {
                eprintln!("skipped {:?}: {}", s.title, s.reason);
            }
            let json = serde_json::to_string_pretty(&snapshot)?;
            match flag(rest, "--out") {
                Some(path) => std::fs::write(path, json)?,
                None => println!("{json}"),
            }
            Ok(())
        }
        Some("send") => {
            let peer = rest.first().context("usage: omaspace send <peer>")?;
            let scope = server::scope_from(&workspace_query(rest))?;
            let (mut snapshot, skipped) =
                capture::capture_with(scope, &tailnet::me()?.name, &home()?, &allowed(rest)?)?;
            if let Some(n) = flag(rest, "--to-workspace") {
                snapshot.onto_workspace(
                    n.parse()
                        .context("--to-workspace takes a workspace number")?,
                );
            }
            if rest.iter().any(|a| a == "--with-files") {
                for line in files::sync(&files::folders(&snapshot), "local", peer)? {
                    println!("files     {line}");
                }
            }
            let report = client::restore_on(peer, &snapshot)?;
            client::print_report(&report, skipped.len());
            client::fail_if_partial(&report)
        }
        Some("pull") => {
            let peer = rest.first().context("usage: omaspace pull <peer>")?;
            let query = format!(
                "{}&allow={}",
                workspace_query(rest),
                allowed(rest)?.join(",")
            );
            let (snapshot, skipped) = client::snapshot_from(peer, &query)?;
            if rest.iter().any(|a| a == "--with-files") {
                for line in files::sync(&files::folders(&snapshot), peer, "local")? {
                    println!("files     {line}");
                }
            }
            let report = serde_json::to_value(restore::restore(&snapshot, &home()?)?)?;
            client::print_report(&report, skipped);
            client::fail_if_partial(&report)
        }
        Some("stash") => {
            let hub = rest.first().context("usage: omaspace stash <hub>")?;
            let scope = server::scope_from(&workspace_query(rest))?;
            let (snapshot, _) = capture::capture(scope, &tailnet::me()?.name, &home()?)?;
            println!("{}", client::stash_put(hub, &snapshot)?);
            Ok(())
        }
        Some("stashes") => {
            let hub = rest.first().context("usage: omaspace stashes <hub>")?;
            println!(
                "{}",
                serde_json::to_string_pretty(&client::stash_list(hub)?)?
            );
            Ok(())
        }
        Some("restore") => {
            let source = rest
                .first()
                .context("usage: omaspace restore <file|hub:id>")?;
            let snapshot = match source.split_once(':') {
                Some((hub, id)) if !std::path::Path::new(source).exists() => {
                    client::stash_get(hub, id)?
                }
                _ => serde_json::from_slice(&std::fs::read(source)?)?,
            };
            let report = serde_json::to_value(restore::restore(&snapshot, &home()?)?)?;
            client::print_report(&report, 0);
            client::fail_if_partial(&report)
        }
        Some("put") => cmd_put(rest),
        Some("get") => cmd_get(rest),
        Some("ls") => {
            let peer = rest.first().context("usage: omaspace ls <peer> [path]")?;
            let path = rest
                .get(1)
                .filter(|a| !a.starts_with("--"))
                .map(String::as_str)
                .unwrap_or("~/Downloads");
            let v = client::files_list(peer, path)?;
            // `--names`: just the files, one per line (for scripts and menus).
            if rest.iter().any(|a| a == "--names") {
                for e in v["entries"].as_array().into_iter().flatten() {
                    if e["dir"].as_bool() != Some(true) {
                        println!("{}", e["name"].as_str().unwrap_or(""));
                    }
                }
                return Ok(());
            }
            println!("{}", v["path"].as_str().unwrap_or(path));
            for e in v["entries"].as_array().into_iter().flatten() {
                let name = e["name"].as_str().unwrap_or("");
                if e["dir"].as_bool() == Some(true) {
                    println!("  {name}/");
                } else {
                    println!("  {name}  ({})", human(e["size"].as_u64().unwrap_or(0)));
                }
            }
            Ok(())
        }
        Some("sync") => sync::run(rest),
        // Undocumented: replay input through the virtual pointer/keyboard, for
        // the e2e suite (e.g. a SUPER-drag): `move X Y`, `button 272 down|up`,
        // `key 125 down|up`, `sleep MS`; X/Y are 0..1 of the primary monitor.
        Some("input") => cmd_input(rest),
        Some("spaces") => {
            println!("{}", serde_json::to_string(&client::spaces()?)?);
            Ok(())
        }
        Some("mcp") => mcp::run(),
        Some("view") => view::serve(),
        // Internal: started by open_browser to hold an agent browser's
        // DevTools pipe and relay it on a private socket.
        Some("cdp-broker") => {
            let socket = rest
                .first()
                .context("usage: omaspace cdp-broker <socket> -- <browser argv>")?;
            anyhow::ensure!(
                rest.get(1).map(String::as_str) == Some("--"),
                "usage: omaspace cdp-broker <socket> -- <browser argv>"
            );
            cdp::broker(std::path::Path::new(socket), &rest[2..])
        }
        Some("setup") => setup::run(&home()?),
        _ => {
            println!("{USAGE}");
            Ok(())
        }
    }
}

fn human(n: u64) -> String {
    let units = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut u = 0;
    while v >= 1024.0 && u < units.len() - 1 {
        v /= 1024.0;
        u += 1;
    }
    if u == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", units[u])
    }
}

/// Positional args up to the first `--flag`.
fn positional(args: &[String]) -> Vec<&String> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i].starts_with("--") {
            i += 2;
            continue;
        }
        out.push(&args[i]);
        i += 1;
    }
    out
}

fn progress_line(label: &str) -> impl FnMut(u64, u64) + '_ {
    let mut last = std::time::Instant::now() - std::time::Duration::from_secs(1);
    move |done, total| {
        if last.elapsed().as_millis() >= 200 || done == total {
            last = std::time::Instant::now();
            let pct = (done * 100).checked_div(total).unwrap_or(100);
            eprint!(
                "\r  {label}  {pct:>3}%  {} / {}   ",
                human(done),
                human(total)
            );
            if done == total {
                eprintln!();
            }
        }
    }
}

fn cmd_put(args: &[String]) -> anyhow::Result<()> {
    let pos = positional(args);
    let peer = pos
        .first()
        .context("usage: omaspace put <peer> <file|dir>... [--to ~/Downloads]")?;
    anyhow::ensure!(
        pos.len() >= 2,
        "usage: omaspace put <peer> <file|dir>... [--to ~/Downloads]"
    );
    let to = flag(args, "--to").unwrap_or_else(|| "~/Downloads".into());
    let to = to.trim_end_matches('/').to_string();
    let mut sent = 0;
    for src in &pos[1..] {
        let src = std::path::Path::new(src.as_str());
        let base = src.parent().unwrap_or(std::path::Path::new("."));
        let files: Vec<std::path::PathBuf> = if src.is_dir() {
            walk(src)?
        } else {
            vec![src.to_path_buf()]
        };
        for f in files {
            let rel = f
                .strip_prefix(base)
                .unwrap_or(&f)
                .to_string_lossy()
                .into_owned();
            let remote = format!("{to}/{rel}");
            let landed = client::put_file(peer, &f, &remote, false, &mut progress_line(&rel))
                .with_context(|| format!("sending {}", f.display()))?;
            println!("sent      {} -> {peer}:{landed}", f.display());
            sent += 1;
        }
    }
    println!("{sent} file(s) sent to {peer}");
    Ok(())
}

fn cmd_get(args: &[String]) -> anyhow::Result<()> {
    let pos = positional(args);
    let peer = pos
        .first()
        .context("usage: omaspace get <peer> <path>... [--to .]")?;
    anyhow::ensure!(
        pos.len() >= 2,
        "usage: omaspace get <peer> <path>... [--to .]"
    );
    let to = std::path::PathBuf::from(flag(args, "--to").unwrap_or_else(|| ".".into()));
    let mut got = 0;
    for remote in &pos[1..] {
        let stat = client::files_stat(peer, remote)?;
        anyhow::ensure!(
            stat["exists"].as_bool() == Some(true),
            "{remote} does not exist on {peer}"
        );
        let remotes: Vec<(String, std::path::PathBuf)> = if stat["dir"].as_bool() == Some(true) {
            let root = stat["path"].as_str().unwrap_or(remote).to_string();
            let name = xfer::plain_name(root.rsplit('/').next().unwrap_or("folder"))?.to_string();
            remote_walk(peer, &root)?
                .into_iter()
                .map(|rel| (format!("{root}/{rel}"), to.join(&name).join(&rel)))
                .collect()
        } else {
            let name = xfer::plain_name(remote.rsplit('/').next().unwrap_or("file"))?.to_string();
            vec![(remote.to_string(), to.join(name))]
        };
        for (r, local) in remotes {
            client::get_file(peer, &r, &local, &mut progress_line(&r))
                .with_context(|| format!("fetching {r}"))?;
            println!("received  {peer}:{r} -> {}", local.display());
            got += 1;
        }
    }
    println!("{got} file(s) received from {peer}");
    Ok(())
}

/// Every file under `dir`, skipping hidden entries.
pub fn walk(dir: &std::path::Path) -> anyhow::Result<Vec<std::path::PathBuf>> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d)?.flatten() {
            if e.file_name().to_string_lossy().starts_with('.') {
                continue;
            }
            let p = e.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
    out.sort();
    Ok(out)
}

/// Relative paths of every file under a remote folder.
pub fn remote_walk(peer: &str, root: &str) -> anyhow::Result<Vec<String>> {
    let mut out = Vec::new();
    let mut stack = vec![String::new()];
    while let Some(rel) = stack.pop() {
        let path = if rel.is_empty() {
            root.to_string()
        } else {
            format!("{root}/{rel}")
        };
        let v = client::files_list(peer, &path)?;
        for e in v["entries"].as_array().into_iter().flatten() {
            // The peer names entries; each must be a plain name in this folder,
            // or a local path built from it could point anywhere.
            let name = crate::xfer::plain_name(e["name"].as_str().unwrap_or(""))?;
            if name.starts_with('.') || name.ends_with(xfer::PART) {
                continue;
            }
            let child = if rel.is_empty() {
                name.to_string()
            } else {
                format!("{rel}/{name}")
            };
            if e["dir"].as_bool() == Some(true) {
                stack.push(child)
            } else {
                out.push(child)
            }
        }
    }
    out.sort();
    Ok(out)
}

fn cmd_input(args: &[String]) -> anyhow::Result<()> {
    let mon = hypr::primary_monitor_size()?;
    let tx = input::start(mon.0, mon.1, None)?;
    let mut it = args.iter().map(String::as_str);
    let num =
        |v: Option<&str>| -> anyhow::Result<f64> { Ok(v.context("missing number")?.parse()?) };
    let state = |v: Option<&str>| -> anyhow::Result<bool> {
        match v {
            Some("down") => Ok(true),
            Some("up") => Ok(false),
            other => anyhow::bail!("expected down|up, got {other:?}"),
        }
    };
    while let Some(word) = it.next() {
        let event = match word {
            "move" => input::Event::Move {
                x: num(it.next())?,
                y: num(it.next())?,
            },
            "button" => input::Event::Button {
                button: num(it.next())? as u32,
                pressed: state(it.next())?,
            },
            "key" => input::Event::Key {
                code: num(it.next())? as u32,
                pressed: state(it.next())?,
            },
            "sleep" => {
                std::thread::sleep(std::time::Duration::from_millis(num(it.next())? as u64));
                continue;
            }
            other => anyhow::bail!("unknown input word {other:?}"),
        };
        tx.send(event)?;
    }
    // Let the input thread flush the last events before the process exits.
    std::thread::sleep(std::time::Duration::from_millis(100));
    Ok(())
}
