//! A small stdio MCP server so agents can see your machines, move workspaces,
//! and work in a space of their own on this machine: a workspace you can
//! swipe to and watch, with browsers signed in as you that they drive without
//! ever touching your keyboard, pointer or focus. Clicking into other desktop
//! apps is cua-driver's job.

use crate::{agent, apps, capture, cdp, client, restore, server, tailnet};
use serde_json::{Value, json};
use std::io::{BufRead, Write};

fn browser_args(extra: Value) -> Value {
    let mut props = json!({"agent": {"type": "string"}, "browser": {"type": "string", "description": "browser id (default: your first)"}, "tab": {"type": "string", "description": "tab id (default: first tab)"}});
    for (k, v) in extra.as_object().into_iter().flatten() {
        props[k] = v.clone();
    }
    json!({"type": "object", "required": ["agent"], "properties": props})
}

fn tools() -> Value {
    let agent_name =
        json!({"type": "string", "description": "your name, shown to the user (e.g. \"Claude\")"});
    let workspace = json!({"type": "string", "description": "workspace number, \"active\" or \"all\"", "default": "active"});
    json!([
        {"name": "list_peers", "description": "Your Omarchy machines on the tailnet running omaspace.",
         "inputSchema": {"type": "object", "properties": {}}},
        {"name": "snapshot", "description": "Describe the windows on a workspace of this machine or a peer (browser tabs, terminal directories, editors).",
         "inputSchema": {"type": "object", "properties": {"peer": {"type": "string"}, "workspace": workspace}}},
        {"name": "send_workspace", "description": "Reopen this machine's workspace on a peer.",
         "inputSchema": {"type": "object", "required": ["peer"], "properties": {"peer": {"type": "string"}, "workspace": workspace}}},
        {"name": "pull_workspace", "description": "Reopen a peer's workspace on this machine.",
         "inputSchema": {"type": "object", "required": ["peer"], "properties": {"peer": {"type": "string"}, "workspace": workspace}}},

        {"name": "claim_space", "description": "Get your own Omarchy workspace on this machine to work in, next to the user (they can swipe to it and watch). Call first; keeps the same workspace for your agent name.",
         "inputSchema": {"type": "object", "required": ["agent"], "properties": {"agent": agent_name.clone(), "task": {"type": "string", "description": "one line: what you're doing"}}}},
        {"name": "set_status", "description": "Tell the user what you're doing now (shows on your workspace in their view).",
         "inputSchema": {"type": "object", "required": ["agent", "status"], "properties": {"agent": agent_name.clone(), "status": {"type": "string"}}}},
        {"name": "ask_for_help", "description": "Ask the user to step in (a login, a 2FA code, a decision). They get a banner and can take over your workspace; poll get_space until help is cleared.",
         "inputSchema": {"type": "object", "required": ["agent", "message"], "properties": {"agent": agent_name.clone(), "message": {"type": "string"}}}},
        {"name": "get_space", "description": "Your space: workspace, status, open browsers, and whether your help request is still pending.",
         "inputSchema": {"type": "object", "required": ["agent"], "properties": {"agent": agent_name.clone()}}},
        {"name": "list_spaces", "description": "Every agent space on this machine.", "inputSchema": {"type": "object", "properties": {}}},
        {"name": "release_space", "description": "Done: close your browsers (their copies of the user's sign-ins are deleted) and free your workspace.",
         "inputSchema": {"type": "object", "required": ["agent"], "properties": {"agent": agent_name.clone()}}},

        {"name": "open_browser", "description": "Open a Chromium on your workspace, signed in with all the user's cookies and site data (a private copy, deleted when you release). You drive it with the browser_* tools; it never takes the user's focus.",
         "inputSchema": {"type": "object", "required": ["agent"], "properties": {"agent": agent_name.clone(), "urls": {"type": "array", "items": {"type": "string"}}, "signed_in": {"type": "boolean", "default": true}}}},
        {"name": "browser_tabs", "description": "Your browser's open tabs.", "inputSchema": browser_args(json!({}))},
        {"name": "browser_navigate", "description": "Go to a URL in a tab and wait for it to load.", "inputSchema": browser_args(json!({"url": {"type": "string"}}))},
        {"name": "browser_read", "description": "A tab's URL, title and visible text.", "inputSchema": browser_args(json!({"max_chars": {"type": "integer", "default": 20000}}))},
        {"name": "browser_click", "description": "Click the element matching a CSS selector.", "inputSchema": browser_args(json!({"selector": {"type": "string"}}))},
        {"name": "browser_type", "description": "Type text (into the element matching `selector` if given).", "inputSchema": browser_args(json!({"text": {"type": "string"}, "selector": {"type": "string"}}))},
        {"name": "browser_key", "description": "Press Enter, Tab, Escape, Backspace or an arrow key.", "inputSchema": browser_args(json!({"key": {"type": "string"}}))},
        {"name": "browser_eval", "description": "Run JavaScript in a tab and return its value.", "inputSchema": browser_args(json!({"expression": {"type": "string"}}))},
        {"name": "browser_screenshot", "description": "PNG screenshot of a tab.", "inputSchema": browser_args(json!({}))},
        {"name": "browser_cookies", "description": "Names of the cookies the tab's site has (to check you're signed in).", "inputSchema": browser_args(json!({}))},
        {"name": "open_terminal", "description": "Open a terminal on your workspace (Omarchy's terminal, running tmux), in a folder. Drive it with terminal_*; nothing reaches the user's keyboard or focus.",
         "inputSchema": {"type": "object", "required": ["agent"], "properties": {"agent": {"type": "string"}, "name": {"type": "string", "default": "main"}, "dir": {"type": "string", "description": "folder (~ allowed); default home"}}}},
        {"name": "terminal_run", "description": "Run a shell command in your terminal and wait for it: returns its output and exit code.",
         "inputSchema": {"type": "object", "required": ["agent", "command"], "properties": {"agent": {"type": "string"}, "name": {"type": "string", "default": "main"}, "command": {"type": "string"}, "timeout_secs": {"type": "integer", "default": 120}}}},
        {"name": "terminal_type", "description": "Type text into your terminal (end with \\n to press Enter). For interactive programs (vim, a REPL, a TUI).",
         "inputSchema": {"type": "object", "required": ["agent", "text"], "properties": {"agent": {"type": "string"}, "name": {"type": "string", "default": "main"}, "text": {"type": "string"}}}},
        {"name": "terminal_key", "description": "Press keys in your terminal by tmux name: Enter, Escape, Tab, Up, Down, C-c, C-d, M-x …",
         "inputSchema": {"type": "object", "required": ["agent", "keys"], "properties": {"agent": {"type": "string"}, "name": {"type": "string", "default": "main"}, "keys": {"type": "array", "items": {"type": "string"}}}}},
        {"name": "terminal_read", "description": "What your terminal shows (plus scrollback lines).",
         "inputSchema": {"type": "object", "required": ["agent"], "properties": {"agent": {"type": "string"}, "name": {"type": "string", "default": "main"}, "history": {"type": "integer", "default": 0}}}},

        {"name": "open_app", "description": "Start any desktop app on your workspace (a command like `nautilus ~/Downloads`, or a .desktop id). Returns its window address for the app_* tools.",
         "inputSchema": {"type": "object", "required": ["agent", "command"], "properties": {"agent": {"type": "string"}, "command": {"type": "string"}}}},
        {"name": "app_windows", "description": "The windows on your workspace.", "inputSchema": {"type": "object", "required": ["agent"], "properties": {"agent": {"type": "string"}}}},
        {"name": "app_read", "description": "A window's accessibility tree (buttons, fields, text, each with a token to act on), optionally with a screenshot. Works while it's not focused.",
         "inputSchema": {"type": "object", "required": ["agent", "window"], "properties": {"agent": {"type": "string"}, "window": {"type": "string", "description": "address from open_app/app_windows"}, "screenshot": {"type": "boolean", "default": false}}}},
        {"name": "app_click", "description": "Click an element by token (from app_read), or a point x,y in the window. By token works in the background for most apps.",
         "inputSchema": {"type": "object", "required": ["agent", "window"], "properties": {"agent": {"type": "string"}, "window": {"type": "string"}, "token": {"type": "string"}, "x": {"type": "number"}, "y": {"type": "number"}}}},
        {"name": "app_set_value", "description": "Set a text field's value by token (accessibility).",
         "inputSchema": {"type": "object", "required": ["agent", "window", "token", "value"], "properties": {"agent": {"type": "string"}, "window": {"type": "string"}, "token": {"type": "string"}, "value": {"type": "string"}}}},
        {"name": "app_type", "description": "Type text into a window (apps the user allowed in ~/.config/cua-driver/qualified-apps). Prefer app_set_value for fields.",
         "inputSchema": {"type": "object", "required": ["agent", "window", "text"], "properties": {"agent": {"type": "string"}, "window": {"type": "string"}, "text": {"type": "string"}}}},
        {"name": "app_key", "description": "Press a key, or a combination like [\"ctrl\",\"s\"], in a window.",
         "inputSchema": {"type": "object", "required": ["agent", "window", "keys"], "properties": {"agent": {"type": "string"}, "window": {"type": "string"}, "keys": {"type": "array", "items": {"type": "string"}}}}},
        {"name": "close_window", "description": "Close one of your windows.", "inputSchema": {"type": "object", "required": ["agent", "window"], "properties": {"agent": {"type": "string"}, "window": {"type": "string"}}}},

        {"name": "take_browser_back", "description": "Give your browser back to the user: its tabs reopen in their browser on their workspace with any sign-ins you made.",
         "inputSchema": browser_args(json!({}))}
    ])
}

