use std::time::Duration;

use amane::Service;

use crate::compositor;

// the title of the focused window, from Umbriel's event stream. empty on another compositor
pub struct Focus {
    pub title: String,
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
 * the focused window's title in one of Umbriel's window lists, {"data":[{window},{window}...],"event":"windows"}:
 * the window object that says "focused":true. that can't appear inside a title, where its quotes would be escaped
 */
fn focused_title(snapshot: &str) -> String {
    objects(snapshot)
        .into_iter()
        .find(|window| window.contains("\"focused\":true"))
        .map_or_else(String::new, |window| json_string(window, "title").unwrap_or_default())
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

impl Service for Focus {
    fn new() -> Self {
        Self { title: String::new() }
    }

    /*
     * Umbriel sends its whole window list on every change to any window (a title, a move, each frame of an animation),
     * far more often than the focused one's title changes. so a reader thread passes the lists on, and this thread
     * sleeps until one comes, waits a tenth of a second for the rest of a burst, and works out the title once, from
     * the last of them. nothing wakes while nothing happens
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

            let title = focused_title(&latest);

            if title != Self::read().title {
                Self::write().title = title;
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
    use super::{focused_title, json_string};

    #[test]
    fn umbriels_focused_window_is_the_one_marked() {
        let line = r#"{"data":[{"app_id":"a","border_effect":{"name":""},"focused":false,"title":"Other"},{"app_id":"b","focused":true,"title":"Mine \"quoted\" {x}","title_bar":false}],"event":"windows"}"#;

        assert_eq!(focused_title(line), r#"Mine "quoted" {x}"#);

        // nothing focused, like an empty workspace
        assert_eq!(focused_title(r#"{"data":[{"focused":false,"title":"x"}],"event":"windows"}"#), "");
        assert_eq!(focused_title(r#"{"data":[],"event":"windows"}"#), "");
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
