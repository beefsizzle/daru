use std::time::Duration;

use amane::Service;

use crate::compositor::{self, Compositor};
use crate::focus::{json_string, objects};

/*
 * the layout of the workspace on screen on each monitor, for the bar's layout chip (see bar.rs), and switching it.
 * Umbriel only: it tiles each workspace one of four ways, which `workspace-set-layout` picks between. its workspace
 * events carry each one's layout, so this follows them and never asks
 */

/// Umbriel's layouts in the order a click goes through them: its own default first
pub const ORDER: [&str; 4] = ["scrolling", "dwindle", "master", "stacking"];

/// a picture of each, from the Nerd Font's Material icons
pub fn glyph(layout: &str) -> &'static str {
    match layout {
        // a window in the middle with its neighbours peeking in at the sides
        "scrolling" => "\u{f056c}",
        // half, then half of what's left, and so on
        "dwindle" => "\u{f0574}",
        // one big window beside a stack of small ones
        "master" => "\u{f0bcc}",
        // windows over each other, as on a desktop that doesn't tile
        "stacking" => "\u{f05b2}",
        _ => "\u{f056e}",
    }
}

/// the layout `steps` places along ORDER from this one, going round. an unknown one starts from the first
pub fn after(layout: &str, steps: i32) -> &'static str {
    let at = ORDER.iter().position(|known| *known == layout).unwrap_or(0) as i32;

    ORDER[(at + steps).rem_euclid(ORDER.len() as i32) as usize]
}

#[derive(Default)]
pub struct Layouts {
    pub by_output: Vec<(String, String)>, // (monitor, the layout of its workspace on screen)
}

impl Layouts {
    pub fn on(&self, output: &str) -> Option<&str> {
        self.by_output.iter().find(|(name, _)| name == output).map(|(_, layout)| layout.as_str())
    }
}

// each monitor's workspace on screen ("active"), from one workspaces event
fn read(snapshot: &str) -> Vec<(String, String)> {
    objects(snapshot)
        .into_iter()
        .filter(|workspace| workspace.contains("\"active\":true"))
        .filter_map(|workspace| Some((json_string(workspace, "output")?, json_string(workspace, "layout")?)))
        .collect()
}

impl Service for Layouts {
    fn new() -> Self {
        Self::default()
    }

    // Umbriel sends the workspaces as they are first, then again on every change. started again if it restarts
    fn listen() {
        if compositor::running() != Compositor::Umbriel {
            return;
        }

        loop {
            compositor::umbriel_events(&["workspaces"], |line| {
                let fresh = read(&line);

                if fresh != Self::read().by_output {
                    Self::write().by_output = fresh;
                }

                true
            });

            std::thread::sleep(Duration::from_secs(2));
        }
    }
}

/*
 * moves the monitor's workspace `steps` layouts along. the chip shows the new one straight away, and Umbriel's event
 * confirms it a moment later. the action works on the focused monitor's workspace, which is the one under the
 * pointer when it clicks a bar
 */
pub fn cycle(output: &str, steps: i32) {
    let next = {
        let mut layouts = Layouts::write();

        let Some((_, layout)) = layouts.by_output.iter_mut().find(|(name, _)| name == output) else { return };

        let next = after(layout, steps);

        *layout = next.to_string();

        next
    };

    amane::spawn(&format!("umbriel msg workspace-set-layout {next}"));
}

#[cfg(test)]
mod tests {
    use super::{after, read};

    #[test]
    fn clicks_go_round_the_layouts() {
        assert_eq!(after("scrolling", 1), "dwindle");
        assert_eq!(after("stacking", 1), "scrolling");
        assert_eq!(after("scrolling", -1), "stacking");
        assert_eq!(after("something new", 1), "dwindle");
    }

    #[test]
    fn reads_the_layout_on_screen_on_each_monitor() {
        // trimmed from Umbriel's workspaces event
        let event = r#"{"data":[
            {"active":true,"focused":true,"id":"eDP-1:1","index":1,"layout":"scrolling","name":"1","output":"eDP-1"},
            {"active":false,"focused":false,"id":"eDP-1:2","index":2,"layout":"dwindle","name":"2","output":"eDP-1"},
            {"active":true,"focused":false,"id":"DP-1:1","index":1,"layout":"master","name":"1","output":"DP-1"}
        ],"event":"workspaces"}"#;

        assert_eq!(read(event), vec![(String::from("eDP-1"), String::from("scrolling")), (String::from("DP-1"), String::from("master"))]);
    }
}