fn call(name: &str, args: &Value) -> anyhow::Result<Value> {
    let home = std::env::var_os("HOME")
        .map(std::path::PathBuf::from)
        .unwrap_or_default();
    let query = format!(
        "workspace={}",
        args["workspace"].as_str().unwrap_or("active")
    );
    let peer = args["peer"].as_str();
    match name {
        "list_peers" => Ok(json!(
            client::peers()?
                .iter()
                .map(|p| json!({"name": p.name, "desktop": p.desktop}))
                .collect::<Vec<_>>()
        )),
        "snapshot" => match peer {
            Some(peer) => Ok(serde_json::to_value(
                client::snapshot_from(peer, &query)?.0,
            )?),
            None => Ok(serde_json::to_value(
                capture::capture(server::scope_from(&query)?, &tailnet::me()?.name, &home)?.0,
            )?),
        },
        "send_workspace" => {
            let peer = peer.ok_or_else(|| anyhow::anyhow!("peer is required"))?;
            let (snapshot, _) =
                capture::capture(server::scope_from(&query)?, &tailnet::me()?.name, &home)?;
            client::restore_on(peer, &snapshot)
        }
        "pull_workspace" => {
            let peer = peer.ok_or_else(|| anyhow::anyhow!("peer is required"))?;
            let (snapshot, _) = client::snapshot_from(peer, &query)?;
            Ok(serde_json::to_value(restore::restore(&snapshot, &home)?)?)
        }
        _ => agent_call(&home, name, args),
    }
}

