//! Read Chromium's session file (`Sessions/Session_*`, the SNSS command log)
//! into the open windows and the URL each tab is showing. Read-only; the file is
//! copied by the caller first because Chromium keeps it open.

use std::collections::{BTreeMap, HashSet};

const SET_TAB_WINDOW: u8 = 0;
const SET_TAB_INDEX_IN_WINDOW: u8 = 2;
const UPDATE_TAB_NAVIGATION: u8 = 6;
const SET_SELECTED_NAVIGATION_INDEX: u8 = 7;
const SET_SELECTED_TAB_IN_INDEX: u8 = 8;
const TAB_CLOSED: u8 = 16;
const WINDOW_CLOSED: u8 = 17;

#[derive(Debug, Clone, PartialEq)]
pub struct Window {
    pub id: i32,
    /// URLs in tab order.
    pub urls: Vec<String>,
    /// Index into `urls` of the selected tab.
    pub active: usize,
}

#[derive(Default)]
struct Tab {
    window: i32,
    index: i32,
    selected_nav: i32,
    navs: BTreeMap<i32, String>,
}

fn i32_at(b: &[u8], at: usize) -> Option<i32> {
    Some(i32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// Parse an SNSS session file. Unknown commands are skipped; a truncated tail
/// (Chromium was mid-write) ends the parse rather than failing it.
pub fn parse(data: &[u8]) -> anyhow::Result<Vec<Window>> {
    anyhow::ensure!(
        data.get(..4) == Some(b"SNSS"),
        "not a Chromium session file"
    );
    let mut tabs: BTreeMap<i32, Tab> = BTreeMap::new();
    let mut selected_tab: BTreeMap<i32, i32> = BTreeMap::new();
    let mut closed_windows: HashSet<i32> = HashSet::new();
    let mut off = 8;
    while off + 2 <= data.len() {
        let size = u16::from_le_bytes([data[off], data[off + 1]]) as usize;
        off += 2;
        if size == 0 || off + size > data.len() {
            break;
        }
        let id = data[off];
        let body = &data[off + 1..off + size];
        off += size;
        match id {
            SET_TAB_WINDOW => {
                if let (Some(window), Some(tab)) = (i32_at(body, 0), i32_at(body, 4)) {
                    tabs.entry(tab).or_default().window = window;
                }
            }
            SET_TAB_INDEX_IN_WINDOW => {
                if let (Some(tab), Some(index)) = (i32_at(body, 0), i32_at(body, 4)) {
                    tabs.entry(tab).or_default().index = index;
                }
            }
            UPDATE_TAB_NAVIGATION => {
                // Pickle: uint32 payload length, then int32 tab, int32 index,
                // and the URL as int32 length + bytes.
                let p = body.get(4..).unwrap_or_default();
                if let (Some(tab), Some(index), Some(len)) =
                    (i32_at(p, 0), i32_at(p, 4), i32_at(p, 8))
                    && let Some(url) = usize::try_from(len)
                        .ok()
                        .and_then(|len| p.get(12..12 + len))
                {
                    tabs.entry(tab)
                        .or_default()
                        .navs
                        .insert(index, String::from_utf8_lossy(url).into_owned());
                }
            }
            SET_SELECTED_NAVIGATION_INDEX => {
                if let (Some(tab), Some(index)) = (i32_at(body, 0), i32_at(body, 4)) {
                    tabs.entry(tab).or_default().selected_nav = index;
                }
            }
            SET_SELECTED_TAB_IN_INDEX => {
                if let (Some(window), Some(index)) = (i32_at(body, 0), i32_at(body, 4)) {
                    selected_tab.insert(window, index);
                }
            }
            TAB_CLOSED => {
                if let Some(tab) = i32_at(body, 0) {
                    tabs.remove(&tab);
                }
            }
            WINDOW_CLOSED => {
                if let Some(window) = i32_at(body, 0) {
                    closed_windows.insert(window);
                }
            }
            _ => {}
        }
    }

    let mut windows: BTreeMap<i32, Vec<(i32, String)>> = BTreeMap::new();
    for tab in tabs.into_values() {
        if closed_windows.contains(&tab.window) {
            continue;
        }
        let url = tab
            .navs
            .get(&tab.selected_nav)
            .or_else(|| tab.navs.values().next_back())
            .cloned();
        if let Some(url) = url.filter(|url| !url.is_empty()) {
            windows
                .entry(tab.window)
                .or_default()
                .push((tab.index, url));
        }
    }
    Ok(windows
        .into_iter()
        .map(|(id, mut tabs)| {
            tabs.sort_by_key(|(index, _)| *index);
            let active = selected_tab
                .get(&id)
                .and_then(|&i| usize::try_from(i).ok())
                .filter(|&i| i < tabs.len())
                .unwrap_or(0);
            Window {
                id,
                urls: tabs.into_iter().map(|(_, url)| url).collect(),
                active,
            }
        })
        .collect())
}

/// Build an SNSS file from commands; tests use it to cover the parser.
#[cfg(test)]
pub(crate) fn encode(commands: &[(u8, Vec<u8>)]) -> Vec<u8> {
    let mut out = b"SNSS".to_vec();
    out.extend(3i32.to_le_bytes());
    for (id, body) in commands {
        out.extend(((body.len() + 1) as u16).to_le_bytes());
        out.push(*id);
        out.extend(body);
    }
    out
}

#[cfg(test)]
pub(crate) fn nav(tab: i32, index: i32, url: &str) -> (u8, Vec<u8>) {
    let mut pickle = Vec::new();
    pickle.extend(tab.to_le_bytes());
    pickle.extend(index.to_le_bytes());
    pickle.extend((url.len() as i32).to_le_bytes());
    pickle.extend(url.as_bytes());
    let mut body = (pickle.len() as u32).to_le_bytes().to_vec();
    body.extend(pickle);
    (UPDATE_TAB_NAVIGATION, body)
}

#[cfg(test)]
pub(crate) fn pair(id: u8, a: i32, b: i32) -> (u8, Vec<u8>) {
    let mut body = a.to_le_bytes().to_vec();
    body.extend(b.to_le_bytes());
    (id, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_tabs_follow_their_selected_navigation_in_tab_order() {
        let file = encode(&[
            pair(SET_TAB_WINDOW, 1, 10),
            pair(SET_TAB_WINDOW, 1, 11),
            pair(SET_TAB_INDEX_IN_WINDOW, 10, 1),
            pair(SET_TAB_INDEX_IN_WINDOW, 11, 0),
            nav(10, 0, "https://a.example/"),
            nav(10, 1, "https://a.example/next"),
            pair(SET_SELECTED_NAVIGATION_INDEX, 10, 1),
            nav(11, 0, "https://b.example/"),
            pair(SET_SELECTED_TAB_IN_INDEX, 1, 1),
        ]);
        assert_eq!(
            parse(&file).unwrap(),
            vec![Window {
                id: 1,
                urls: vec!["https://b.example/".into(), "https://a.example/next".into()],
                active: 1,
            }]
        );
    }

    #[test]
    fn closed_tabs_and_windows_are_not_restored() {
        let file = encode(&[
            pair(SET_TAB_WINDOW, 1, 10),
            nav(10, 0, "https://kept.example/"),
            pair(SET_TAB_WINDOW, 1, 11),
            nav(11, 0, "https://closed-tab.example/"),
            (TAB_CLOSED, 11i32.to_le_bytes().to_vec()),
            pair(SET_TAB_WINDOW, 2, 12),
            nav(12, 0, "https://closed-window.example/"),
            (WINDOW_CLOSED, 2i32.to_le_bytes().to_vec()),
        ]);
        let windows = parse(&file).unwrap();
        assert_eq!(windows.len(), 1);
        assert_eq!(windows[0].urls, vec!["https://kept.example/".to_string()]);
    }

    #[test]
    fn truncated_tail_keeps_what_was_complete() {
        let mut file = encode(&[
            pair(SET_TAB_WINDOW, 1, 10),
            nav(10, 0, "https://a.example/"),
        ]);
        file.extend([0xff, 0x00, 6, 1, 2]);
        assert_eq!(
            parse(&file).unwrap()[0].urls,
            vec!["https://a.example/".to_string()]
        );
    }

    #[test]
    fn rejects_files_that_are_not_sessions() {
        assert!(parse(b"nope").is_err());
    }
}
