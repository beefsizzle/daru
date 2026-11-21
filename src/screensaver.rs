use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::thread;

use amane::{Argument, Bus, Idle, Method, Service, Value};

const NAME: &str = "org.freedesktop.ScreenSaver";
const INTERFACE: &str = "org.freedesktop.ScreenSaver";

// KDE also answers on /ScreenSaver, but SDL, browsers and the portal all ask here
const PATH: &str = "/org/freedesktop/ScreenSaver";

const BUS: &str = "org.freedesktop.DBus";

static NEXT_COOKIE: AtomicU32 = AtomicU32::new(1);

/*
 * keeps the session awake while a game or a player asks for it. a controller's input goes straight to the game and
 * never reaches the compositor, so without this the session locked mid-game. programs ask org.freedesktop.ScreenSaver,
 * or the desktop portal, which passes its idle inhibits on to it when there's no GNOME session. while anything holds
 * it, Amane holds the idle timeouts off (lock and screens off), and they count again from when the last one lets go
 */
#[derive(Default)]
pub struct ScreenSaver {
    holds: Vec<Hold>,
}

// one Inhibit, until its UnInhibit or until the program that asked leaves the bus
struct Hold {
    cookie: u32,
    owner: String,
}

impl Service for ScreenSaver {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {
        let bus = Bus::session();

        // watched before owning the name, so no early call is missed
        let methods = bus.methods(PATH, INTERFACE);
        let properties = bus.methods(PATH, "org.freedesktop.DBus.Properties");
        let departures = bus.signals("org.freedesktop.DBus", "NameOwnerChanged");

        // something else already answers, like KDE's own screen locker, and then keeping the session awake is its job
        if !bus.own(NAME) {
            return;
        }

        thread::spawn(move || properties.for_each(|method| answer_properties(&method)));

        thread::spawn(move || {
            for signal in departures {
                // (name, old owner, new owner), and an empty new owner means it left
                if let [_, gone, arrived] = signal.arguments()
                    && arrived.text().is_empty()
                {
                    let gone = gone.text();

                    let held = Self::read().holds.iter().any(|hold| hold.owner == gone);

                    if held {
                        Self::write().let_go(|hold| hold.owner == gone);
                    }
                }
            }
        });

        methods.for_each(|method| answer(&method));
    }
}

impl ScreenSaver {
    fn take(&mut self, owner: &str) -> u32 {
        let cookie = NEXT_COOKIE.fetch_add(1, Ordering::Relaxed);

        if self.holds.is_empty() {
            Idle::hold();
        }

        self.holds.push(Hold { cookie, owner: String::from(owner) });

        cookie
    }

    fn let_go(&mut self, done: impl Fn(&Hold) -> bool) {
        let before = self.holds.len();

        self.holds.retain(|hold| !done(hold));

        if before > 0 && self.holds.is_empty() {
            Idle::release();
        }
    }
}

fn answer(method: &Method) {
    match (method.name(), method.arguments()) {
        // (application, reason), answered with the cookie that ends it
        ("Inhibit", [_, _]) => {
            let owner = method.sender();

            let cookie = ScreenSaver::write().take(owner);

            method.reply(&[Argument::Unsigned(cookie)]);

            /*
             * leaving the bus is heard on another thread, which can hear it before the hold above was taken, and the
             * session would then stay awake for good. so the asker is looked for once the hold is there: gone now
             * means it left in between, and leaving later is heard with the hold in place
             */
            let here = Bus::session().call(BUS, "/org/freedesktop/DBus", BUS, "NameHasOwner", &[Argument::from(owner)]);

            // only a clear no: a call that failed says nothing about whether it left
            if matches!(here, Value::Bool(false)) {
                ScreenSaver::write().let_go(|hold| hold.cookie == cookie);
            }
        }

        ("UnInhibit", [cookie]) => {
            method.reply(&[]);

            let cookie = cookie.number() as u32;

            ScreenSaver::write().let_go(|hold| hold.cookie == cookie);
        }

        // whether the screensaver is showing, which this one never does
        ("GetActive", _) => method.reply(&[Argument::Bool(false)]),

        // every call waits for an answer, even one this doesn't know, like SimulateUserActivity
        _ => method.reply(&[]),
    }
}

// there are none, but a caller that never hears back keeps waiting: the GTK portal reads them as it starts, and
// without an answer it didn't come up at all
fn answer_properties(method: &Method) {
    match method.name() {
        "GetAll" => method.reply(&[Argument::Map(BTreeMap::new())]),
        "Get" => method.reply_error("org.freedesktop.DBus.Error.UnknownProperty", "there are no properties"),
        _ => method.reply(&[]),
    }
}