fn agent_call(home: &std::path::Path, name: &str, args: &Value) -> anyhow::Result<Value> {
    if name == "list_spaces" {
        return Ok(json!(agent::list(home)));
    }
    let who = args["agent"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("agent is required"))?;
    let text = |k: &str| {
        args[k]
            .as_str()
            .ok_or_else(|| anyhow::anyhow!("{k} is required"))
    };
    let term = || args["name"].as_str().unwrap_or("main");
    let strings = |k: &str| -> Vec<String> {
        args[k]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(String::from))
            .collect()
    };
    let page = || -> anyhow::Result<cdp::Page> {
        agent::browser(home, who, args["browser"].as_str())?.page(args["tab"].as_str())
    };
    Ok(match name {
        "claim_space" => json!(agent::claim(
            home,
            who,
            args["task"].as_str().unwrap_or("")
        )?),
        "set_status" => json!(agent::update(home, who, Some(text("status")?), None)?),
        "ask_for_help" => {
            let s = agent::update(
                home,
                who,
                Some("waiting for you"),
                Some(Some(text("message")?)),
            )?;
            let _ = std::process::Command::new("notify-send")
                .args([
                    "-u",
                    "critical",
                    &format!("{who} needs you (workspace {})", s.workspace),
                    text("message")?,
                ])
                .status();
            json!(s)
        }
        "get_space" => json!(agent::space_of(home, who)?),
        "list_spaces" => json!(agent::list(home)),
        "release_space" => {
            agent::release(home, who)?;
            json!({"released": true})
        }
        "open_browser" => {
            let urls: Vec<String> = args["urls"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|u| u.as_str().map(String::from))
                .collect();
            json!(agent::open_browser(
                home,
                who,
                &urls,
                args["signed_in"].as_bool().unwrap_or(true),
                None
            )?)
        }
        "browser_tabs" => json!(agent::browser(home, who, args["browser"].as_str())?.tabs()?),
        "browser_navigate" => {
            let mut p = page()?;
            p.navigate(text("url")?)?;
            p.read(2000)?
        }
        "browser_read" => page()?.read(args["max_chars"].as_u64().unwrap_or(20000) as usize)?,
        "browser_click" => {
            page()?.click(text("selector")?)?;
            json!({"clicked": true})
        }
        "browser_type" => {
            page()?.type_text(args["selector"].as_str(), text("text")?)?;
            json!({"typed": true})
        }
        "browser_key" => {
            page()?.key(text("key")?)?;
            json!({"pressed": true})
        }
        "browser_eval" => page()?.eval(text("expression")?)?,
        "browser_screenshot" => json!({"png_base64": page()?.screenshot()?}),
        "browser_cookies" => json!(page()?.cookie_names()?),
        "take_browser_back" => {
            json!({"tabs": agent::take_back(home, who, args["browser"].as_str())?})
        }
        "open_terminal" => apps::open_terminal(home, who, term(), args["dir"].as_str())?,
        "terminal_run" => apps::terminal_run(
            who,
            term(),
            text("command")?,
            std::time::Duration::from_secs(args["timeout_secs"].as_u64().unwrap_or(120)),
        )?,
        "terminal_type" => {
            apps::terminal_type(who, term(), text("text")?)?;
            json!({"typed": true})
        }
        "terminal_key" => {
            apps::terminal_key(who, term(), &strings("keys"))?;
            json!({"pressed": true})
        }
        "terminal_read" => {
            json!({"screen": apps::terminal_read(who, term(), args["history"].as_u64().unwrap_or(0) as u32)?})
        }
        "open_app" => apps::open_app(home, who, text("command")?)?,
        "app_windows" => apps::list_windows(home, who)?,
        "app_read" => apps::app_read(
            home,
            who,
            text("window")?,
            args["screenshot"].as_bool().unwrap_or(false),
        )?,
        "app_click" => apps::app_click(
            home,
            who,
            text("window")?,
            args["token"].as_str(),
            args["x"].as_f64().zip(args["y"].as_f64()),
        )?,
        "app_set_value" => {
            apps::app_set_value(home, who, text("window")?, text("token")?, text("value")?)?
        }
        "app_type" => apps::app_type(home, who, text("window")?, text("text")?)?,
        "app_key" => apps::app_key(home, who, text("window")?, &strings("keys"))?,
        "close_window" => {
            apps::close_window(home, who, text("window")?)?;
            json!({"closed": true})
        }
        _ => anyhow::bail!("unknown tool {name}"),
    })
}

