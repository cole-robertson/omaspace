//! Tailscale identity via tailscaled's LocalAPI socket. Every request to the
//! daemon is identified by the caller's tailnet address, and only your own
//! devices are trusted (see SPEC.md "Peers and trust").

use anyhow::Context;
use serde::Deserialize;
use std::io::{Read, Write};
use std::os::unix::net::UnixStream;

const SOCKET: &str = "/run/tailscale/tailscaled.sock";

#[derive(Debug, Clone, PartialEq)]
pub struct Identity {
    /// The node's stable ID (empty when unknown, e.g. in tests).
    pub node_id: String,
    pub name: String,
    pub user_id: i64,
    pub tags: Vec<String>,
    /// The owning person's Tailscale login (empty for tagged devices).
    pub login: String,
}

#[derive(Deserialize)]
struct WhoIs {
    #[serde(rename = "Node")]
    node: Node,
    #[serde(rename = "UserProfile", default)]
    profile: Option<Profile>,
}

#[derive(Deserialize)]
struct Profile {
    #[serde(rename = "LoginName", default)]
    login: String,
}

#[derive(Deserialize)]
struct Node {
    #[serde(rename = "StableID", default)]
    stable_id: String,
    #[serde(rename = "ComputedName", default)]
    name: String,
    #[serde(rename = "User", default)]
    user: i64,
    #[serde(rename = "Tags", default)]
    tags: Option<Vec<String>>,
}

#[derive(Deserialize)]
struct Status {
    #[serde(rename = "Self")]
    me: Peer,
    #[serde(rename = "Peer", default)]
    peers: Option<std::collections::HashMap<String, Peer>>,
    #[serde(rename = "User", default)]
    users: Option<std::collections::HashMap<String, Profile>>,
    #[serde(rename = "MagicDNSSuffix", default)]
    magic_dns_suffix: String,
}

#[derive(Deserialize, Clone)]
pub struct Peer {
    #[serde(rename = "ID", default)]
    pub id: String,
    #[serde(rename = "HostName")]
    pub host: String,
    #[serde(rename = "UserID")]
    pub user_id: i64,
    #[serde(rename = "Tags", default)]
    pub tags: Option<Vec<String>>,
    #[serde(rename = "TailscaleIPs", default)]
    pub ips: Vec<String>,
    #[serde(rename = "Online", default)]
    pub online: bool,
    /// MagicDNS name, e.g. `alpha.tail1234.ts.net.`.
    #[serde(rename = "DNSName", default)]
    pub dns_name: String,
    #[serde(rename = "OS", default)]
    pub os: String,
    /// Owner login, filled from the status `User` table.
    #[serde(skip)]
    pub login: String,
}

fn local_api(path: &str) -> anyhow::Result<String> {
    let mut stream = UnixStream::connect(SOCKET).context("connecting to tailscaled")?;
    stream.set_read_timeout(Some(std::time::Duration::from_secs(5)))?;
    write!(
        stream,
        "GET /localapi/v0/{path} HTTP/1.0\r\nHost: local-tailscaled.sock\r\n\r\n"
    )?;
    let mut response = String::new();
    stream.read_to_string(&mut response)?;
    let (head, body) = response
        .split_once("\r\n\r\n")
        .context("malformed LocalAPI response")?;
    anyhow::ensure!(
        head.starts_with("HTTP/1.0 200") || head.starts_with("HTTP/1.1 200"),
        "LocalAPI {path}: {head}"
    );
    Ok(body.to_string())
}

pub fn whois(addr: &str) -> anyhow::Result<Identity> {
    let w: WhoIs = serde_json::from_str(&local_api(&format!("whois?addr={addr}"))?)?;
    let tags = w.node.tags.unwrap_or_default();
    // Tagged devices report the placeholder "tagged-devices" as their login.
    let login = if tags.is_empty() {
        w.profile.map(|p| p.login).unwrap_or_default()
    } else {
        String::new()
    };
    Ok(Identity {
        node_id: w.node.stable_id,
        name: w.node.name,
        user_id: w.node.user,
        tags,
        login,
    })
}

pub fn me() -> anyhow::Result<Identity> {
    let s: Status = serde_json::from_str(&local_api("status")?)?;
    Ok(Identity {
        node_id: s.me.id.clone(),
        name: s.me.host,
        user_id: s.me.user_id,
        tags: s.me.tags.unwrap_or_default(),
        login: String::new(),
    })
}

/// The tailnet's DNS name, e.g. `tail1234.ts.net`.
pub fn magic_dns_suffix() -> anyhow::Result<String> {
    let s: Status = serde_json::from_str(&local_api("status")?)?;
    anyhow::ensure!(
        !s.magic_dns_suffix.is_empty(),
        "MagicDNS is off on this tailnet"
    );
    Ok(s.magic_dns_suffix)
}

/// This machine's MagicDNS name, e.g. `alpha.tail1234.ts.net.`.
pub fn my_dns_name() -> anyhow::Result<String> {
    let s: Status = serde_json::from_str(&local_api("status")?)?;
    anyhow::ensure!(!s.me.dns_name.is_empty(), "MagicDNS is off on this tailnet");
    Ok(s.me.dns_name)
}

pub fn my_ipv4() -> anyhow::Result<String> {
    let s: Status = serde_json::from_str(&local_api("status")?)?;
    s.me.ips
        .into_iter()
        .find(|ip| ip.contains('.'))
        .context("no Tailscale IPv4 address")
}

