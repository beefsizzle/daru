/*
 * whether the NixOS flake this system is built from has updates: for each of its inputs (nixpkgs, home-manager...),
 * is the commit in flake.lock still the newest on the branch it follows. this is the cheap question. which packages
 * would change needs nixpkgs evaluated (a gigabyte and half a minute), and `nh os switch --update --ask` shows that
 * list anyway before it switches, which is what the dashboard's Update button runs.
 *
 * nothing here runs off NixOS (no /etc/NIXOS), or without a flake: the dashboard hides the row then.
 */

use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use amane::Service;

// how often GitHub is asked. its API allows 60 unauthenticated requests an hour and a check is one per input
const CHECK_EVERY: Duration = Duration::from_secs(6 * 3600);

// after a failed check (offline, rate limited), when to try again
const RETRY_AFTER: Duration = Duration::from_secs(15 * 60);

#[derive(Clone, Debug, PartialEq)]
pub struct Input {
    pub name: String,
    owner: String,
    repo: String,
    // the branch the input follows, or HEAD for the repository's default one
    branch: String,
    locked: String,
    // when the locked commit was made
    pub modified: SystemTime,
    // the branch's newest commit at the last check
    newest: Option<String>,
}

impl Input {
    pub fn behind(&self) -> bool {
        self.newest.as_ref().is_some_and(|newest| *newest != self.locked)
    }
}

#[derive(Default)]
pub struct NixUpdates {
    // the folder with flake.nix, None off NixOS or with no flake found
    pub flake: Option<PathBuf>,
    pub inputs: Vec<Input>,
    pub checked: Option<SystemTime>,
    pub checking: bool,
    // the last check didn't get through
    pub failed: bool,
}

impl NixUpdates {
    pub fn behind(&self) -> Vec<&Input> {
        self.inputs.iter().filter(|input| input.behind()).collect()
    }

    pub fn input(&self, name: &str) -> Option<&Input> {
        self.inputs.iter().find(|input| input.name == name)
    }
}

/*
 * where nh is told the flake is (programs.nh.flake sets NH_FLAKE for the session), or NixOS's default place. only a
 * folder with a flake.lock counts
 */
fn find_flake() -> Option<PathBuf> {
    ["NH_OS_FLAKE", "NH_FLAKE"]
        .iter()
        .filter_map(std::env::var_os)
        .map(PathBuf::from)
        .chain(std::iter::once(PathBuf::from("/etc/nixos")))
        .find(|folder| folder.join("flake.lock").is_file())
}

/*
 * the root's own inputs that live on GitHub, one per line as name|owner|repo|branch|rev|lastModified. an input that
 * `follows` another one has a list instead of a node name and is left out, and so is one pinned to a commit in
 * flake.nix, which never has updates
 */
const QUERY: &str = r#"
.nodes as $n | $n.root.inputs | to_entries[] | select(.value | type == "string") | $n[.value] as $i
| select($i.locked.type == "github" and $i.original.rev == null)
| "\(.key)|\($i.locked.owner)|\($i.locked.repo)|\($i.original.ref // "HEAD")|\($i.locked.rev)|\($i.locked.lastModified)"
"#;

fn parse(text: &str) -> Vec<Input> {
    text.lines()
        .filter_map(|line| {
            let fields: Vec<&str> = line.split('|').collect();

            let [name, owner, repo, branch, locked, modified] = fields[..] else { return None };

            Some(Input {
                name: name.to_string(),
                owner: owner.to_string(),
                repo: repo.to_string(),
                branch: branch.to_string(),
                locked: locked.to_string(),
                modified: UNIX_EPOCH + Duration::from_secs(modified.parse().ok()?),
                newest: None,
            })
        })
        .collect()
}

fn read_lock(flake: &Path) -> Vec<Input> {
    let lock = flake.join("flake.lock");

    // the path goes inside single quotes, so any single quote in it is closed, escaped and opened again
    let quoted = lock.to_string_lossy().replace('\'', r"'\''");

    parse(&amane::output(&format!("jq -r '{QUERY}' '{quoted}'")))
}

// GitHub names, which are letters, digits and -_. only, as they end up in a command
fn safe(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|letter| letter.is_ascii_alphanumeric() || "-_./".contains(letter))
}

// the newest commit on the input's branch. the sha media type makes GitHub answer with the bare 40 characters
fn newest(input: &Input) -> Option<String> {
    if ![&input.owner, &input.repo, &input.branch].iter().all(|name| safe(name)) {
        return None;
    }

    let answer = amane::output(&format!(
        "curl -fsS --max-time 15 -H 'Accept: application/vnd.github.sha' \
         'https://api.github.com/repos/{}/{}/commits/{}'",
        input.owner, input.repo, input.branch
    ));

    (answer.len() == 40 && answer.chars().all(|letter| letter.is_ascii_hexdigit())).then_some(answer)
}

