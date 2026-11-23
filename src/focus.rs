use std::time::Duration;

use amane::Service;

use crate::compositor;

// the open windows, for the bar's shelf, from Umbriel's event stream. empty on another compositor
pub struct Windows {
    pub list: Vec<Window>,
}

/*
 * the windows' titles, apart from the list: a title changes far more often than which windows are open, and only an
 * app's menu shows them, so a new title redraws that menu while it's open and nothing else
 */
pub struct Titles {
    by_id: Vec<(String, String)>,
}

impl Titles {
    pub fn of(&self, id: &str) -> &str {
        self.by_id.iter().find(|(window, _)| window == id).map_or("", |(_, title)| title)
    }
}

impl Service for Titles {
    fn new() -> Self {
        Self { by_id: Vec::new() }
    }

    // written along with the window list, see Windows::listen
    fn listen() {}
}

#[derive(Clone, PartialEq)]
pub struct Window {
    pub id: String,
    pub app_id: String,
    pub focused: bool,
}

/// the string value of `"key":"..."` in a line of JSON, with its escapes undone
pub fn json_string(text: &str, key: &str) -> Option<String> {
    let start = text.find(&format!("\"{key}\":\""))? + key.len() + 4;

    let mut result = String::new();
    let mut characters = text[start..].chars();

    while let Some(letter) = characters.next() {
        match letter {
            '"' => return Some(result),
            '\\' => match characters.next()? {
                // control characters have no place in a one-line title
                'n' | 't' | 'r' | 'b' | 'f' => result.push(' '),
                'u' => {
                    let code = unicode(&mut characters)?;

                    // a character outside the basic plane arrives as two \u escapes
                    if (0xD800..0xDC00).contains(&code) {
                        let low = (characters.next() == Some('\\') && characters.next() == Some('u')).then(|| unicode(&mut characters)).flatten()?;

                        result.push(char::from_u32(0x10000 + ((code - 0xD800) << 10) + (low.wrapping_sub(0xDC00) & 0x3FF))?);
                    } else {
                        result.push(char::from_u32(code)?);
                    }
                }
                other => result.push(other),
            },
            other => result.push(other),
        }
    }

    None
}

fn unicode(characters: &mut std::str::Chars) -> Option<u32> {
    let digits: String = characters.take(4).collect();

    u32::from_str_radix(&digits, 16).ok()
}

/*
 * the windows in one of Umbriel's window lists, {"data":[{window},{window}...],"event":"windows"}. the one with the
 * keyboard says "active":true ("focused" is the last focused one of each workspace), which can't appear inside a
 * title, where its quotes would be escaped. scratchpad windows (the drop-down terminal) come and go by keybind, so
 * they're left out
 */
fn windows(snapshot: &str) -> Vec<Window> {
    objects(snapshot)
        .into_iter()
        .filter(|window| json_string(window, "scratchpad").is_none_or(|pad| pad.is_empty()))
        .filter_map(|window| {
            Some(Window {
                id: json_string(window, "id")?,
                app_id: json_string(window, "app_id").unwrap_or_default(),
                focused: window.contains("\"active\":true"),
            })
        })
        .collect()
}

fn titles(snapshot: &str) -> Vec<(String, String)> {
    objects(snapshot)
        .into_iter()
        .filter_map(|window| Some((json_string(window, "id")?, json_string(window, "title").unwrap_or_default())))
        .collect()
}

/*
 * Umbriel lists the focused window first, so the shelf's apps would swap places on every change of focus. windows
 * already shown keep their place, and new ones go after them
 */
fn in_order(shown: &[Window], mut list: Vec<Window>) -> Vec<Window> {
    list.sort_by_key(|window| shown.iter().position(|old| old.id == window.id).unwrap_or(usize::MAX));

    list
}

/// the objects in the list of one of Umbriel's events, {"data":[{...},{...}],"event":"..."}, each as its own text.
/// braces inside strings (a window titled "{draft}") don't count
pub fn objects(snapshot: &str) -> Vec<&str> {
    let mut found = Vec::new();

    let (mut depth, mut in_string, mut escaped, mut start) = (0, false, false, 0);

    for (at, letter) in snapshot.char_indices() {
        if in_string {
            match letter {
                _ if escaped => escaped = false,
                '\\' => escaped = true,
                '"' => in_string = false,
                _ => {}
            }

            continue;
        }

        match letter {
            '"' => in_string = true,
            '{' => {
                depth += 1;

                if depth == 2 {
                    start = at;
                }
            }
            '}' => {
                if depth == 2 {
                    found.push(&snapshot[start..=at]);
                }

                depth -= 1;
            }
            _ => {}
        }
    }

    found
}

impl Service for Windows {
    fn new() -> Self {
        Self { list: Vec::new() }
    }

