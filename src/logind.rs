use std::process::{Child, Command, Stdio};
use std::time::Duration;

use amane::{Argument, Bus, Lock, Service};

/*
 * locks the screen when something else asks for it:
 *  - `loginctl lock-session` (idle daemons, other tools) emits logind's Lock signal
 *  - before the machine suspends, so the lock is up before the screen goes off and when it wakes
 */
pub struct Logind;

impl Service for Logind {
    fn new() -> Self {
        Logind
    }

    fn interval() -> Duration {
        Duration::from_secs(3600)
    }

    fn listen() {
        std::thread::spawn(watch_lock);

        // logind waits for delay inhibitors before it suspends, which gives the lock a moment to appear
        let mut inhibitor = inhibit();

        for signal in Bus::system().signals("org.freedesktop.login1.Manager", "PrepareForSleep") {
            let going_to_sleep = signal.arguments().first().is_some_and(amane::Value::bool);

            if going_to_sleep {
                Lock::start();

                std::thread::sleep(Duration::from_millis(800));

                if let Some(mut child) = inhibitor.take() {
                    release(&mut child);
                }
            } else if !holding(&mut inhibitor) {
                // after waking, or when something ended the inhibitor while we slept
                inhibitor = inhibit();
            }
        }
    }
}

/*
 * the lock is held by `systemd-inhibit` for as long as its command runs. the command waits for this process to exit
 * (`tail --pid`) instead of `sleep infinity`, so a restart, a crash or a kill releases the lock too, and no shell has
 * to clean up after another, which would also take the lock away from a second shell that's still running
 */
fn inhibit() -> Option<Child> {
    let pid = std::process::id().to_string();

    Command::new("systemd-inhibit")
        .args(["--what=sleep", "--who=amane-shell", "--why=lock the screen before sleeping", "--mode=delay", "tail", "--pid"])
        .arg(pid)
        .args(["-f", "/dev/null"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .ok()
}

// whether the inhibitor is still running
fn holding(inhibitor: &mut Option<Child>) -> bool {
    inhibitor.as_mut().is_some_and(|child| matches!(child.try_wait(), Ok(None)))
}

// `systemd-inhibit` runs its command as a child of its own, and killing only the parent leaves that child (a `tail`)
// behind, one for every suspend. ending the child ends the parent too, and the lock goes with it
fn release(inhibitor: &mut Child) {
    let _ = Command::new("pkill").args(["-P", &inhibitor.id().to_string()]).status();

    let _ = inhibitor.kill();
    let _ = inhibitor.wait();
}

/*
 * the logind object of this session, so a lock request for somebody else's session doesn't lock ours. logind can't
 * always find the shell by pid (one started as a user service isn't inside the session's own cgroup), so the session
 * id in the environment is tried first, then the graphical session logind lists for this user
 */
fn session_path(bus: &Bus) -> String {
    let manager = "org.freedesktop.login1.Manager";

    if let Ok(session) = std::env::var("XDG_SESSION_ID") {
        let path = bus
            .call("org.freedesktop.login1", "/org/freedesktop/login1", manager, "GetSession", &[Argument::Text(session)])
            .text()
            .to_string();

        if !path.is_empty() {
            return path;
        }
    }

    let user = std::fs::metadata("/proc/self").map_or(0, |metadata| std::os::unix::fs::MetadataExt::uid(&metadata));

    // Display is a (session id, session path) pair
    let display = bus.property("org.freedesktop.login1", &format!("/org/freedesktop/login1/user/_{user}"), "org.freedesktop.login1.User", "Display");

    display.list().get(1).map_or(String::new(), |path| path.text().to_string())
}

fn watch_lock() {
    let bus = Bus::system();

    let path = session_path(&bus);

    // without it, every user's lock request would lock this screen, so none is better
    if path.is_empty() {
        eprintln!("amane: couldn't find this login session, so lock requests from logind are ignored");

        return;
    }

    for signal in bus.signals("org.freedesktop.login1.Session", "Lock") {
        if signal.path() == path {
            Lock::start();
        }
    }
}
