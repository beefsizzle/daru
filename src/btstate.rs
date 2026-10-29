use std::sync::Mutex;
use std::time::Duration;

use amane::{Argument, Bus, Service, Value};

use crate::panels::{self, Which};

/*
 * Bluetooth state and controls, talking to BlueZ directly instead of through Amane's `Bluetooth` service. that service
 * starts the moment anything reads or controls it and then polls BlueZ every two seconds for good, inside `update()`,
 * which holds the service locked while the bar and panels read it. this one polls only while a panel that shows
 * Bluetooth is open, and does the D-Bus call before taking any lock
 */
const BLUEZ: &str = "org.bluez";
const ADAPTER: &str = "org.bluez.Adapter1";
const DEVICE: &str = "org.bluez.Device1";
const BATTERY: &str = "org.bluez.Battery1";
const OBJECTS: &str = "org.freedesktop.DBus.ObjectManager";

#[derive(Clone, Default, PartialEq)]
pub struct Device {
    pub path: String,
    pub name: String,
    pub address: String,
    pub icon: String, // a freedesktop icon name like "audio-headset"
    pub paired: bool,
    pub connected: bool,
    pub battery: Option<u8>,
}

#[derive(Default, PartialEq)]
pub struct BtState {
    pub adapter: Option<String>, // none when the machine has no Bluetooth or bluetoothd isn't running
    pub powered: bool,
    pub scanning: bool,
    pub devices: Vec<Device>,
}

fn read_state() -> BtState {
    let mut state = BtState::default();

    let Value::Map(objects) = Bus::system().call(BLUEZ, "/", OBJECTS, "GetManagedObjects", &[]) else { return state };

    for (path, interfaces) in &objects {
        let adapter = interfaces.get(ADAPTER);

        // the first adapter wins, most machines only have one
        if adapter != &Value::Nothing && state.adapter.is_none() {
            state.adapter = Some(path.clone());
            state.powered = adapter.get("Powered").bool();
            state.scanning = adapter.get("Discovering").bool();
        }

        let device = interfaces.get(DEVICE);

        if device == &Value::Nothing {
            continue;
        }

        let address = device.get("Address").text().to_string();

        // an unnamed device shows its address, like other Bluetooth menus do
        let name = match device.get("Alias").text() {
            "" => address.clone(),
            alias => alias.to_string(),
        };

        let battery = match interfaces.get(BATTERY).get("Percentage") {
            Value::Nothing => None,
            percent => Some(percent.number() as u8),
        };

        state.devices.push(Device {
            path: path.clone(),
            name,
            address,
            icon: device.get("Icon").text().to_string(),
            paired: device.get("Paired").bool(),
            connected: device.get("Connected").bool(),
            battery,
        });
    }

    // connected first, then paired, then by name
    state.devices.sort_by_key(|device| ((!device.connected, !device.paired), device.name.to_lowercase()));

    state
}

fn wanted() -> bool {
    panels::is_open(Which::Bluetooth) || panels::is_open(Which::Dashboard)
}

fn publish(fresh: BtState) {
    if fresh != *BtState::read() {
        *BtState::write() = fresh;
    }
}

impl Service for BtState {
    fn new() -> Self {
        Self::default()
    }

    fn listen() {
        let mut rounds_since_search = 0u32;

        loop {
            if wanted() {
                let state = read_state();

                // the Bluetooth panel is open, the adapter is on, and nothing is searching: the adapter was switched on
                // while the panel was already open (a search can't start on an adapter that's off, so the one asked for when
                // the panel opened did nothing), or it stopped by itself. start it, but not every second if it keeps failing
                let idle = panels::is_open(Which::Bluetooth) && state.powered && !state.scanning;

                rounds_since_search += 1;

                publish(state);

                if idle && rounds_since_search >= 2 {
                    rounds_since_search = 0;

                    follow_panel();

                    publish(read_state());
                }
            } else {
                rounds_since_search = 0;

                panels::wait_until_open(&[Which::Bluetooth, Which::Dashboard]);

                continue;
            }

            std::thread::sleep(Duration::from_secs(1));
        }
    }
}

/// look now, without waiting for the next round (a panel just opened, or something was just done to a device)
pub fn refresh() {
    std::thread::spawn(|| publish(read_state()));
}

// the adapter from the last look, or a look right now if there hasn't been one yet
fn adapter() -> Option<String> {
    BtState::read().adapter.clone().or_else(|| read_state().adapter)
}

// calls run on their own thread: connecting and pairing can take seconds, and nothing here may wait on the drawing thread
fn device_calls(device: &str, methods: &'static [&'static str]) {
    let device = device.to_string();

    std::thread::spawn(move || {
        for method in methods {
            Bus::system().call(BLUEZ, &device, DEVICE, method, &[]);

            publish(read_state());
        }
    });
}

pub fn set_powered(on: bool) {
    std::thread::spawn(move || {
        if let Some(adapter) = adapter() {
            Bus::system().set_property(BLUEZ, &adapter, ADAPTER, "Powered", Argument::from(on));

            publish(read_state());
        }
    });
}

// held while a search is started or stopped, so those calls happen one at a time and in order
static SCAN: Mutex<()> = Mutex::new(());

/*
 * searches while the Bluetooth panel is open and stops when it isn't. looking for devices drains the adapter and the
 * battery of whatever is nearby, so it must not outlive the panel.
 *
 * the panel's state is read under the lock instead of being passed in: an open and a close in quick succession start
 * two threads, and if the stop reached BlueZ first and the start second, the search would run on with nothing left to
 * stop it. whichever call runs last sees the panel as it is now
 */
