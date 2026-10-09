// put / get / resume / sync between machines, and the path safety rules.

import { test, expect, waitFor } from "../lib/fixtures.js";

test("put and get: a large file and a folder arrive intact both ways", async ({ peer, desk }) => {
  const src = peer.scratch("src");
  peer.randomFile(`${src}/big.bin`, 40_000_000);
  peer.write(`${src}/folder/a.txt`, "one");
  peer.write(`${src}/folder/sub/b.txt`, "two");
  const dest = desk.scratch("dest");
  const remoteDest = dest.replace(/^\/home\/[^/]+/, "~");

  peer.omaspace(["put", desk.name, `${src}/big.bin`, `${src}/folder`, "--to", remoteDest]);
  expect(desk.sha256(`${dest}/big.bin`)).toBe(peer.sha256(`${src}/big.bin`));
  expect(desk.read(`${dest}/folder/sub/b.txt`)).toBe("two");

  const back = peer.scratch("back");
  peer.omaspace(["get", desk.name, `${remoteDest}/big.bin`, `${remoteDest}/folder`, "--to", back]);
  expect(peer.sha256(`${back}/big.bin`)).toBe(peer.sha256(`${src}/big.bin`));
  expect(peer.read(`${back}/folder/a.txt`)).toBe("one");
});

test("an interrupted upload resumes, and only appears once verified", async ({ peer, desk }) => {
  const src = peer.scratch("resume");
  peer.randomFile(`${src}/f.bin`, 20_000_000);
  const dest = desk.scratch("resume");
  const remote = `${dest.replace(/^\/home\/[^/]+/, "~")}/f.bin`;
  // First 8 MiB only, as if the connection dropped.
  peer.sh(`head -c 8388608 ${src}/f.bin | curl -sf -X PUT --data-binary @- "http://${desk.ip()}:7787/v1/files/put?path=${encodeURIComponent(remote)}&offset=0" >/dev/null`);
  expect(desk.exists(`${dest}/f.bin`), "partial file must not appear under its name").toBe(false);

  const { out } = peer.omaspace(["put", desk.name, `${src}/f.bin`, "--to", remote.replace(/\/f\.bin$/, "")]);
  expect(out).toMatch(/1 file\(s\) sent/);
  expect(desk.sha256(`${dest}/f.bin`)).toBe(peer.sha256(`${src}/f.bin`));
  expect(desk.sh(`ls ${dest} | grep -c omaspace-part || true`)).toBe("0");
});

test("sync: edits, new files, deletions and conflicts in both directions", async ({ peer, desk }) => {
  const dir = peer.scratch("sync");
  const rdir = dir; // same path on both (same user, same home layout)
  desk.onCleanup(() => desk.rm(rdir));
  peer.onCleanup(() => peer.sh("rm -rf ~/.local/state/omaspace/sync"));
  peer.write(`${dir}/notes.md`, "v1");
  peer.write(`${dir}/shared.txt`, "base");
  peer.write(`${dir}/doomed.txt`, "x");
  peer.omaspace(["sync", desk.name, dir, "--once"]);
  expect(desk.read(`${rdir}/shared.txt`)).toBe("base");

  peer.write(`${dir}/notes.md`, "v2 from peer");
  peer.write(`${dir}/shared.txt`, "peer's");
  desk.write(`${rdir}/from-desk.txt`, "made on desk");
  desk.rm(`${rdir}/doomed.txt`);
  desk.write(`${rdir}/shared.txt`, "desk's");
  const { out } = peer.omaspace(["sync", desk.name, dir, "--once"]);
  expect(out).toMatch(/changed on both sides/);

  for (const m of [peer, desk]) {
    expect(m.read(`${dir}/notes.md`)).toBe("v2 from peer");
    expect(m.read(`${dir}/from-desk.txt`)).toBe("made on desk");
    expect(m.exists(`${dir}/doomed.txt`)).toBe(false);
    expect(m.read(`${dir}/shared.txt`)).toBe("peer's");
    expect(m.read(`${dir}/shared (conflict from ${desk.name}).txt`)).toBe("desk's");
  }
  expect(peer.omaspace(["sync", desk.name, dir, "--once"]).out).toMatch(/^0 change\(s\)$/m);
});

test("paths outside home or in hidden folders are refused", async ({ peer, desk }) => {
  const src = peer.scratch("refuse");
  peer.write(`${src}/a.txt`, "x");
  expect(peer.omaspace(["get", desk.name, "~/.ssh/authorized_keys", "--to", src], { check: false }).out).toMatch(/hidden folder/);
  expect(peer.omaspace(["put", desk.name, `${src}/a.txt`, "--to", "~/../../etc"], { check: false }).out).toMatch(/'\.\.' is not allowed/);
  expect(peer.omaspace(["put", desk.name, `${src}/a.txt`, "--to", "/etc"], { check: false }).out).toMatch(/outside your home/);
});

test("a folder added with `sync add` keeps syncing in the background, also after the service restarts", async ({ peer, desk }) => {
  const dir = peer.scratch("kept");
  desk.onCleanup(() => desk.rm(dir));
  peer.onCleanup(() => peer.omaspace(["sync", "remove", dir], { check: false }));
  peer.write(`${dir}/first.txt`, "one");
  const { out } = peer.omaspace(["sync", "add", desk.name, dir]);
  expect(out).toMatch(/keeps syncing in the background/);
  expect(desk.read(`${dir}/first.txt`)).toBe("one");
  expect(peer.omaspace(["sync", "list"]).out).toContain(desk.name);

  // No command running now: the daemon carries it, both ways.
  peer.write(`${dir}/later.txt`, "from peer");
  await waitFor(() => desk.exists(`${dir}/later.txt`), "peer's new file reaches desk", 20_000);
  desk.write(`${dir}/back.txt`, "from desk");
  await waitFor(() => peer.exists(`${dir}/back.txt`), "desk's new file reaches peer", 20_000);

  // A restart (or a reboot) doesn't stop it.
  peer.sh("systemctl --user restart omaspace");
  peer.write(`${dir}/after-restart.txt`, "still syncing");
  await waitFor(() => desk.exists(`${dir}/after-restart.txt`), "syncs after a restart", 30_000);
  expect(peer.omaspace(["sync", "list"]).out).toMatch(/synced/);

  peer.omaspace(["sync", "remove", dir]);
  peer.write(`${dir}/after-remove.txt`, "x");
  await new Promise(r => setTimeout(r, 8000));
  expect(desk.exists(`${dir}/after-remove.txt`), "removed: no longer synced").toBe(false);
  expect(desk.read(`${dir}/first.txt`), "removing keeps the files").toBe("one");
});

test("a symlink can't smuggle a file into a hidden folder", async ({ peer, desk }) => {
  const src = peer.scratch("smuggle");
  peer.write(`${src}/a.txt`, "x");
  const link = desk.scratch("innocent");
  desk.sh(`rmdir ${link} && ln -s ~/.ssh ${link}`);
  desk.onCleanup(() => desk.sh(`rm -f ${link}`));
  const remote = link.replace(/^\/home\/[^/]+/, "~");
  expect(peer.omaspace(["put", desk.name, `${src}/a.txt`, "--to", remote], { check: false }).out).toMatch(/hidden folder/);
});
