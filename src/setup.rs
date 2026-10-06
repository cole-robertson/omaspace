//! `omaspace setup`: install the user service and Omarchy keybindings.
//! Idempotent; re-running replaces only what omaspace owns.

use std::path::Path;
use std::process::Command;

const UNIT: &str = include_str!("../dist/omaspace.service");
const VIEW_UNIT: &str = include_str!("../dist/omaspace-view.service");
const LAUNCHER: &str = include_str!("../dist/omarchy-omaspace");
const DRAG: &str = include_str!("../dist/omaspace-spaces-drag");
const PLUGIN_ID: &str = "omaspace.spaces";
const PLUGIN_FILES: [(&str, &str); 2] = [
    (
        "manifest.json",
        include_str!("../dist/spaces-plugin/manifest.json"),
    ),
    (
        "Spaces.qml",
        include_str!("../dist/spaces-plugin/Spaces.qml"),
    ),
];
const BINDINGS_MARK: &str = "-- omaspace (managed by `omaspace setup`)";
const BINDINGS: &str = r#"-- omaspace (managed by `omaspace setup`)
o.bind("SUPER + CTRL + SHIFT + O", "Spaces (give, take back, watch your machines)", "omarchy-omaspace")
o.bind("SUPER + CTRL + ALT + O", "Give this workspace to another machine", "omarchy-omaspace send")
-- Drag a window to the top edge to give it to another machine. Non-consuming,
-- so Omarchy's own SUPER-drag still moves the window.
hl.bind("SUPER + mouse:272", hl.dsp.exec_cmd("omaspace-spaces-drag start"), { non_consuming = true })
hl.bind("SUPER + mouse:272", hl.dsp.exec_cmd("omaspace-spaces-drag end"), { release = true, non_consuming = true })
-- Agents' browsers work on their own workspace and must never take your
-- focus (Omarchy focuses any window that asks for attention).
hl.window_rule({ match = { class = "^(omaspace-agent)$" }, suppress_event = "activate activatefocus" })
hl.window_rule({ match = { class = "^(omaspace-agent-term)$" }, suppress_event = "activate activatefocus" })
"#;

