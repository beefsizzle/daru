use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;

use amane::{Argument, Bus, Color, Service, Value};

use crate::theme::{Theme, hex, mix, over, signal, theme, to_hsl};

/*
 * the wallpaper's colours for other apps, written whenever the shell's own colours change. each app reads its file
 * from its own config, so it has to be pointed at it once (see the README); the shell only ever writes its own files
 * and never edits an app's config. which apps get a file is up to ~/.config/amane-shell/recolor, one name per line:
 *
 *     gtk       ~/.config/gtk-4.0/daru.css, for GTK4 and libadwaita apps when they next start, and GNOME's accent
 *               setting, which libadwaita apps follow at once, even ones already open
 *     niri      ~/.config/niri/daru-colors.kdl, the focus ring. niri reloads it by itself
 *     umbriel   ~/.config/umbriel/daru-colors.toml, the window borders, for `[include.optional] files =
 *               ["daru-colors.toml"]` in Umbriel's config. Umbriel is asked to reload
 *     ghostty   ~/.config/ghostty/daru-colors, background, text, cursor and selection. Ghostty is asked to reload
 *     zed       ~/.config/zed/themes/daru.json, a theme called "Daru". Zed reloads it by itself
 *     kde       ~/.local/share/color-schemes/Daru.colors, a colour scheme for KDE apps like Dolphin, and the same colours
 *               in ~/.config/kdeglobals, where Qt apps take them from with QT_QPA_PLATFORMTHEME=kde (plasma-integration).
 *               open ones are told the palette changed, as Plasma does, and recolour at once
 *     strata    ~/.config/strata/themes/daru.toml, a theme called "Daru" for the Strata file manager, which reloads it
 *               by itself
 *     btop      ~/.config/btop/themes/daru.theme, for btop, which is told to read its config again (SIGUSR2) and picks up
 *               the new colours at once
 *     sddm      /var/lib/daru-sddm, the wallpaper and its colours for the login screen (sddm/ in this repository). the
 *               greeter runs as its own user and can't read the home folder, so they're copied to a folder that
 *               sddm/install.sh makes once, with sudo. nothing is written until it exists
 *     regreet   /var/lib/daru-greeter, the same for the login screen under greetd (greetd/ in this repository): the
 *               wallpaper and a stylesheet for ReGreet, in a folder greetd/install.sh makes once, with sudo
 */
#[derive(Clone, Copy, PartialEq)]
enum App {
    Gtk,
    Niri,
    Umbriel,
    Ghostty,
    Zed,
    Kde,
    Strata,
    Btop,
    Sddm,
    Regreet,
}

// one export at a time: two wallpaper changes in quick succession must not interleave their writes
static WRITING: Mutex<()> = Mutex::new(());

fn wanted() -> Vec<App> {
    let text = std::fs::read_to_string(crate::cache::config_dir().join("recolor")).unwrap_or_default();

    text.lines()
        .map(|line| line.split('#').next().unwrap_or("").trim())
        .filter_map(|name| match name {
            "gtk" => Some(App::Gtk),
            "niri" => Some(App::Niri),
            "umbriel" => Some(App::Umbriel),
            "ghostty" => Some(App::Ghostty),
            "zed" => Some(App::Zed),
            "kde" => Some(App::Kde),
            "strata" => Some(App::Strata),
            "btop" => Some(App::Btop),
            "sddm" => Some(App::Sddm),
            "regreet" => Some(App::Regreet),
            _ => None,
        })
        .collect()
}

/// writes the current colours for every app that asked for them. the files are small, but this runs on a thread of
/// its own all the same, after the shell has already recoloured itself. DARU_NO_RECOLOR turns it off, for a second
/// shell that shares the home folder, like dev/nested.sh's
pub fn export() {
    if std::env::var_os("DARU_NO_RECOLOR").is_some_and(|value| !value.is_empty()) {
        return;
    }

    std::thread::spawn(|| {
        let _writing = crate::sync::lock(&WRITING);

        let apps = wanted();

        if apps.is_empty() {
            return;
        }

        let colors = Colors::from(&theme());

        for app in apps {
            match app {
                App::Gtk => {
                    write(&config("gtk-4.0/daru.css"), &gtk(&colors));

                    set_accent(gnome_accent(colors.accent));
                }
                App::Niri => {
                    write(&config("niri/daru-colors.kdl"), &niri(&colors));
                }
                App::Umbriel => {
                    if write(&config("umbriel/daru-colors.toml"), &umbriel(&colors)) {
                        reload_umbriel();
                    }
                }
                App::Ghostty => {
                    if write(&config("ghostty/daru-colors"), &ghostty(&colors)) {
                        reload_ghostty();
                    }
                }
                App::Zed => {
                    write(&config("zed/themes/daru.json"), &zed(&colors));
                }
                App::Kde => {
                    let scheme = kde(&colors);

                    write(&data("color-schemes/Daru.colors"), &scheme);

                    let globals = config("kdeglobals");

                    // KDE apps write to kdeglobals too, so it's read, changed and written while holding their lock
                    let changed = {
                        let _locked = KdeLock::take(&globals);

                        let current = std::fs::read_to_string(&globals).unwrap_or_default();

                        write(&globals, &with_colors(&current, &scheme))
                    };

                    if changed {
                        palette_changed();
                    }
                }
                App::Strata => {
                    write(&config("strata/themes/daru.toml"), &strata(&colors));
                }
                App::Btop => {
                    if write(&config("btop/themes/daru.theme"), &btop(&colors)) {
                        reload_btop();
                    }
                }
                App::Sddm => {
                    login_screen(&colors);
                }
                App::Regreet => {
                    greeter(&colors);
                }
            }
        }
    });
}