/// asks GitHub now. the listen loop does this every few hours, the dashboard's button when it's clicked
pub fn check() {
    let inputs = {
        let mut state = NixUpdates::write();

        if state.checking || state.flake.is_none() {
            return;
        }

        state.checking = true;

        state.inputs.clone()
    };

    // outside the lock: a slow answer mustn't hold up the dashboard, which reads this every frame while it's open
    let answers: Vec<(String, Option<String>)> = inputs.iter().map(|input| (input.name.clone(), newest(input))).collect();

    let mut state = NixUpdates::write();

    let failed = answers.iter().any(|(_, answer)| answer.is_none());

    // what came back goes onto the inputs as they are now, in case the lock was read again in the meantime. an input
    // that got no answer keeps what the last check said
    for (name, answer) in answers {
        if let (Some(input), Some(answer)) = (state.inputs.iter_mut().find(|input| input.name == name), answer) {
            input.newest = Some(answer);
        }
    }

    state.checking = false;
    state.failed = failed;
    state.checked = Some(SystemTime::now());
}

impl Service for NixUpdates {
    fn new() -> Self {
        Self::default()
    }

    /*
     * every minute: read flake.lock again if it changed (after an update the row clears at once, as the branches'
     * newest commits are already known), and ask GitHub when the last check is old enough. the times are wall-clock
     * times, so a laptop that slept through the 6 hours checks when it wakes up.
     *
     * Amane starts this the first time the dashboard opens, not at login, so the first check runs straight away: the
     * network is up by then
     */
    fn listen() {
        if !Path::new("/etc/NIXOS").exists() {
            return;
        }

        let mut seen: Option<(PathBuf, SystemTime)> = None;

        loop {
            if let Some(flake) = find_flake() {
                let changed = std::fs::metadata(flake.join("flake.lock")).and_then(|metadata| metadata.modified()).ok();

                if let Some(changed) = changed
                    && seen.as_ref() != Some(&(flake.clone(), changed))
                {
                    let mut inputs = read_lock(&flake);

                    let mut state = Self::write();

                    // what each branch had at the last check still holds for the new lock
                    for input in &mut inputs {
                        let same = |old: &&Input| (&old.owner, &old.repo, &old.branch) == (&input.owner, &input.repo, &input.branch);

                        input.newest = state.input(&input.name).filter(same).and_then(|old| old.newest.clone());
                    }

                    state.flake = Some(flake.clone());
                    state.inputs = inputs;

                    seen = Some((flake, changed));
                }
            }

            let due = {
                let state = Self::read();

                let wait = if state.failed { RETRY_AFTER } else { CHECK_EVERY };

                // a check from the future (the clock was set back) counts as old
                state.flake.is_some()
                    && state.checked.is_none_or(|checked| SystemTime::now().duration_since(checked).unwrap_or(Duration::MAX) >= wait)
            };

            if due {
                check();
            }

            std::thread::sleep(Duration::from_secs(60));
        }
    }
}

/// a terminal running the update: `nh os switch --update --ask` lists what would change and asks before it switches.
/// without nh, the same in plain Nix (no list, no question). the window waits for Enter at the end, so the result
/// can be read
pub fn update() {
    let Some(flake) = NixUpdates::read().flake.clone() else { return };

    let quoted = flake.to_string_lossy().replace('\'', r"'\''");

    let run = if amane::output("command -v nh").is_empty() {
        format!("nix flake update --flake '{quoted}' && sudo nixos-rebuild switch --flake '{quoted}'")
    } else {
        format!("nh os switch --update --ask '{quoted}'")
    };

    let script = format!("{run}; echo; printf 'Press Enter to close'; read _").replace('\'', r"'\''");

    let terminal = std::env::var("TERMINAL").ok().filter(|name| safe(name)).unwrap_or_else(|| String::from("foot"));

    // foot ignores -e, the others need it
    amane::spawn(&format!("{terminal} -e sh -c '{script}'"));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_lock_lines_become_inputs() {
        let inputs = parse(
            "nixpkgs|NixOS|nixpkgs|nixos-unstable|0bb2eed|1793540411\n\
             umbriel|noctalia-dev|umbriel|HEAD|e0da106|1793669697\n\
             broken|line",
        );

        assert_eq!(inputs.len(), 2);
        assert_eq!(inputs[0].name, "nixpkgs");
        assert_eq!(inputs[0].branch, "nixos-unstable");
        assert_eq!(inputs[1].branch, "HEAD");
        assert_eq!(inputs[1].modified, UNIX_EPOCH + Duration::from_secs(1793669697));
    }

    #[test]
    fn an_input_is_behind_only_once_a_newer_commit_is_known() {
        let mut input = parse("nixpkgs|NixOS|nixpkgs|nixos-unstable|aaa|0").remove(0);

        assert!(!input.behind());

        input.newest = Some(String::from("aaa"));
        assert!(!input.behind());

        input.newest = Some(String::from("bbb"));
        assert!(input.behind());
    }

    #[test]
    fn names_that_could_break_the_command_are_refused() {
        assert!(safe("nix-community"));
        assert!(safe("nixos-unstable"));
        assert!(safe("release-25.11"));
        assert!(!safe("a'b"));
        assert!(!safe("a b"));
        assert!(!safe("$(x)"));
        assert!(!safe(""));
    }
}