pub fn run(home: &Path) -> anyhow::Result<()> {
    let bin = home.join(".local/bin");
    std::fs::create_dir_all(&bin)?;
    let launcher = bin.join("omarchy-omaspace");
    std::fs::write(&launcher, LAUNCHER)?;
    std::fs::set_permissions(
        &launcher,
        std::os::unix::fs::PermissionsExt::from_mode(0o755),
    )?;
    println!("installed {}", launcher.display());
    let drag = bin.join("omaspace-spaces-drag");
    std::fs::write(&drag, DRAG)?;
    std::fs::set_permissions(&drag, std::os::unix::fs::PermissionsExt::from_mode(0o755))?;
    println!("installed {}", drag.display());

    // The Spaces panel: an Omarchy shell plugin, enabled through Omarchy's own
    // plugin command so it lands in shell.json the way any plugin does.
    let plugin = home.join(".config/omarchy/plugins").join(PLUGIN_ID);
    std::fs::create_dir_all(&plugin)?;
    for (name, text) in PLUGIN_FILES {
        std::fs::write(plugin.join(name), text)?;
    }
    // The shell rescans asynchronously; enabling only works once it knows
    // the plugin, so wait (up to 5s) for it to show up in listPlugins.
    let _ = Command::new("omarchy-shell")
        .args(["-q", "shell", "rescanPlugins"])
        .status();
    for _ in 0..50 {
        let listed = Command::new("omarchy-shell")
            .args(["shell", "listPlugins"])
            .output();
        if listed
            .is_ok_and(|o| String::from_utf8_lossy(&o.stdout).contains(&format!("\"{PLUGIN_ID}\"")))
        {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    match Command::new("omarchy-plugin-enable")
        .arg(PLUGIN_ID)
        .status()
    {
        Ok(s) if s.success() => println!(
            "installed and enabled the Spaces panel ({})",
            plugin.display()
        ),
        _ => println!(
            "installed the Spaces panel in {} but `omarchy-plugin-enable {PLUGIN_ID}` failed: enable it by hand",
            plugin.display()
        ),
    }

    let unit = home.join(".config/systemd/user/omaspace.service");
    std::fs::create_dir_all(unit.parent().unwrap())?;
    std::fs::write(&unit, UNIT)?;
    run_cmd("systemctl", &["--user", "daemon-reload"])?;
    run_cmd("systemctl", &["--user", "enable", "omaspace"])?;
    run_cmd("systemctl", &["--user", "restart", "omaspace"])?;
    println!("installed and started {}", unit.display());

    // The live view, so this machine can be watched from a phone or another
    // machine: a user service on a private unix socket, published tailnet-only.
    let view = home.join(".config/systemd/user/omaspace-view.service");
    std::fs::write(&view, VIEW_UNIT)?;
    run_cmd("systemctl", &["--user", "daemon-reload"])?;
    run_cmd("systemctl", &["--user", "enable", "omaspace-view"])?;
    run_cmd("systemctl", &["--user", "restart", "omaspace-view"])?;
    let target = format!("unix:{}", crate::view::socket_path().display());
    match Command::new("tailscale")
        .args(["serve", "--bg", "--https=7788", &target])
        .status()
    {
        Ok(s) if s.success() => {
            println!("live view at https://<this machine>:7788 on your tailnet")
        }
        _ => println!(
            "installed the live view; publish it with: tailscale serve --bg --https=7788 {target} (may need `sudo tailscale set --operator=$USER`)"
        ),
    }

    // Agents' hands for desktop apps: the omaspace-driver package (cua-driver
    // + cua's Hyprland plugin, with the Omarchy patches). Optional; without it
    // agents still get their browsers and terminals.
    let driver_share = Path::new("/usr/share/omaspace-driver");
    if driver_share.is_dir() {
        let apps = home.join(".config/cua-driver/qualified-apps");
        if !apps.exists() {
            std::fs::create_dir_all(apps.parent().unwrap())?;
            std::fs::copy(driver_share.join("qualified-apps.example"), &apps)?;
        }
        let lua = home.join(".config/hypr/cua.lua");
        std::fs::copy(driver_share.join("cua.lua"), &lua)?;
        let main = home.join(".config/hypr/hyprland.lua");
        if let Ok(text) = std::fs::read_to_string(&main)
            && !text.contains("require(\"hypr.cua\")")
        {
            std::fs::write(
                &main,
                format!("{}\nrequire(\"hypr.cua\")\n", text.trim_end()),
            )?;
            println!(
                "the agent input plugin loads at the next Hyprland start (restart the desktop to turn it on)"
            );
        }
        run_cmd(
            "systemctl",
            &["--user", "enable", "--now", "omaspace-cua-driver"],
        )?;
        println!(
            "agents can use desktop apps (omaspace-driver); allowed for typing: {}",
            apps.display()
        );
    } else {
        println!(
            "omaspace-driver isn't installed: agents get browsers and terminals; install it for other desktop apps"
        );
    }

    let bindings = home.join(".config/hypr/bindings.lua");
    if bindings.is_file() {
        let text = std::fs::read_to_string(&bindings)?;
        let kept: String = match text.find(BINDINGS_MARK) {
            Some(at) => text[..at].trim_end().to_string() + "\n",
            None => text.trim_end().to_string() + "\n",
        };
        std::fs::write(&bindings, format!("{kept}\n{BINDINGS}"))?;
        if crate::hypr::available() {
            let _ = Command::new("hyprctl").arg("reload").status();
        }
        println!(
            "bound SUPER+CTRL+SHIFT+O (Spaces), SUPER+CTRL+ALT+O (give) and SUPER-drag to the top edge in {}",
            bindings.display()
        );
    } else {
        println!("no ~/.config/hypr/bindings.lua (not an Omarchy desktop?): skipped keybindings");
    }
    Ok(())
}

fn run_cmd(cmd: &str, args: &[&str]) -> anyhow::Result<()> {
    let status = Command::new(cmd).args(args).status()?;
    anyhow::ensure!(status.success(), "{cmd} {args:?} failed");
    Ok(())
}
