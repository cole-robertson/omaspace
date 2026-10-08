//! File transfer between your machines (put / get / ls) and the path rules
//! both ends share. Uploads go to `<path>.omaspace-part` and are renamed into
//! place only after `commit` checks size and SHA-256.

use anyhow::Context;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Component, Path, PathBuf};

pub const CHUNK: usize = 8 * 1024 * 1024;
pub const PART: &str = ".omaspace-part";

/// Names refused at any depth: what a version-control tool, direnv, a build
/// tool, a version manager or an editor runs or loads on its own
/// (`.git/config` can set `core.fsmonitor` to a command, `.cargo/config.toml`
/// a build runner, `.nvim.lua` is executed on open), so a file put there
/// would run code later. Sync and `put` already skip dot-entries; this stops
/// a peer that asks for them directly.
const NEVER: &[&str] = &[
    ".git",
    ".hg",
    ".svn",
    ".jj",
    ".envrc",
    ".direnv",
    ".vscode",
    ".idea",
    ".zed",
    ".cargo",
    ".npmrc",
    ".yarnrc",
    ".yarnrc.yml",
    ".pnpmfile.cjs",
    ".mise.toml",
    "mise.toml",
    ".mise",
    ".tool-versions",
    ".nvim.lua",
    ".nvimrc",
    ".exrc",
    ".lazy.lua",
    ".helix",
    ".devcontainer",
    ".pre-commit-config.yaml",
];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Entry {
    pub name: String,
    pub size: u64,
    pub mtime: u64,
    pub dir: bool,
}

/// Resolve a client path (`~/x`, `x` = under home, or absolute) to a real
/// path inside `home`, refusing escapes and top-level dot-folders.
pub fn resolve(home: &Path, path: &str) -> anyhow::Result<PathBuf> {
    let rel = path
        .strip_prefix("~/")
        .or_else(|| path.strip_prefix('~'))
        .unwrap_or(path);
    let joined = if Path::new(rel).is_absolute() {
        PathBuf::from(rel)
    } else {
        home.join(rel)
    };
    let mut clean = PathBuf::new();
    for c in joined.components() {
        match c {
            Component::ParentDir => anyhow::bail!("'..' is not allowed in {path:?}"),
            Component::CurDir => {}
            other => clean.push(other),
        }
    }
    let inside = clean
        .strip_prefix(home)
        .map_err(|_| anyhow::anyhow!("{path:?} is outside your home folder"))?;
    check_allowed(inside, path)?;
    // Symlinks: the deepest existing part of the path (the target itself, if
    // it exists) must resolve to somewhere these same rules allow. Otherwise
    // `~/notes -> ~/.ssh` would let `~/notes/authorized_keys` through.
    let mut probe = clean.clone();
    while probe.symlink_metadata().is_err() {
        if !probe.pop() {
            break;
        }
    }
    if probe.symlink_metadata().is_ok() {
        let real = probe.canonicalize()?;
        let real_home = home.canonicalize()?;
        let inside = real
            .strip_prefix(&real_home)
            .map_err(|_| anyhow::anyhow!("{path:?} leads outside your home folder"))?;
        check_allowed(inside, path)?;
    }
    Ok(clean)
}

/// The rules for a path relative to home: no top-level dot-folder, and no
/// component a tool runs code from.
fn check_allowed(inside: &Path, path: &str) -> anyhow::Result<()> {
    if let Some(Component::Normal(first)) = inside.components().next() {
        anyhow::ensure!(
            !first.to_string_lossy().starts_with('.'),
            "{path:?} is inside a hidden folder"
        );
    }
    for c in inside.components() {
        let name = c.as_os_str().to_string_lossy();
        anyhow::ensure!(
            !NEVER.iter().any(|n| name.eq_ignore_ascii_case(n)),
            "{path:?} is inside {name}, which tools run code from"
        );
    }
    Ok(())
}

/// A single file or folder name from a peer (a directory listing entry):
/// it must name something *in* the folder, not a path out of it.
pub fn plain_name(name: &str) -> anyhow::Result<&str> {
    anyhow::ensure!(
        !name.is_empty()
            && name != "."
            && name != ".."
            && !name.contains('/')
            && !name.contains('\0'),
        "a peer sent an invalid file name {name:?}"
    );
    Ok(name)
}

