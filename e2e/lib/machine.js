// Drive one Omarchy machine for end-to-end tests: run commands (locally or over
// Tailscale SSH), read Hyprland state, open and close windows, run omaspace,
// read and write files. Everything a test creates is tracked and cleaned up.

import { execFileSync, spawnSync } from "node:child_process";
import { hostname } from "node:os";

const HYPR_ENV = `export XDG_RUNTIME_DIR=/run/user/$(id -u) WAYLAND_DISPLAY=wayland-1 HYPRLAND_INSTANCE_SIGNATURE=$(for s in $(ls -t /run/user/$(id -u)/hypr/); do HYPRLAND_INSTANCE_SIGNATURE=$s timeout 2 hyprctl -j version >/dev/null 2>&1 && { echo $s; break; }; done) OMARCHY_PATH=$(test -d ~/.local/share/omarchy && readlink -f ~/.local/share/omarchy || echo /usr/share/omarchy) PATH=$HOME/.local/bin:$PATH CUA_TELEMETRY=0`;

export class Machine {
  /** @param {string} name tailnet hostname, e.g. "peer" */
  constructor(name) {
    this.name = name;
    this.local = name === hostname();
    this.cleanups = [];
    this.marker = `osp-e2e-${process.pid}-${Math.random().toString(36).slice(2, 8)}`;
  }

  /** Run a bash script on the machine (with the Hyprland session env); return stdout. */
  sh(script, { check = true, timeout = 120_000 } = {}) {
    const full = `${HYPR_ENV}\n${script}`;
    const r = this.local
      ? spawnSync("bash", ["-c", full], { encoding: "utf8", timeout })
      : spawnSync("ssh", ["-o", "BatchMode=yes", this.name, "bash -s"], { input: full, encoding: "utf8", timeout });
    if (check && r.status !== 0) {
      throw new Error(`[${this.name}] exit ${r.status}: ${script.slice(0, 200)}\n${r.stderr || ""}${r.stdout || ""}`);
    }
    return (r.stdout || "").trim();
  }

  json(script) {
    return JSON.parse(this.sh(script) || "null");
  }

  /** Register cleanup to run after the test, newest first. */
  onCleanup(fn) {
    this.cleanups.push(fn);
  }

  async cleanup() {
    for (const fn of this.cleanups.reverse()) {
      try { await fn(); } catch (e) { console.warn(`[${this.name}] cleanup: ${e.message}`); }
    }
    this.cleanups = [];
  }

  // ---- Hyprland --------------------------------------------------------------

  clients() { return this.json("hyprctl -j clients"); }
  /** Every monitor that's on, phone screens included. Disabled ones are
   *  leftovers Hyprland can't remove until it restarts: ignored. */
  monitors() { return this.json("hyprctl -j monitors all").filter(m => !m.disabled); }
  activeWorkspace() { return this.json("hyprctl -j activeworkspace").id; }
  windowsOn(ws) { return this.clients().filter(c => c.workspace.id === ws); }
  dispatch(lua) { return this.sh(`hyprctl dispatch ${shq(lua)}`); }
  focusWorkspace(ws) { this.dispatch(`hl.dsp.focus({ workspace = "${ws}" })`); }

  /** Close every window on a workspace (test hygiene). */
  clearWorkspace(ws) {
    for (const c of this.windowsOn(ws)) this.dispatch(`hl.dsp.window.close({ window = "address:${c.address}" })`);
    return waitFor(() => this.windowsOn(ws).length === 0, `${this.name} ws${ws} empty`, 8000);
  }

  /**
   * Launch a command on a workspace without taking focus and wait for its
   * window. The window is closed after the test.
   */
  async open(cmd, ws, { match = () => true, timeout = 15000 } = {}) {
    const before = new Set(this.clients().map(c => c.address));
    this.dispatch(`hl.dsp.exec_cmd(${luaq(cmd)}, { workspace = "${ws} silent" })`);
    const win = await waitFor(() => this.clients().find(c => !before.has(c.address) && match(c)), `${this.name}: window for ${cmd}`, timeout);
    if (win.workspace.id !== ws) this.dispatch(`hl.dsp.window.move({ workspace = "${ws}", follow = false, window = "address:${win.address}" })`);
    this.onCleanup(() => this.dispatch(`hl.dsp.window.close({ window = "address:${win.address}" })`));
    return win;
  }