fn config(file: &str) -> PathBuf {
    crate::cache::config_dir().parent().map_or_else(PathBuf::new, Path::to_path_buf).join(file)
}

// under ~/.local/share, or $XDG_DATA_HOME
fn data(file: &str) -> PathBuf {
    let home = std::env::var_os("XDG_DATA_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".local/share"));

    home.join(file)
}

// written under another name and renamed, so an app never reads half a file. false when it was already up to date,
// so nothing needs to reload. a symlink (kdeglobals kept with dotfiles) stays one: its target is replaced instead
fn write(path: &Path, text: &str) -> bool {
    if std::fs::read_to_string(path).is_ok_and(|current| current == text) {
        return false;
    }

    let resolved = path.is_symlink().then(|| std::fs::canonicalize(path).ok()).flatten();
    let path = resolved.as_deref().unwrap_or(path);

    if let Some(folder) = path.parent() {
        let _ = std::fs::create_dir_all(folder);
    }

    let partial = path.with_extension("daru-part");

    std::fs::write(&partial, text).is_ok() && std::fs::rename(&partial, path).is_ok()
}

// the theme's colours made solid, as other apps want them, plus a few hues for syntax and the terminal
struct Colors {
    background: Color, // the panels' background
    surface: Color,    // chips and cards
    raised: Color,     // the same, hovered or selected
    border: Color,
    text: Color,
    dim: Color,
    faint: Color,
    accent: Color,
    on_accent: Color,
    red: Color,
    yellow: Color,
    green: Color,
    blue: Color,
    purple: Color,
    orange: Color,
}

impl Colors {
    fn from(t: &Theme) -> Self {
        let background = Color::rgb(t.panel.red(), t.panel.green(), t.panel.blue());

        let hue = |seed: Color| signal(seed, t.accent);

        Self {
            background,
            surface: over(t.surface, background),
            raised: over(t.surface_hi, background),
            border: over(t.border, background),
            text: t.fg,
            dim: over(t.dim, background),
            faint: over(t.faint, background),
            accent: t.accent,
            on_accent: t.on_accent,
            red: t.red,
            yellow: t.yellow,
            green: hue(Color::rgb(0x9e, 0xce, 0x6a)),
            blue: hue(Color::rgb(0x7a, 0xa2, 0xf7)),
            purple: hue(Color::rgb(0xbb, 0x9a, 0xf7)),
            orange: hue(Color::rgb(0xff, 0x9e, 0x64)),
        }
    }
}

const HEADER: &str = "written by the daru shell from the wallpaper, and rewritten on every change";

/*
 * libadwaita 1.6 and later read the css variables, older GTK4 apps the named colours. GTK reads this file only when an
 * app starts, so the accent isn't in it: it comes from GNOME's accent setting instead (see `set_accent`), which open
 * apps follow at once. a colour here would override that setting
 */
fn gtk(c: &Colors) -> String {
    let roles = [
        ("window-bg", c.background),
        ("window-fg", c.text),
        ("view-bg", c.background),
        ("view-fg", c.text),
        ("headerbar-bg", c.surface),
        ("headerbar-fg", c.text),
        ("headerbar-backdrop", c.background),
        ("sidebar-bg", c.surface),
        ("sidebar-fg", c.text),
        ("secondary-sidebar-bg", c.surface),
        ("secondary-sidebar-fg", c.text),
        ("card-bg", c.surface),
        ("card-fg", c.text),
        ("dialog-bg", c.surface),
        ("dialog-fg", c.text),
        ("popover-bg", c.raised),
        ("popover-fg", c.text),
        ("thumbnail-bg", c.surface),
        ("thumbnail-fg", c.text),
    ];

    let mut text = format!("/* {HEADER} */\n\n:root {{\n");

    for (role, color) in roles {
        text.push_str(&format!("    --{role}-color: {};\n", hex(color)));
    }

    text.push_str("}\n\n");

    for (role, color) in roles {
        text.push_str(&format!("@define-color {}_color {};\n", role.replace('-', "_"), hex(color)));
    }

    text
}

/*
 * GNOME's accent setting only has nine colours, so the wallpaper's accent becomes the nearest of them by hue. a nearly
 * grey accent becomes slate. the hues are those of libadwaita's own accent colours
 */
fn gnome_accent(accent: Color) -> &'static str {
    const HUES: [(&str, f32); 8] = [
        ("blue", 213.0),
        ("teal", 190.0),
        ("green", 131.0),
        ("yellow", 41.0),
        ("orange", 23.0),
        ("red", 353.0),
        ("pink", 331.0),
        ("purple", 285.0),
    ];

    let (hue, saturation, _) = to_hsl(accent);

    if saturation < 0.15 {
        return "slate";
    }

    // the distance around the colour wheel, so 350 and 10 are close
    let distance = |other: f32| {
        let apart = (hue - other).abs() % 360.0;

        apart.min(360.0 - apart)
    };

    HUES.iter().min_by(|a, b| distance(a.1).total_cmp(&distance(b.1))).map_or("blue", |(name, _)| name)
}