pub fn peers() -> anyhow::Result<Vec<Peer>> {
    let s: Status = serde_json::from_str(&local_api("status")?)?;
    let users = s.users.unwrap_or_default();
    Ok(s.peers
        .unwrap_or_default()
        .into_values()
        .map(|mut p| {
            if p.tags.as_ref().is_none_or(|t| t.is_empty()) {
                p.login = users
                    .get(&p.user_id.to_string())
                    .map(|u| u.login.clone())
                    .unwrap_or_default();
            }
            p
        })
        .collect())
}

/// Same owner: on a user-owned machine the caller must belong to the same
/// Tailscale user; on a tagged machine it must share one of this machine's tags.
pub fn trusted(me: &Identity, caller: &Identity) -> bool {
    !is_self(me, caller) && trusted_with(me, caller, &owners())
}

/// A caller on this very machine. Whois maps a connection from here to this
/// machine's own tailnet address back to this node, which would pass the
/// same-user rule; but on this machine any process can make one (another
/// Unix user, a container, a sandboxed app), so it is never trusted. Your
/// own tools reach this machine's desktop directly, not over the network.
pub fn is_self(me: &Identity, caller: &Identity) -> bool {
    !me.node_id.is_empty() && me.node_id == caller.node_id
}

/// The trust rule, with the owners list passed in (tests):
/// - a caller sharing one of this machine's tags (your tagged machines);
/// - on a user-owned machine, a device of the same user;
/// - a device owned by a login in `~/.config/omaspace/owners`, so a tagged
///   machine can trust its owner's phone and laptops. Other people on the
///   same tailnet are not trusted unless listed.
pub fn trusted_with(me: &Identity, caller: &Identity, owners: &[String]) -> bool {
    if !me.tags.is_empty() && caller.tags.iter().any(|t| me.tags.contains(t)) {
        return true;
    }
    if me.tags.is_empty() && caller.tags.is_empty() && caller.user_id == me.user_id {
        return true;
    }
    caller.tags.is_empty()
        && !caller.login.is_empty()
        && owners.iter().any(|o| o.eq_ignore_ascii_case(&caller.login))
}

/// Logins (one per line, `#` comments) trusted as this machine's owners.
pub fn owners() -> Vec<String> {
    let path = std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config")))
        .map(|d| d.join("omaspace/owners"));
    path.and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default()
        .lines()
        .map(|l| l.split('#').next().unwrap_or("").trim().to_string())
        .filter(|l| l.contains('@'))
        .collect()
}

pub fn peer_trusted(me: &Identity, peer: &Peer) -> bool {
    trusted(
        me,
        &Identity {
            node_id: peer.id.clone(),
            name: peer.host.clone(),
            user_id: peer.user_id,
            tags: peer.tags.clone().unwrap_or_default(),
            login: peer.login.clone(),
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A device of `user`; each call is a different node.
    fn id(user: i64, tags: &[&str]) -> Identity {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(1);
        Identity {
            node_id: format!(
                "n{}",
                NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ),
            name: "n".into(),
            user_id: user,
            tags: tags.iter().map(|t| t.to_string()).collect(),
            login: String::new(),
        }
    }

    #[test]
    fn tagged_machines_trust_only_callers_sharing_a_tag() {
        let me = id(1, &["tag:desktop"]);
        assert!(trusted(&me, &id(2, &["tag:desktop"])));
        assert!(
            !trusted(&me, &id(1, &[])),
            "an untagged device of another owner is not a peer"
        );
        assert!(!trusted(&me, &id(3, &["tag:server"])));
    }

    #[test]
    fn tagged_machines_trust_their_listed_owners_devices_only() {
        let me = id(1, &["tag:desktop"]);
        let phone = Identity {
            node_id: "nphone".into(),
            name: "iphone".into(),
            user_id: 9,
            tags: vec![],
            login: "owner@example.com".into(),
        };
        let colleague = Identity {
            node_id: "nmac".into(),
            name: "mac".into(),
            user_id: 8,
            tags: vec![],
            login: "colleague@example.com".into(),
        };
        let owners = vec!["owner@example.com".to_string()];
        assert!(trusted_with(&me, &phone, &owners));
        assert!(
            !trusted_with(&me, &colleague, &owners),
            "another person on the tailnet"
        );
        assert!(
            !trusted_with(&me, &phone, &[]),
            "no owners file: only tag-mates"
        );
    }

    #[test]
    fn user_owned_machines_trust_only_the_same_user() {
        let me = id(7, &[]);
        assert!(trusted(&me, &id(7, &[])));
        assert!(!trusted(&me, &id(8, &[])));
        assert!(
            !trusted(&me, &id(7, &["tag:shared"])),
            "a tagged device no longer belongs to the user"
        );
    }

    /// A connection from this machine to its own tailnet address is
    /// identified as this node: any local process (another Unix user, a
    /// container) could make one, so it must not count as your device.
    #[test]
    fn this_machine_itself_is_never_a_trusted_caller() {
        let me = id(7, &[]);
        let same_node = Identity {
            name: "itself".into(),
            ..me.clone()
        };
        assert!(!trusted(&me, &same_node));
        assert!(
            trusted(&me, &id(7, &[])),
            "another device of yours still is"
        );
        let tagged = id(1, &["tag:desktop"]);
        assert!(!trusted(&tagged, &tagged.clone()));
    }
}