pub fn run() -> anyhow::Result<()> {
    let stdin = std::io::stdin();
    let mut out = std::io::stdout();
    for line in stdin.lock().lines() {
        let line = line?;
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let Some(id) = message.get("id").cloned() else {
            continue;
        }; // notifications
        let result = match message["method"].as_str().unwrap_or("") {
            "initialize" => json!({
                "protocolVersion": "2025-06-18",
                "capabilities": {"tools": {}},
                "serverInfo": {"name": "omaspace", "version": env!("CARGO_PKG_VERSION")},
                "instructions": "Omarchy workspaces across your machines, and a space of your own on this one. To work next to the user: claim_space, then use your workspace: open_browser (signed in as them) + browser_*, open_terminal + terminal_*, and open_app + app_* for any other desktop app. set_status as you go; ask_for_help when you need them; release_space when done. You never touch their keyboard, pointer or focus, and you can only act on windows on your own workspace."
            }),
            "tools/list" => json!({"tools": tools()}),
            "tools/call" => match call(
                message["params"]["name"].as_str().unwrap_or(""),
                &message["params"]["arguments"],
            ) {
                Ok(value) => {
                    json!({"content": [{"type": "text", "text": value.to_string()}], "structuredContent": value})
                }
                Err(e) => {
                    json!({"content": [{"type": "text", "text": e.to_string()}], "isError": true})
                }
            },
            "ping" => json!({}),
            other => {
                writeln!(
                    out,
                    "{}",
                    json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("unknown method {other}")}})
                )?;
                out.flush()?;
                continue;
            }
        };
        writeln!(
            out,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "result": result})
        )?;
        out.flush()?;
    }
    Ok(())
}