  /** Open the Omarchy terminal in a directory on a workspace. */
  terminal(dir, ws) {
    return this.open(`uwsm-app -- xdg-terminal-exec --dir=${dir}`, ws, { match: c => /foot|terminal|alacritty|ghostty|kitty/i.test(c.class) });
  }

  /** Open Chromium (Omarchy's default profile) at URLs on a workspace. */
  browser(urls, ws) {
    return this.open(`uwsm-app -- chromium --no-first-run --new-window ${urls.join(" ")}`, ws, { match: c => c.class === "chromium" });
  }

  /** The working directory of the shell inside a terminal window. */
  terminalCwd(win) {
    return this.sh(`for c in $(cat /proc/${win.pid}/task/*/children); do readlink /proc/$c/cwd; done | head -1`);
  }

  /** Type a line into a terminal window's shell (via its pty; no focus needed). */
  typeInTerminal(win, line) {
    this.sh(`SH=$(cat /proc/${win.pid}/task/*/children | awk '{print $1}'); printf '%s\\n' ${shq(line)} > /proc/$SH/fd/0`);
  }

  // ---- files -----------------------------------------------------------------

  /** A scratch directory under ~ (removed after the test). */
  scratch(name = "") {
    const dir = `$HOME/${this.marker}${name ? "-" + name : ""}`;
    const real = this.sh(`mkdir -p ${dir} && cd ${dir} && pwd`);
    this.onCleanup(() => this.sh(`rm -rf ${shq(real)}`));
    return real;
  }

  write(path, content) { this.sh(`mkdir -p "$(dirname ${qpath(path)})" && printf '%s' ${shq(content)} > ${qpath(path)}`); }
  read(path) { return this.sh(`cat ${qpath(path)}`); }
  exists(path) { return this.sh(`test -e ${qpath(path)} && echo yes || echo no`) === "yes"; }
  sha256(path) { return this.sh(`sha256sum ${qpath(path)} | cut -d' ' -f1`); }
  randomFile(path, bytes) { this.sh(`mkdir -p "$(dirname ${qpath(path)})" && head -c ${bytes} /dev/urandom > ${qpath(path)}`); }
  rm(path) { this.sh(`rm -rf ${qpath(path)}`); }

  // ---- omaspace --------------------------------------------------------------

  /** Run `omaspace <args>` on this machine; returns { out, code }. */
  omaspace(args, { check = true, timeout = 180_000 } = {}) {
    // stdout and stderr both: `out` is what a person sees, `stdout` is for parsing.
    const tmp = `/tmp/omaspace-e2e-${process.pid}`;
    const script = `omaspace ${args.map(shq).join(" ")} >${tmp}.o 2>${tmp}.e; echo "__exit=$?"; cat ${tmp}.o; echo __err; cat ${tmp}.e; rm -f ${tmp}.o ${tmp}.e`;
    const raw = this.sh(script, { check: false, timeout });
    const m = raw.match(/^__exit=(\d+)\n?/);
    const code = m ? Number(m[1]) : 1;
    const [stdout, err = ""] = raw.slice(m ? m[0].length : 0).split(/^__err$/m).map(t => t.trim());
    const out = [stdout, err].filter(Boolean).join("\n");
    if (check && code !== 0) throw new Error(`[${this.name}] omaspace ${args.join(" ")} failed (${code}):\n${out}`);
    return { out, stdout, err, code };
  }

  /** Real input through omaspace's virtual pointer/keyboard, e.g.
   *  input("key 125 down", "move 0.5 0.01", "button 272 up"). X/Y are 0..1. */
  input(...words) { this.sh(`omaspace input ${words.join(" ")}`); }