/// Open `path` for writing without following a symlink at its last
/// component: a link planted where a part file goes can't redirect the
/// write somewhere else.
pub fn open_no_follow(path: &Path, truncate: bool) -> anyhow::Result<std::fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    const O_NOFOLLOW: i32 = 0o400000;
    Ok(std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(truncate)
        .custom_flags(O_NOFOLLOW)
        .open(path)?)
}

pub fn list(dir: &Path) -> anyhow::Result<Vec<Entry>> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .flatten()
    {
        let name = e.file_name().to_string_lossy().into_owned();
        let Ok(m) = e.metadata() else { continue };
        out.push(Entry {
            name,
            size: m.len(),
            mtime: m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map_or(0, |d| d.as_secs()),
            dir: m.is_dir(),
        });
    }
    out.sort_by(|a, b| {
        b.dir
            .cmp(&a.dir)
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

/// Write `body` at `offset` into the part file for `dest` (offset 0 starts over).
pub fn put_chunk(dest: &Path, offset: u64, body: &mut dyn Read) -> anyhow::Result<u64> {
    let part = part_path(dest);
    if let Some(parent) = part.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut f = open_no_follow(&part, offset == 0)?;
    let len = f.metadata()?.len();
    anyhow::ensure!(
        offset <= len,
        "offset {offset} is past what was received ({len})"
    );
    f.set_len(offset)?;
    f.seek(SeekFrom::Start(offset))?;
    let written = std::io::copy(&mut body.take(CHUNK as u64 + 1), &mut f)?;
    anyhow::ensure!(written <= CHUNK as u64, "chunk larger than {CHUNK} bytes");
    Ok(offset + written)
}

/// Size already received for `dest` (to resume an upload).
pub fn received(dest: &Path) -> u64 {
    std::fs::metadata(part_path(dest)).map_or(0, |m| m.len())
}

/// Check the part file and move it into place, never over an existing file:
/// returns the final path (`name (2).ext` on a clash unless `replace`).
pub fn commit(dest: &Path, size: u64, sha256: &str, replace: bool) -> anyhow::Result<PathBuf> {
    let part = part_path(dest);
    let meta = std::fs::symlink_metadata(&part).with_context(|| "nothing was received")?;
    anyhow::ensure!(meta.is_file(), "the received data is not a plain file");
    let got = meta.len();
    anyhow::ensure!(got == size, "received {got} bytes, expected {size}");
    let actual = hash_file(&part)?;
    if actual != sha256 {
        let _ = std::fs::remove_file(&part);
        anyhow::bail!("checksum mismatch; the transfer was discarded");
    }
    let final_path = if replace {
        anyhow::ensure!(
            !std::fs::symlink_metadata(dest).is_ok_and(|m| m.file_type().is_symlink()),
            "{} is a symbolic link; not replacing it",
            dest.display()
        );
        dest.to_path_buf()
    } else {
        free_name(dest)
    };
    std::fs::rename(&part, &final_path)?;
    Ok(final_path)
}

pub fn part_path(dest: &Path) -> PathBuf {
    let mut s = dest.as_os_str().to_owned();
    s.push(PART);
    PathBuf::from(s)
}

/// `report.pdf` → `report (2).pdf` if taken, like a browser download.
pub fn free_name(dest: &Path) -> PathBuf {
    let taken = |p: &Path| std::fs::symlink_metadata(p).is_ok();
    if !taken(dest) {
        return dest.to_path_buf();
    }
    let stem = dest
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = dest
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    (2..)
        .map(|n| dest.with_file_name(format!("{stem} ({n}){ext}")))
        .find(|p| !taken(p))
        .expect("some free name")
}

pub fn hash_file(path: &Path) -> anyhow::Result<String> {
    let mut f = std::fs::File::open(path)?;
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 20];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Copy `from` to `w`, starting at `offset` (resumed downloads).
pub fn stream_file(from: &Path, offset: u64, w: &mut dyn Write) -> anyhow::Result<u64> {
    let mut f = std::fs::File::open(from)?;
    f.seek(SeekFrom::Start(offset))?;
    Ok(std::io::copy(&mut f, w)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_symlink_cannot_lead_into_a_hidden_or_blocked_folder() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        std::fs::create_dir_all(h.join(".ssh")).unwrap();
        std::fs::create_dir_all(h.join("src/x/.git")).unwrap();
        std::os::unix::fs::symlink(h.join(".ssh"), h.join("notes")).unwrap();
        std::os::unix::fs::symlink(h.join("src/x/.git"), h.join("gitdir")).unwrap();
        std::os::unix::fs::symlink(h.join(".ssh/authorized_keys"), h.join("keys")).unwrap();
        assert!(resolve(h, "~/notes/authorized_keys").is_err());
        assert!(resolve(h, "~/gitdir/config").is_err());
        assert!(
            resolve(h, "~/keys").is_err(),
            "a link to a file that doesn't exist yet"
        );
        assert!(resolve(h, "~/src/x/.cargo/config.toml").is_err());
        assert!(resolve(h, "~/src/x/mise.toml").is_err());
    }

    #[test]
    fn a_part_file_symlink_is_not_written_through() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        let victim = h.join("victim");
        std::fs::write(&victim, "keep").unwrap();
        let dest = h.join("Downloads/a.txt");
        std::fs::create_dir_all(dest.parent().unwrap()).unwrap();
        std::os::unix::fs::symlink(&victim, part_path(&dest)).unwrap();
        assert!(put_chunk(&dest, 0, &mut &b"evil"[..]).is_err());
        assert_eq!(std::fs::read_to_string(&victim).unwrap(), "keep");
    }

    #[test]
    fn peer_file_names_cannot_climb_out() {
        assert!(plain_name("report.pdf").is_ok());
        assert!(plain_name("..").is_err());
        assert!(plain_name(".").is_err());
        assert!(plain_name("").is_err());
        assert!(plain_name("a/../../b").is_err());
        assert!(plain_name("x\0y").is_err());
    }

    #[test]
    fn paths_stay_inside_home_and_out_of_hidden_folders() {
        let home = tempfile::tempdir().unwrap();
        let h = home.path();
        assert_eq!(
            resolve(h, "~/Downloads/a.txt").unwrap(),
            h.join("Downloads/a.txt")
        );
        assert_eq!(resolve(h, "Downloads").unwrap(), h.join("Downloads"));
        assert!(resolve(h, "~/../etc/passwd").is_err());
        assert!(resolve(h, "/etc/passwd").is_err());
        assert!(resolve(h, "~/.ssh/authorized_keys").is_err());
        assert!(resolve(h, "~/.config/x").is_err());
        assert!(
            resolve(h, "~/src/notes/.todo").is_ok(),
            "other nested dot-files are fine"
        );
        assert!(
            resolve(h, "~/src/x/.git/config").is_err(),
            "core.fsmonitor in .git/config runs a command"
        );
        assert!(resolve(h, "~/src/x/.git/hooks/pre-commit").is_err());
        assert!(
            resolve(h, "~/src/x/.GIT/config").is_err(),
            "case-insensitive filesystems"
        );
        assert!(resolve(h, "~/src/x/.envrc").is_err());
        assert!(resolve(h, "~/src/x/.vscode/tasks.json").is_err());
        assert!(
            resolve(h, "~/src/x/.git").is_err(),
            "the folder itself (mkdir, delete)"
        );
        std::os::unix::fs::symlink("/etc", h.join("escape")).unwrap();
        assert!(
            resolve(h, "~/escape/passwd").is_err(),
            "a symlink out of home is refused"
        );
    }

    #[test]
    fn upload_only_appears_after_a_verified_commit() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path().join("a.bin");
        let data: Vec<u8> = (0..20_000u32).map(|i| (i % 251) as u8).collect();
        put_chunk(&dest, 0, &mut &data[..12_000]).unwrap();
        assert!(!dest.exists());
        assert_eq!(received(&dest), 12_000, "resume point");
        put_chunk(&dest, 12_000, &mut &data[12_000..]).unwrap();
        let sha = Sha256::digest(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        assert!(
            commit(&dest, 20_000, "00", false).is_err(),
            "bad checksum is refused"
        );
        put_chunk(&dest, 0, &mut &data[..]).unwrap();
        let landed = commit(&dest, 20_000, &sha, false).unwrap();
        assert_eq!(std::fs::read(&landed).unwrap(), data);
        put_chunk(&dest, 0, &mut &data[..]).unwrap();
        assert_eq!(
            commit(&dest, 20_000, &sha, false).unwrap(),
            dir.path().join("a (2).bin"),
            "no overwrite"
        );
    }
}
