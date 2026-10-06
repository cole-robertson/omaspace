//! Viewer input into Hyprland through a Wayland virtual pointer
//! (`zwlr_virtual_pointer_v1`) and virtual keyboard (`zwp_virtual_keyboard_v1`).
//! These are real input devices to the compositor, so Hyprland keybindings
//! (SUPER+…) fire exactly as they do for a physical keyboard.
//!
//! The keyboard uses a plain US keymap compiled by `xkbcli`, so key names map
//! to Linux evdev codes directly.

use std::os::fd::AsFd;
use std::sync::mpsc;
use wayland_client::protocol::{wl_output, wl_registry, wl_seat};
use wayland_client::{
    Connection, Dispatch, QueueHandle, globals::GlobalListContents, globals::registry_queue_init,
};
use wayland_protocols_misc::zwp_virtual_keyboard_v1::client::{
    zwp_virtual_keyboard_manager_v1::ZwpVirtualKeyboardManagerV1,
    zwp_virtual_keyboard_v1::ZwpVirtualKeyboardV1,
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

/// One input event from a viewer. Pointer positions are 0..1 of the output.
#[derive(Debug, Clone)]
pub enum Event {
    Move {
        x: f64,
        y: f64,
    },
    Button {
        button: u32,
        pressed: bool,
    },
    Scroll {
        dx: f64,
        dy: f64,
    },
    Key {
        code: u32,
        pressed: bool,
    },
    /// Modifier mask (shift=1, ctrl=4, alt=8, super=64) held for the next keys.
    Mods {
        mask: u32,
    },
}

#[derive(Default)]
struct State {
    names: Vec<(u32, String)>,
}

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
macro_rules! ignore_events {
    ($($t:ty),*) => {$(
        impl Dispatch<$t, ()> for State {
            fn event(_: &mut Self, _: &$t, _: <$t as wayland_client::Proxy>::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {}
        }
    )*};
}
/// Output names, learned from wl_output.name (version 4+).
impl Dispatch<wl_output::WlOutput, u32> for State {
    fn event(
        state: &mut Self,
        _: &wl_output::WlOutput,
        event: wl_output::Event,
        id: &u32,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let wl_output::Event::Name { name } = event {
            state.names.push((*id, name));
        }
    }
}
ignore_events!(
    wl_seat::WlSeat,
    ZwlrVirtualPointerManagerV1,
    ZwlrVirtualPointerV1,
    ZwpVirtualKeyboardManagerV1,
    ZwpVirtualKeyboardV1
);

/// A US keymap for the virtual keyboard, compiled once.
fn us_keymap() -> anyhow::Result<std::fs::File> {
    let out = std::process::Command::new("xkbcli")
        .args(["compile-keymap", "--layout", "us"])
        .output()?;
    anyhow::ensure!(out.status.success(), "xkbcli compile-keymap failed");
    let mut keymap = out.stdout;
    keymap.push(0);
    let mut file = tempfile::tempfile()?;
    std::io::Write::write_all(&mut file, &keymap)?;
    Ok(file)
}

/// Start the input thread; send `Event`s to it. Fails if the compositor
/// doesn't offer the virtual input protocols. With `output`, the pointer's
/// absolute coordinates map onto that output only (phone mode's virtual
/// screen); otherwise onto the primary output's `width`x`height`.
pub fn start(width: u32, height: u32, output: Option<&str>) -> anyhow::Result<mpsc::Sender<Event>> {
    let conn = Connection::connect_to_env()?;
    let (globals, mut queue) = registry_queue_init::<State>(&conn)?;
    let qh = queue.handle();
    let mut st = State::default();
    let seat: wl_seat::WlSeat = globals.bind(&qh, 1..=7, ())?;
    let pointer_mgr: ZwlrVirtualPointerManagerV1 = globals.bind(&qh, 1..=2, ())?;
    let keyboard_mgr: ZwpVirtualKeyboardManagerV1 = globals.bind(&qh, 1..=1, ())?;
    let mut target_output = None;
    if let Some(want) = output {
        let outputs: Vec<(u32, wl_output::WlOutput)> = globals.contents().with_list(|list| {
            list.iter()
                .filter(|g| g.interface == "wl_output")
                .map(|g| {
                    (
                        g.name,
                        globals.registry().bind::<wl_output::WlOutput, _, _>(
                            g.name,
                            g.version.min(4),
                            &qh,
                            g.name,
                        ),
                    )
                })
                .collect()
        });
        queue.roundtrip(&mut st)?;
        target_output = st
            .names
            .iter()
            .find(|(_, n)| n == want)
            .and_then(|(id, _)| {
                outputs
                    .iter()
                    .find(|(g, _)| g == id)
                    .map(|(_, o)| o.clone())
            });
        anyhow::ensure!(target_output.is_some(), "output {want} not found");
    }
    let pointer = match &target_output {
        Some(out) if wayland_client::Proxy::version(&pointer_mgr) >= 2 => {
            pointer_mgr.create_virtual_pointer_with_output(Some(&seat), Some(out), &qh, ())
        }
        _ => pointer_mgr.create_virtual_pointer(Some(&seat), &qh, ()),
    };
    let keyboard = keyboard_mgr.create_virtual_keyboard(&seat, &qh, ());
    let keymap = us_keymap()?;
    let size = keymap.metadata()?.len() as u32;
    keyboard.keymap(1 /* XKB_V1 */, keymap.as_fd(), size);
    queue.roundtrip(&mut st)?;

    let (tx, rx) = mpsc::channel::<Event>();
    std::thread::spawn(move || {
        let _keymap = keymap;
        let start = std::time::Instant::now();
        let now = || start.elapsed().as_millis() as u32;
        let mut mods = 0u32; // from sticky buttons / combos
        let mut held = 0u32; // from modifier keys currently down
        for event in rx {
            if std::env::var_os("OMASPACE_DEBUG_INPUT").is_some() {
                eprintln!("input: {event:?}");
            }
            match event {
                Event::Move { x, y } => {
                    let px = (x.clamp(0.0, 1.0) * f64::from(width)) as u32;
                    let py = (y.clamp(0.0, 1.0) * f64::from(height)) as u32;
                    pointer.motion_absolute(now(), px, py, width, height);
                }
                Event::Button { button, pressed } => {
                    use wayland_client::protocol::wl_pointer::ButtonState;
                    pointer.button(
                        now(),
                        button,
                        if pressed {
                            ButtonState::Pressed
                        } else {
                            ButtonState::Released
                        },
                    );
                }
                Event::Scroll { dx, dy } => {
                    use wayland_client::protocol::wl_pointer::Axis;
                    if dy != 0.0 {
                        pointer.axis(now(), Axis::VerticalScroll, dy);
                    }
                    if dx != 0.0 {
                        pointer.axis(now(), Axis::HorizontalScroll, dx);
                    }
                }
                Event::Key { code, pressed } => {
                    // Modifier keys update the xkb modifier state too, so the
                    // focused client sees Shift held for the next key (`>`).
                    let bit = match code {
                        42 | 54 => 1 << 0,   // Shift
                        29 | 97 => 1 << 2,   // Control
                        56 | 100 => 1 << 3,  // Alt
                        125 | 126 => 1 << 6, // Super
                        _ => 0,
                    };
                    keyboard.key(now(), code, u32::from(pressed));
                    if bit != 0 {
                        held = if pressed { held | bit } else { held & !bit };
                        keyboard.modifiers(held | mods, 0, 0, 0);
                    }
                }
                Event::Mods { mask } => {
                    // xkb modifier indices in the US keymap: Shift=0 Control=2 Mod1(Alt)=3 Mod4(Super)=6.
                    let mut depressed = 0;
                    if mask & 1 != 0 {
                        depressed |= 1 << 0;
                    }
                    if mask & 4 != 0 {
                        depressed |= 1 << 2;
                    }
                    if mask & 8 != 0 {
                        depressed |= 1 << 3;
                    }
                    if mask & 64 != 0 {
                        depressed |= 1 << 6;
                    }
                    mods = depressed;
                    keyboard.modifiers(mods | held, 0, 0, 0);
                }
            }
            if matches!(
                event,
                Event::Move { .. } | Event::Button { .. } | Event::Scroll { .. }
            ) {
                pointer.frame();
            }
            // A dead connection would swallow every later key: end the
            // thread so the next event finds the channel closed and reconnects.
            if let Err(e) = queue.flush().map_err(anyhow::Error::from).and_then(|_| {
                queue
                    .dispatch_pending(&mut st)
                    .map(|_| ())
                    .map_err(anyhow::Error::from)
            }) {
                eprintln!("view: input connection lost ({e}); reconnecting on the next event");
                break;
            }
        }
    });
    Ok(tx)
}

/// Linux evdev code for a key name as browsers report it (`KeyboardEvent.code`).
pub fn evdev(code: &str) -> Option<u32> {
    let letters = "QWERTYUIOP";
    let row2 = "ASDFGHJKL";
    let row3 = "ZXCVBNM";
    if let Some(c) = code.strip_prefix("Key").and_then(|k| k.chars().next()) {
        if let Some(i) = letters.find(c) {
            return Some(16 + i as u32);
        }
        if let Some(i) = row2.find(c) {
            return Some(30 + i as u32);
        }
        if let Some(i) = row3.find(c) {
            return Some(44 + i as u32);
        }
    }
    if let Some(d) = code
        .strip_prefix("Digit")
        .and_then(|d| d.parse::<u32>().ok())
    {
        return Some(if d == 0 { 11 } else { 1 + d });
    }
    Some(match code {
        "Escape" => 1,
        "Minus" => 12,
        "Equal" => 13,
        "Backspace" => 14,
        "Tab" => 15,
        "BracketLeft" => 26,
        "BracketRight" => 27,
        "Enter" => 28,
        "ControlLeft" => 29,
        "Semicolon" => 39,
        "Quote" => 40,
        "Backquote" => 41,
        "ShiftLeft" => 42,
        "Backslash" => 43,
        "Comma" => 51,
        "Period" => 52,
        "Slash" => 53,
        "ShiftRight" => 54,
        "AltLeft" => 56,
        "Space" => 57,
        "CapsLock" => 58,
        "F1" => 59,
        "F2" => 60,
        "F3" => 61,
        "F4" => 62,
        "F5" => 63,
        "F6" => 64,
        "F7" => 65,
        "F8" => 66,
        "F9" => 67,
        "F10" => 68,
        "F11" => 87,
        "F12" => 88,
        "ControlRight" => 97,
        "AltRight" => 100,
        "Home" => 102,
        "ArrowUp" => 103,
        "PageUp" => 104,
        "ArrowLeft" => 105,
        "ArrowRight" => 106,
        "End" => 107,
        "ArrowDown" => 108,
        "PageDown" => 109,
        "Insert" => 110,
        "Delete" => 111,
        "MetaLeft" => 125,
        "MetaRight" => 126,
        _ => return None,
    })
}

/// Linux button codes for browser `MouseEvent.button` (0 left, 1 middle, 2 right).
pub fn button(b: u32) -> u32 {
    match b {
        1 => 0x112,
        2 => 0x111,
        _ => 0x110,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn browser_key_codes_map_to_evdev() {
        assert_eq!(evdev("KeyQ"), Some(16));
        assert_eq!(evdev("KeyA"), Some(30));
        assert_eq!(evdev("KeyM"), Some(50));
        assert_eq!(evdev("Digit1"), Some(2));
        assert_eq!(evdev("Digit0"), Some(11));
        assert_eq!(evdev("Enter"), Some(28));
        assert_eq!(evdev("MetaLeft"), Some(125));
        assert_eq!(evdev("Nope"), None);
    }
}