  /** Press a key combo by evdev codes, e.g. keys(125, 29, 42, 24) = SUPER+CTRL+SHIFT+O. */
  keys(...codes) {
    const down = codes.map(c => `key ${c} down sleep 20`), up = [...codes].reverse().map(c => `key ${c} up sleep 20`);
    this.input(...down, ...up);
  }

  /** Call the Spaces panel (omaspace.spaces shell plugin). */
  spaces(method, arg = "") {
    return this.sh(`omarchy-shell shell call omaspace.spaces ${method} ${shq(arg)}`);
  }
  spacesState() { return JSON.parse(this.spaces("debugState")); }

  /** Pixel size of the primary monitor. */
  screen() { const m = this.monitors().find(m => !m.name.startsWith("OSP-")); return [m.width, m.height]; }

  /** Call one omaspace MCP tool on this machine (a fresh stdio server). */
  mcp(tool, args) {
    const lines = [
      { jsonrpc: "2.0", id: 1, method: "initialize", params: {} },
      { jsonrpc: "2.0", id: 2, method: "tools/call", params: { name: tool, arguments: args } },
    ].map(m => JSON.stringify(m)).join("\n");
    const out = this.sh(`printf '%s\\n' ${shq(lines)} | timeout 120 omaspace mcp | tail -1`, { timeout: 150_000 });
    const r = JSON.parse(out).result;
    if (r.isError) throw new Error(`${tool}: ${r.content[0].text}`);
    // The text is the tool's own value; structuredContent wraps a list in
    // { items } (MCP requires an object there).
    return JSON.parse(r.content[0].text);
  }

  /** Tailscale IPv4 of this machine. */
  ip() { return this.sh("tailscale ip -4 | head -1"); }

  /** Refuse a locked machine (the tests would type into its lock screen and
   *  count as failed unlocks), and keep its screen on and awake for the run:
   *  a screen that's off gives the live view nothing to stream. */
  ready() {
    if (this._ready) return;
    const locked = this.sh("omarchy-shell lock isLocked 2>/dev/null || echo false", { check: false }).trim();
    if (locked === "true") throw new Error(`${this.name} is locked: unlock it before running the tests`);
    this.sh(`omarchy-toggle-idle --status | grep -q '"enabled":true' || omarchy-toggle-idle stay-awake; hyprctl dispatch 'hl.dsp.dpms({ action = "on" })' >/dev/null`, { check: false });
    this._ready = true;
  }

  /** HTTPS base of this machine's live view (tailscale serve :7788). */
  viewUrl() { return `https://${this.name}.${tailnetDomain()}:7788`; }

  /** The Omarchy shell's notification history summaries (newest first). */
  notifications() {
    return this.sh(`omarchy-shell shell listNotifications 2>/dev/null || true`);
  }
}

let domain;
export function tailnetDomain() {
  return (domain ??= execFileSync("tailscale", ["status", "--json"], { encoding: "utf8" }).match(/"MagicDNSSuffix":\s*"([^"]+)"/)[1]);
}

/** Poll until `fn` returns a truthy value (or throw with `what`). */
export async function waitFor(fn, what, timeout = 10000, every = 250) {
  const end = Date.now() + timeout;
  let last;
  while (Date.now() < end) {
    try { const v = await fn(); if (v) return v; } catch (e) { last = e; }
    await new Promise(r => setTimeout(r, every));
  }
  throw new Error(`timed out waiting for ${what}${last ? ` (last error: ${last.message})` : ""}`);
}

/** Quote a path but let a leading ~ or $HOME expand. */
export function qpath(p) {
  const m = String(p).match(/^(~|\$HOME)(\/.*)?$/);
  return m ? `"$HOME"${m[2] ? shq(m[2]) : ""}` : shq(p);
}

export function shq(s) { return `'${String(s).replace(/'/g, `'\\''`)}'`; }
function luaq(s) { return JSON.stringify(String(s)); }