fn follow_panel() {
    let _turn = crate::sync::lock(&SCAN);

    let Some(adapter) = adapter() else { return };

    let method = if panels::is_open(Which::Bluetooth) { "StartDiscovery" } else { "StopDiscovery" };

    Bus::system().call(BLUEZ, &adapter, ADAPTER, method, &[]);
}

/// the Bluetooth panel opened or closed
pub fn scan_with_panel() {
    std::thread::spawn(|| {
        follow_panel();

        publish(read_state());
    });
}

pub fn connect(path: &str) {
    device_calls(path, &["Connect"]);
}

pub fn disconnect(path: &str) {
    device_calls(path, &["Disconnect"]);
}

/*
 * BlueZ asks a pairing agent to confirm a pairing, and refuses the pairing when no program has registered one, which
 * nothing does here but a desktop's own Bluetooth tool: a mouse that wants its pairing confirmed never paired. so
 * daru registers one for as long as it pairs. it agrees only to the pairing daru started, for the device picked in
 * the dropdown, and only when bluetoothd asks, so nothing nearby can pair itself meanwhile. it can't show or take a
 * code, so a device that needs one typed is still refused
 */
const AGENT_PATH: &str = "/org/amane_shell/bluetooth_agent";
const AGENT: &str = "org.bluez.Agent1";
const AGENT_MANAGER: &str = "org.bluez.AgentManager1";

// the device being paired, the only one the agent agrees to
static PAIRING: Mutex<Option<String>> = Mutex::new(None);

static ANSWERING: std::sync::Once = std::sync::Once::new();

// registered while it lives
struct Agent;

impl Agent {
    fn register(device: &str) -> Self {
        // one thread for good, made at the first pairing: the calls are watched before the agent is registered, so
        // none is missed
        ANSWERING.call_once(|| {
            let calls = Bus::system().methods(AGENT_PATH, AGENT);

            std::thread::spawn(move || calls.for_each(|call| answer(&call)));
        });

        *crate::sync::lock(&PAIRING) = Some(device.to_string());

        let arguments = [Argument::Path(AGENT_PATH.to_string()), Argument::from("NoInputNoOutput")];

        Bus::system().call(BLUEZ, "/org/bluez", AGENT_MANAGER, "RegisterAgent", &arguments);

        Self
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        Bus::system().call(BLUEZ, "/org/bluez", AGENT_MANAGER, "UnregisterAgent", &[Argument::Path(AGENT_PATH.to_string())]);

        *crate::sync::lock(&PAIRING) = None;
    }
}

// what BlueZ asks the agent. the device it's about comes first in every request that has one
fn answer(call: &amane::Method) {
    let bluetoothd = Bus::system().call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "GetNameOwner", &[Argument::from(BLUEZ)]);

    let device = call.arguments().first().map_or("", Value::text);

    let ours = call.sender() == bluetoothd.text() && crate::sync::lock(&PAIRING).as_deref() == Some(device);

    match call.name() {
        // "yes, pair": a mouse or keyboard confirming, or a device asking to pair or use a service
        "RequestConfirmation" | "RequestAuthorization" | "AuthorizeService" if ours => call.reply(&[]),

        // news, nothing to answer but that it arrived
        "Release" | "Cancel" | "DisplayPasskey" | "DisplayPinCode" => call.reply(&[]),

        // a code to type, or anything not about this pairing
        _ => call.reply_error("org.bluez.Error.Rejected", "only the pairing started from the Bluetooth dropdown is confirmed"),
    }
}

/*
 * pairs, trusts and connects; only for devices that pair without a code.
 *
 * BlueZ only keeps a pairing's keys (a bond) while the adapter is pairable, which it often isn't with no other
 * Bluetooth tool running, and it refuses input from an input device that isn't bonded: a game controller would say
 * connected and light up, but never become a controller. so the adapter is pairable for the pairing, and set back
 * after. trusting lets the device connect by itself later, like a controller woken by its button
 */
pub fn pair(path: &str) {
    let device = path.to_string();

    std::thread::spawn(move || {
        let Some(adapter) = adapter() else { return };

        let bus = Bus::system();

        let was_pairable = bus.property(BLUEZ, &adapter, ADAPTER, "Pairable").bool();

        if !was_pairable {
            bus.set_property(BLUEZ, &adapter, ADAPTER, "Pairable", Argument::from(true));
        }

        // BlueZ asks the agent of whoever called Pair, so it's registered on this connection, for this pairing only
        let _agent = Agent::register(&device);

        bus.call(BLUEZ, &device, DEVICE, "Pair", &[]);

        if !was_pairable {
            bus.set_property(BLUEZ, &adapter, ADAPTER, "Pairable", Argument::from(false));
        }

        if bus.property(BLUEZ, &device, DEVICE, "Paired").bool() {
            bus.set_property(BLUEZ, &device, DEVICE, "Trusted", Argument::from(true));
        }

        publish(read_state());

        bus.call(BLUEZ, &device, DEVICE, "Connect", &[]);

        publish(read_state());
    });
}

// unpairs and removes the device, it comes back only after a new scan
pub fn forget(path: &str) {
    let path = path.to_string();

    std::thread::spawn(move || {
        if let Some(adapter) = adapter() {
            Bus::system().call(BLUEZ, &adapter, ADAPTER, "RemoveDevice", &[Argument::Path(path)]);

            publish(read_state());
        }
    });
}