    /*
     * Umbriel sends its whole window list on every change to any window (a title, a move, each frame of an animation),
     * far more often than a window opens, closes or takes focus. so a reader thread passes the lists on, and this
     * thread sleeps until one comes, waits a tenth of a second for the rest of a burst, and reads the last of them.
     * a move changes nothing kept here, and a title only Titles, so the bar isn't drawn again for those
     */
    fn listen() {
        if !compositor::umbriel() {
            return;
        }

        let (noted, lists) = std::sync::mpsc::channel::<String>();

        std::thread::spawn(move || follow(&noted));

        while let Ok(mut latest) = lists.recv() {
            std::thread::sleep(Duration::from_millis(100));

            while let Ok(newer) = lists.try_recv() {
                latest = newer;
            }

            let list = in_order(&Self::read().list, windows(&latest));

            if list != Self::read().list {
                Self::write().list = list;
            }

            let by_id = titles(&latest);

            if by_id != Titles::read().by_id {
                Titles::write().by_id = by_id;
            }
        }
    }
}

// the lists as they come, the first being how things are now. started again if Umbriel restarts
fn follow(noted: &std::sync::mpsc::Sender<String>) {
    loop {
        let mut gone = false;

        compositor::umbriel_events(&["windows"], |line| {
            gone = noted.send(line).is_err();

            !gone
        });

        if gone {
            return;
        }

        std::thread::sleep(Duration::from_secs(2));
    }
}

#[cfg(test)]
mod tests {
    use super::{Window, in_order, json_string, titles, windows};

    #[test]
    fn umbriels_windows_and_the_focused_one() {
        let line = r#"{"data":[{"active":false,"app_id":"a","border_effect":{"name":""},"focused":true,"id":"1","scratchpad":"","title":"\"active\":true {x}"},{"active":true,"app_id":"b","focused":true,"id":"2","title":"Mine"},{"active":false,"app_id":"ghostty","focused":false,"id":"3","scratchpad":"quake"}],"event":"windows"}"#;

        let found: Vec<_> =
            windows(line).into_iter().map(|window| (window.id, window.app_id, window.focused)).collect();

        // the drop-down terminal is left out, a workspace's own focused window isn't the one with the keyboard, and
        // a title that looks like the marker doesn't count
        assert_eq!(found, [("1".into(), "a".into(), false), ("2".into(), "b".into(), true)]);

        assert!(windows(r#"{"data":[],"event":"windows"}"#).is_empty());

        assert_eq!(titles(line)[1], ("2".into(), "Mine".into()));
    }

    #[test]
    fn windows_keep_their_place_when_focus_moves() {
        let window = |id: &str, focused| Window { id: id.into(), app_id: String::new(), focused };

        let shown = [window("a", true), window("b", false)];

        // b took focus and a new window c opened, both listed first
        let ordered = in_order(&shown, vec![window("c", false), window("b", true), window("a", false)]);

        let ids: Vec<_> = ordered.iter().map(|window| (window.id.as_str(), window.focused)).collect();

        assert_eq!(ids, [("a", false), ("b", true), ("c", false)]);
    }

    #[test]
    fn finds_the_key() {
        let line = r#"{"WindowFocusChanged":{"id":3,"title":"Terminal","app_id":"foot"}}"#;

        assert_eq!(json_string(line, "title").as_deref(), Some("Terminal"));
        assert_eq!(json_string(line, "app_id").as_deref(), Some("foot"));
        assert_eq!(json_string(line, "missing"), None);
    }

    #[test]
    fn null_is_not_a_string() {
        assert_eq!(json_string(r#"{"title":null}"#, "title"), None);
    }

    #[test]
    fn undoes_escapes() {
        assert_eq!(json_string(r#"{"title":"say \"hi\" \\ ok \/"}"#, "title").as_deref(), Some(r#"say "hi" \ ok /"#));
    }

    #[test]
    fn control_characters_become_spaces() {
        assert_eq!(json_string(r#"{"title":"a\nb\tc"}"#, "title").as_deref(), Some("a b c"));
    }

    #[test]
    fn unicode_escapes() {
        assert_eq!(json_string(r#"{"title":"caf\u00e9"}"#, "title").as_deref(), Some("caf\u{e9}"));
        // outside the basic plane: a surrogate pair
        assert_eq!(json_string(r#"{"title":"\ud83d\ude00!"}"#, "title").as_deref(), Some("\u{1f600}!"));
    }

    #[test]
    fn broken_input_is_none_not_a_panic() {
        assert_eq!(json_string(r#"{"title":"unfinished"#, "title"), None);
        assert_eq!(json_string(r#"{"title":"bad \u12"}"#, "title"), None);
        assert_eq!(json_string(r#"{"title":"lone \ud83d"}"#, "title"), None);
        assert_eq!(json_string(r#"{"title":"cut \"#, "title"), None);
    }
}