// only when it changes: every change makes every libadwaita app restyle
fn set_accent(name: &str) {
    let current = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "accent-color"])
        .stderr(Stdio::null())
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().trim_matches('\'').to_string())
        .unwrap_or_default();

    if current == name {
        return;
    }

    let _ = Command::new("gsettings")
        .args(["set", "org.gnome.desktop.interface", "accent-color", name])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn niri(c: &Colors) -> String {
    format!(
        "// {HEADER}\n\nlayout {{\n    focus-ring {{\n        active-color \"{}\"\n        inactive-color \"{}\"\n    }}\n}}\n",
        hex(c.accent),
        hex(c.raised),
    )
}

// the window borders, as niri's focus ring: the accent on the focused window. colours as "#rrggbbaa"
fn umbriel(c: &Colors) -> String {
    format!(
        "# {HEADER}\n\n[colors.border]\nfocused = \"{}ff\"\nunfocused = \"{}ff\"\n",
        hex(c.accent),
        hex(c.raised),
    )
}

// Umbriel watches its own config file but not every file it includes, so it's asked to read them again. only while
// it's the compositor running: the command would fail anywhere else
fn reload_umbriel() {
    if std::env::var_os("UMBRIEL_SOCKET").is_none() {
        return;
    }

    let _ = Command::new("umbriel")
        .args(["msg", "config-reload"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

// only the base colours: the 16 terminal colours keep their usual hues, which programs rely on, apart from the black
// and white ones, which follow the background and the text
fn ghostty(c: &Colors) -> String {
    let lines = [
        ("background", c.background),
        ("foreground", c.text),
        ("cursor-color", c.accent),
        ("cursor-text", c.on_accent),
        ("selection-background", mix(c.background, c.accent, 0.35)),
        ("selection-foreground", c.text),
    ];

    let mut text = format!("# {HEADER}\n\n");

    for (key, color) in lines {
        text.push_str(&format!("{key} = {}\n", hex(color)));
    }

    for (index, color) in [(0, c.surface), (7, c.dim), (8, c.faint), (15, c.text)] {
        text.push_str(&format!("palette = {index}={}\n", hex(color)));
    }

    text
}

// a KDE colour scheme: each group of widgets (windows, the file view, buttons, the selection...) gets the same set of
// roles, as "red,green,blue"
fn kde(c: &Colors) -> String {
    let rgb = |color: Color| format!("{},{},{}", color.red(), color.green(), color.blue());

    let group = |name: &str, background: Color, alternate: Color, text: Color| {
        format!(
            "[Colors:{name}]\nBackgroundNormal={}\nBackgroundAlternate={}\nForegroundNormal={}\nForegroundInactive={}\n\
             ForegroundActive={}\nForegroundLink={}\nForegroundVisited={}\nForegroundNegative={}\nForegroundNeutral={}\n\
             ForegroundPositive={}\nDecorationFocus={}\nDecorationHover={}\n\n",
            rgb(background),
            rgb(alternate),
            rgb(text),
            rgb(c.dim),
            rgb(c.accent),
            rgb(c.blue),
            rgb(c.purple),
            rgb(c.red),
            rgb(c.yellow),
            rgb(c.green),
            rgb(c.accent),
            rgb(c.accent),
        )
    };

    let mut text = format!("# {HEADER}\n\n[General]\nColorScheme=Daru\nName=Daru\n\n");

    text.push_str(&group("Window", c.background, c.surface, c.text));
    text.push_str(&group("View", c.background, c.surface, c.text));
    text.push_str(&group("Button", c.surface, c.raised, c.text));
    text.push_str(&group("Header", c.surface, c.background, c.text));
    text.push_str(&group("Tooltip", c.raised, c.surface, c.text));
    text.push_str(&group("Complementary", c.surface, c.background, c.text));
    text.push_str(&group("Selection", c.accent, mix(c.accent, c.background, 0.3), c.on_accent));

    text.push_str(&format!(
        "[WM]\nactiveBackground={}\nactiveForeground={}\ninactiveBackground={}\ninactiveForeground={}\n",
        rgb(c.surface),
        rgb(c.text),
        rgb(c.background),
        rgb(c.dim),
    ));

    text
}

/*
 * kdeglobals with the scheme's colour groups in place of any it had, and its [General] ColorScheme pointing at it,
 * the way Plasma applies a scheme. everything else in the file (icons, previews, the terminal) is kept as it was.
 * without these groups KDE apps fall back to the light default for text, dark on a dark window
 */
fn with_colors(globals: &str, scheme: &str) -> String {
    let colour_group = |header: &str| header.starts_with("[Colors:") || header.starts_with("[ColorEffects:") || header == "[WM]";

    // split into sections, each its header line and the lines after it. lines before the first header have ""
    let sections = |text: &str| {
        let mut sections: Vec<(String, Vec<String>)> = vec![(String::new(), Vec::new())];

        for line in text.lines() {
            if line.starts_with('[') {
                sections.push((line.trim().to_string(), Vec::new()));
            } else if let Some((_, lines)) = sections.last_mut() {
                lines.push(line.to_string());
            }
        }

        sections
    };

    let mut kept: Vec<(String, Vec<String>)> = sections(globals).into_iter().filter(|(header, _)| !colour_group(header)).collect();

    // [General] ColorScheme=Daru, in the existing [General] or a new one
    let general = match kept.iter().position(|(header, _)| header == "[General]") {
        Some(index) => index,
        None => {
            kept.push((String::from("[General]"), Vec::new()));
            kept.len() - 1
        }
    };

    let lines = &mut kept[general].1;

    lines.retain(|line| !line.starts_with("ColorScheme="));
    lines.insert(0, String::from("ColorScheme=Daru"));

    let mut text = String::new();

    for (header, lines) in kept.iter().chain(sections(scheme).iter().filter(|(header, _)| colour_group(header))) {
        if !header.is_empty() {
            text.push_str(header);
            text.push('\n');
        }

        // one blank line between sections, however many there were
        let body: Vec<&String> = lines.iter().filter(|line| !line.trim().is_empty()).collect();

        for line in body {
            text.push_str(line);
            text.push('\n');
        }

        if !header.is_empty() || !lines.iter().all(|line| line.trim().is_empty()) {
            text.push('\n');
        }
    }

    text
}

/*
 * the lock KDE apps take before writing a config file (KConfig, through Qt's QLockFile): "<file>.lock", created only if
 * it doesn't exist, holding the process id, the program and the host. one that's still there after a second is taken
 * to be left over from a crash, and the file is written all the same
 */
struct KdeLock(Option<PathBuf>);

impl KdeLock {
    fn take(file: &Path) -> Self {
        let mut name = file.as_os_str().to_owned();
        name.push(".lock");

        let lock = PathBuf::from(name);

        let host = std::fs::read_to_string("/proc/sys/kernel/hostname").unwrap_or_default();

        for _ in 0..50 {
            let created = std::fs::OpenOptions::new().write(true).create_new(true).open(&lock);

            if let Ok(mut created) = created {
                let _ = std::io::Write::write_all(&mut created, format!("{}\ndaru\n{}\n", std::process::id(), host.trim()).as_bytes());

                return Self(Some(lock));
            }

            std::thread::sleep(std::time::Duration::from_millis(20));
        }

        Self(None)
    }
}

impl Drop for KdeLock {
    fn drop(&mut self) {
        if let Some(lock) = &self.0 {
            let _ = std::fs::remove_file(lock);
        }
    }
}

// what Plasma sends when the colour scheme changes: Qt apps using KDE's platform theme read kdeglobals again.
// (KGlobalSettings::notifyChange, with PaletteChanged, which is 0)
fn palette_changed() {
    Bus::session().emit("/KGlobalSettings", "org.kde.KGlobalSettings", "notifyChange", &[Argument::Int(0), Argument::Int(0)]);
}

// Ghostty reloads its config when asked over D-Bus, but only while it's running: asking would otherwise start it
fn reload_ghostty() {
    const NAME: &str = "com.mitchellh.ghostty";

    let running = Bus::session()
        .call("org.freedesktop.DBus", "/org/freedesktop/DBus", "org.freedesktop.DBus", "NameHasOwner", &[Argument::Text(NAME.to_string())])
        .list()
        .first()
        .is_some_and(Value::bool);

    if !running {
        return;
    }

    // its actions take an empty list and dictionary, which gdbus writes out more simply than Amane's arguments
    let _ = Command::new("gdbus")
        .args(["call", "--session", "--dest", NAME, "--object-path", "/com/mitchellh/ghostty"])
        .args(["--method", "org.gtk.Actions.Activate", "reload-config", "[]", "{}"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

/*
 * Strata's nine colours, plus its code preview's. "muted" fills raised things like switches and table headers,
 * "highlight" the hovered and selected rows
 */
fn strata(c: &Colors) -> String {
    let lines = [
        ("background", c.background),
        ("surface", c.surface),
        ("text", c.text),
        ("accent", c.accent),
        ("danger", c.red),
        ("muted", c.raised),
        ("highlight", mix(c.background, c.accent, 0.35)),
        ("border", c.border),
        ("dim_text", c.dim),
        ("syntax_keyword", c.purple),
        ("syntax_string", c.green),
        ("syntax_constant", c.orange),
        ("syntax_type", c.yellow),
        ("syntax_preprocessor", c.blue),
    ];

    let mut text = format!("# {HEADER}\n\nname = \"Daru\"\n");

    for (key, color) in lines {
        text.push_str(&format!("{key} = \"{}\"\n", hex(color)));
    }

    text
}

// where the login screen reads from. made by sddm/install.sh, owned by the user and readable by SDDM's greeter
const SDDM_DIR: &str = "/var/lib/daru-sddm";

// the wallpaper, blurred, and the colours, for the login screen to show on the next boot or logout
fn login_screen(c: &Colors) {
    let folder = Path::new(SDDM_DIR);

    // not set up on this machine
    if !folder.is_dir() {
        return;
    }

    let shown = login_wallpaper(folder);

    write(&folder.join("theme.conf.user"), &sddm(c, shown.as_deref()));
}

// where ReGreet's wallpaper and stylesheet go. made by greetd/install.sh, owned by the user, readable by the greeter
const GREETER_DIR: &str = "/var/lib/daru-greeter";

// the same for ReGreet under greetd: greetd/regreet.toml reads wallpaper.jpg, greetd/daru-greeter the stylesheet
fn greeter(c: &Colors) {
    let folder = Path::new(GREETER_DIR);

    if !folder.is_dir() {
        return;
    }

    login_wallpaper(folder);

    write(&folder.join("regreet.css"), &regreet(c));
}

/*
 * the blurred wallpaper as `folder`/wallpaper.jpg, or the one already there when this wallpaper has no blurred copy
 * yet. none when there's neither, and the login screen shows its background colour
 */
fn login_wallpaper(folder: &Path) -> Option<PathBuf> {
    /*
     * it's only seen at the next logout, so the blur below waits for the wallpaper's fade to finish rather than
     * taking the cores the fade is drawn with. a fade takes under half a second; this gives up waiting after two
     */
    for _ in 0..40 {
        if crate::wallpaper::Wallpaper::read().incoming.is_none() {
            break;
        }

        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let chosen = crate::wallpaper::Wallpaper::read().path.clone();

    let picture = folder.join("wallpaper.jpg");

    let copied = crate::wallpapers::make_blurred(&chosen).is_some_and(|blurred| copy(Path::new(&blurred), &picture));

    // the previous wallpaper stays when this one has no copy yet, the colours always follow
    (copied || picture.is_file()).then_some(picture)
}

// copied under another name and renamed, like `write`, and only when it changed
fn copy(from: &Path, to: &Path) -> bool {
    let Ok(new) = std::fs::read(from) else { return false };

    if std::fs::read(to).is_ok_and(|current| current == new) {
        return true;
    }

    let partial = to.with_extension("daru-part");

    std::fs::write(&partial, &new).is_ok() && std::fs::rename(&partial, to).is_ok()
}

// the keys sddm/Main.qml reads. colours are quoted, or the # would start a comment
fn sddm(c: &Colors, wallpaper: Option<&Path>) -> String {
    let lines = [
        ("backgroundColor", c.background),
        ("surface", c.surface),
        ("border", c.border),
        ("text", c.text),
        ("dim", c.dim),
        ("faint", c.faint),
        ("accent", c.accent),
        ("onAccent", c.on_accent),
        ("danger", c.red),
    ];

    let mut text = format!("# {HEADER}\n\n[General]\n");

    text.push_str(&format!("background={}\n", wallpaper.map(|path| path.display().to_string()).unwrap_or_default()));

    for (key, color) in lines {
        text.push_str(&format!("{key}=\"{}\"\n", hex(color)));
    }

    text
}

/*
 * ReGreet's stylesheet: the wallpaper dimmed like the lock screen's, a dark see-through card, quiet fields that light
 * up in the accent, and the power buttons as plain text until hovered. square corners everywhere, like the rest of
 * the shell. ReGreet is plain GTK4 on Adwaita, so these style its widgets by type
 */
fn regreet(c: &Colors) -> String {
    let colors = [
        ("daru_bg", c.background),
        ("daru_surface", c.surface),
        ("daru_text", c.text),
        ("daru_dim", c.dim),
        ("daru_faint", c.faint),
        ("daru_accent", c.accent),
        ("daru_on_accent", c.on_accent),
        ("daru_danger", c.red),
    ];

    let mut text = format!("/* {HEADER} */\n\n");

    for (name, color) in colors {
        text.push_str(&format!("@define-color {name} {};\n", hex(color)));
    }

    text.push_str(REGREET_STYLE);

    text
}

const REGREET_STYLE: &str = r#"
/* Adwaita rounds the card, fields, buttons, menus and focus rings */
* {
    border-radius: 0;
}

window {
    background-color: black;
    color: @daru_text;
}

/* the wallpaper, dimmed */
picture {
    opacity: 0.52;
}

frame, frame > border {
    background-color: alpha(@daru_bg, 0.72);
    border: 1px solid alpha(@daru_text, 0.08);
    box-shadow: none;
}

label {
    color: @daru_dim;
}

entry, button {
    background-color: alpha(black, 0.35);
    background-image: none;
    color: @daru_text;
    border: 1px solid alpha(@daru_text, 0.18);
    box-shadow: none;
    outline-color: alpha(@daru_accent, 0.6);
}

button label {
    color: inherit;
}

entry:focus-within, button:hover {
    border-color: alpha(@daru_accent, 0.6);
}

button.suggested-action {
    background-color: @daru_accent;
    color: @daru_on_accent;
    border-color: @daru_accent;
}

button.destructive-action {
    background-color: transparent;
    color: @daru_faint;
    border-color: transparent;
}

button.destructive-action:hover {
    background-color: alpha(@daru_text, 0.12);
    color: @daru_text;
}

popover > contents {
    background-color: @daru_surface;
    color: @daru_text;
}

label.error {
    color: @daru_danger;
}
"#;

/*
 * btop's colours: quiet box lines, the accent for what's highlighted, and graphs that go from calm to the warning
 * colours as they fill. each gradient starts close to the background, so an idle graph stays out of the way
 */
fn btop(c: &Colors) -> String {
    let rise = |color: Color| [mix(c.background, color, 0.4), mix(c.background, color, 0.7), color];
    let heat = [c.accent, c.yellow, c.red];

    let solid = [
        ("main_bg", c.background),
        ("main_fg", c.text),
        ("title", c.text),
        ("hi_fg", c.accent),
        ("selected_bg", mix(c.background, c.accent, 0.35)),
        ("selected_fg", c.text),
        ("inactive_fg", c.faint),
        ("graph_text", c.dim),
        ("meter_bg", c.raised),
        ("proc_misc", c.accent),
        ("cpu_box", c.faint),
        ("mem_box", c.faint),
        ("net_box", c.faint),
        ("proc_box", c.faint),
        ("div_line", c.border),
        ("followed_bg", mix(c.background, c.accent, 0.35)),
        ("followed_fg", c.text),
        ("proc_follow_bg", mix(c.background, c.accent, 0.35)),
        ("proc_pause_bg", c.red),
        ("proc_banner_bg", c.raised),
        ("proc_banner_fg", c.text),
    ];

    let mut lines: Vec<(String, Color)> = solid.into_iter().map(|(key, color)| (key.to_string(), color)).collect();

    for (name, stops) in [
        ("temp", heat),
        ("cpu", heat),
        ("free", rise(c.green)),
        ("cached", rise(c.blue)),
        ("available", rise(c.purple)),
        ("used", [c.yellow, c.orange, c.red]),
        ("download", rise(c.accent)),
        ("upload", rise(c.purple)),
        ("process", heat),
    ] {
        for (step, color) in ["start", "mid", "end"].into_iter().zip(stops) {
            lines.push((format!("{name}_{step}"), color));
        }
    }

    let mut text = format!("# {HEADER}\n\n");

    for (key, color) in lines {
        text.push_str(&format!("theme[{key}]=\"{}\"\n", hex(color)));
    }

    text
}

// btop reads its config and theme again on SIGUSR2. nothing happens when it isn't running
fn reload_btop() {
    let _ = Command::new("pkill")
        .args(["-USR2", "-x", "btop"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

fn zed(c: &Colors) -> String {
    let h = |color: Color| hex(color);

    // a colour with transparency, "#rrggbbaa"
    let a = |color: Color, alpha: f32| format!("{}{:02x}", hex(color), (alpha * 255.0).round() as u8);

    let syntax = |color: Color| format!("{{ \"color\": \"{}\" }}", h(color));

    format!(
        r#"{{
  "$schema": "https://zed.dev/schema/themes/v0.2.0.json",
  "name": "Daru",
  "author": "{HEADER}",
  "themes": [
    {{
      "name": "Daru",
      "appearance": "dark",
      "style": {{
        "background": "{background}",
        "surface.background": "{surface}",
        "elevated_surface.background": "{raised}",
        "panel.background": "{surface}",
        "status_bar.background": "{background}",
        "title_bar.background": "{background}",
        "title_bar.inactive_background": "{background}",
        "toolbar.background": "{background}",
        "tab_bar.background": "{surface}",
        "tab.active_background": "{background}",
        "tab.inactive_background": "{surface}",
        "editor.background": "{background}",
        "editor.foreground": "{text}",
        "editor.gutter.background": "{background}",
        "editor.active_line.background": "{active_line}",
        "editor.line_number": "{faint}",
        "editor.active_line_number": "{text}",
        "editor.indent_guide": "{border}",
        "editor.indent_guide_active": "{faint}",
        "border": "{border}",
        "border.variant": "{border}",
        "border.focused": "{accent}",
        "border.selected": "{accent}",
        "text": "{text}",
        "text.muted": "{dim}",
        "text.placeholder": "{faint}",
        "text.accent": "{accent}",
        "icon": "{text}",
        "icon.muted": "{dim}",
        "icon.accent": "{accent}",
        "element.background": "{surface}",
        "element.hover": "{raised}",
        "element.selected": "{selected}",
        "element.active": "{selected}",
        "ghost_element.hover": "{raised}",
        "ghost_element.selected": "{selected}",
        "ghost_element.active": "{selected}",
        "scrollbar.thumb.background": "{scrollbar}",
        "scrollbar.track.background": "{background}",
        "search.match_background": "{search}",
        "error": "{red}",
        "warning": "{yellow}",
        "success": "{green}",
        "info": "{blue}",
        "created": "{green}",
        "modified": "{yellow}",
        "deleted": "{red}",
        "terminal.background": "{background}",
        "terminal.foreground": "{text}",
        "accents": ["{accent}"],
        "players": [
          {{ "cursor": "{accent}", "background": "{accent}", "selection": "{selection}" }}
        ],
        "syntax": {{
          "keyword": {purple_syntax},
          "function": {blue_syntax},
          "string": {green_syntax},
          "number": {orange_syntax},
          "constant": {orange_syntax},
          "boolean": {orange_syntax},
          "type": {yellow_syntax},
          "attribute": {yellow_syntax},
          "tag": {red_syntax},
          "property": {text_syntax},
          "variable": {text_syntax},
          "punctuation": {dim_syntax},
          "operator": {dim_syntax},
          "title": {accent_syntax},
          "link_uri": {accent_syntax},
          "comment": {{ "color": "{faint}", "font_style": "italic" }}
        }}
      }}
    }}
  ]
}}
"#,
        background = h(c.background),
        surface = h(c.surface),
        raised = h(c.raised),
        active_line = a(c.raised, 0.5),
        faint = h(c.faint),
        border = h(c.border),
        accent = h(c.accent),
        text = h(c.text),
        dim = h(c.dim),
        selected = a(c.accent, 0.25),
        scrollbar = a(c.text, 0.2),
        search = a(c.accent, 0.3),
        selection = a(c.accent, 0.3),
        red = h(c.red),
        yellow = h(c.yellow),
        green = h(c.green),
        blue = h(c.blue),
        purple_syntax = syntax(c.purple),
        blue_syntax = syntax(c.blue),
        green_syntax = syntax(c.green),
        orange_syntax = syntax(c.orange),
        yellow_syntax = syntax(c.yellow),
        red_syntax = syntax(c.red),
        text_syntax = syntax(c.text),
        dim_syntax = syntax(c.dim),
        accent_syntax = syntax(c.accent),
    )
}

#[cfg(test)]
mod tests {
    use super::{Color, gnome_accent};

    #[test]
    fn accents_land_on_the_nearest_gnome_colour() {
        assert_eq!(gnome_accent(Color::rgb(0xea, 0x70, 0x7c)), "red"); // a pink-red
        assert_eq!(gnome_accent(Color::rgb(0x6a, 0xb4, 0xf0)), "blue");
        assert_eq!(gnome_accent(Color::rgb(0xcc, 0xd2, 0x89)), "yellow"); // an olive yellow
        assert_eq!(gnome_accent(Color::rgb(0x60, 0xc0, 0x70)), "green");
        assert_eq!(gnome_accent(Color::rgb(0x9a, 0x60, 0xd0)), "purple");
        assert_eq!(gnome_accent(Color::rgb(0x30, 0xa8, 0xb8)), "teal");
    }

    #[test]
    fn kdeglobals_keeps_its_settings_and_gets_the_colours() {
        let globals = "[General]\nTerminalApplication=ghostty\nColorScheme=Old\n\n[Icons]\nTheme=breeze-dark\n\n[Colors:View]\nForegroundNormal=1,2,3\n";
        let scheme = "# header\n\n[General]\nColorScheme=Daru\nName=Daru\n\n[Colors:View]\nForegroundNormal=236,232,237\n\n[WM]\nactiveBackground=1,1,1\n";

        let merged = super::with_colors(globals, scheme);

        assert!(merged.contains("TerminalApplication=ghostty"));
        assert!(merged.contains("[Icons]\nTheme=breeze-dark"));
        assert!(merged.contains("ColorScheme=Daru") && !merged.contains("ColorScheme=Old"));
        assert!(merged.contains("ForegroundNormal=236,232,237") && !merged.contains("ForegroundNormal=1,2,3"));
        assert!(merged.contains("[WM]"));
        assert!(!merged.contains("Name=Daru"), "the scheme's own [General] stays out of kdeglobals");
        assert_eq!(merged.matches("[General]").count(), 1);
    }

    #[test]
    fn a_symlinked_file_keeps_its_link_and_kde_locks_are_waited_for() {
        let folder = std::env::temp_dir().join(format!("daru-recolor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&folder);
        std::fs::create_dir_all(&folder).unwrap();

        let (target, link) = (folder.join("dotfiles-kdeglobals"), folder.join("kdeglobals"));
        std::fs::write(&target, "old").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();

        assert!(super::write(&link, "new"));
        assert!(link.is_symlink());
        assert_eq!(std::fs::read_to_string(&target).unwrap(), "new");
        assert!(!super::write(&link, "new"), "unchanged, so nothing to reload");

        // held while taken, gone once dropped, and a second taker waits it out
        let lock = folder.join("kdeglobals.lock");
        let held = super::KdeLock::take(&link);
        assert!(lock.exists() && held.0.is_some());

        let waiting = std::thread::spawn(move || super::KdeLock::take(&folder.join("kdeglobals")).0.is_some());
        std::thread::sleep(std::time::Duration::from_millis(100));
        drop(held);

        assert!(waiting.join().unwrap());
        assert!(!lock.exists());

        let _ = std::fs::remove_dir_all(lock.parent().unwrap());
    }

    #[test]
    fn the_strata_theme_has_all_nine_colours() {
        let theme = super::strata(&super::Colors::from(&crate::theme::derive(Color::rgb(20, 24, 30), Color::rgb(230, 230, 230), Color::rgb(0xea, 0x70, 0x7c))));

        assert!(theme.contains("name = \"Daru\""));

        for key in ["background", "surface", "text", "accent", "danger", "muted", "highlight", "border", "dim_text"] {
            let line = theme.lines().find(|line| line.starts_with(&format!("{key} = "))).unwrap_or_else(|| panic!("no {key}"));
            let value = line.split('"').nth(1).unwrap();

            assert!(value.len() == 7 && value.starts_with('#'), "{line}");
        }
    }

    #[test]
    fn the_login_screen_gets_every_colour_main_qml_reads() {
        let colors = super::Colors::from(&crate::theme::derive(Color::rgb(20, 24, 30), Color::rgb(230, 230, 230), Color::rgb(0xea, 0x70, 0x7c)));

        let conf = super::sddm(&colors, Some(std::path::Path::new("/var/lib/daru-sddm/wallpaper.jpg")));

        assert!(conf.contains("[General]\n"));
        assert!(conf.contains("background=/var/lib/daru-sddm/wallpaper.jpg\n"));

        // quoted, or QSettings would read the # as the start of a comment
        for key in ["backgroundColor", "surface", "border", "text", "dim", "faint", "accent", "onAccent", "danger"] {
            let line = conf.lines().find(|line| line.starts_with(&format!("{key}="))).unwrap_or_else(|| panic!("no {key}"));
            let value = line.split_once('=').unwrap().1;

            assert!(value.len() == 9 && value.starts_with("\"#") && value.ends_with('"'), "{line}");
        }
    }

    #[test]
    fn without_a_wallpaper_the_login_screen_shows_the_background_colour() {
        let colors = super::Colors::from(&crate::theme::derive(Color::rgb(20, 24, 30), Color::rgb(230, 230, 230), Color::rgb(0xea, 0x70, 0x7c)));

        assert!(super::sddm(&colors, None).contains("\nbackground=\n"));
    }

    #[test]
    fn the_greeter_stylesheet_defines_every_colour_it_uses() {
        let colors = super::Colors::from(&crate::theme::derive(Color::rgb(20, 24, 30), Color::rgb(230, 230, 230), Color::rgb(0xea, 0x70, 0x7c)));

        let css = super::regreet(&colors);

        // GTK drops a rule that names a colour nobody defined
        for used in css.split('@').skip(1).filter_map(|rest| rest.split(|c: char| !c.is_alphanumeric() && c != '_').next()) {
            if used != "define" {
                assert!(css.contains(&format!("@define-color {used} #")), "@{used} isn't defined");
            }
        }
    }

    #[test]
    fn the_btop_theme_sets_every_colour_and_gradient() {
        let colors = super::Colors::from(&crate::theme::derive(Color::rgb(20, 24, 30), Color::rgb(230, 230, 230), Color::rgb(0xea, 0x70, 0x7c)));

        let theme = super::btop(&colors);

        for key in ["main_bg", "main_fg", "hi_fg", "selected_bg", "div_line", "proc_banner_fg", "cpu_start", "cpu_mid", "cpu_end", "used_end", "process_start"] {
            let line = theme.lines().find(|line| line.starts_with(&format!("theme[{key}]="))).unwrap_or_else(|| panic!("no {key}"));
            let value = line.split_once('=').unwrap().1;

            assert!(value.len() == 9 && value.starts_with("\"#") && value.ends_with('"'), "{line}");
        }

        // 21 plain colours and nine gradients of three
        assert_eq!(theme.lines().filter(|line| line.starts_with("theme[")).count(), 21 + 27);
    }

    #[test]
    fn umbriel_gets_both_border_colours_with_alpha() {
        let colors = super::Colors::from(&crate::theme::derive(Color::rgb(20, 24, 30), Color::rgb(230, 230, 230), Color::rgb(0xea, 0x70, 0x7c)));

        let file = super::umbriel(&colors);

        assert!(file.contains("[colors.border]\n"));

        for key in ["focused", "unfocused"] {
            let line = file.lines().find(|line| line.starts_with(&format!("{key} = "))).unwrap_or_else(|| panic!("no {key}"));

            assert_eq!(line.len(), key.len() + 3 + 11, "{line}");
        }
    }

    #[test]
    fn greys_become_slate() {
        assert_eq!(gnome_accent(Color::rgb(0x88, 0x88, 0x8c)), "slate");
    }
}
