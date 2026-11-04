use amane::{Color, Service};

use crate::theme::{SEEDS, Theme, alpha, mix, signal};

/*
 * named colour schemes, picked in the wallpaper picker, which replace the colours the shell takes from the wallpaper
 * until "Wallpaper" is picked again. unlike the wallpaper's colours, which only lend their hues to fixed lightnesses
 * (see theme::derive), a scheme is drawn with its own colours, so Nord looks like Nord. the choice is kept in
 * ~/.local/state/amane-shell/scheme, and `amane ipc call scheme <name>|wallpaper` picks one from a keybind
 */
pub struct Scheme {
    pub name: &'static str,
    background: Color,
    foreground: Color,
    accent: Color,
    // [red, yellow, green, blue, purple, orange] where the scheme has its own, otherwise they're made like the
    // wallpaper's: seed hues leaning toward the accent
    hues: Option<[Color; 6]>,
}

const fn hex(value: u32) -> Color {
    Color::rgb((value >> 16) as u8, (value >> 8) as u8, value as u8)
}

const fn scheme(name: &'static str, background: u32, foreground: u32, accent: u32) -> Scheme {
    Scheme { name, background: hex(background), foreground: hex(foreground), accent: hex(accent), hues: None }
}

const fn full(name: &'static str, background: u32, foreground: u32, accent: u32, hues: [u32; 6]) -> Scheme {
    let [red, yellow, green, blue, purple, orange] = hues;

    Scheme {
        name,
        background: hex(background),
        foreground: hex(foreground),
        accent: hex(accent),
        hues: Some([hex(red), hex(yellow), hex(green), hex(blue), hex(purple), hex(orange)]),
    }
}

pub const SCHEMES: &[Scheme] = &[
    scheme("Amberbyte", 0x1a1313, 0xede3e3, 0xd9636b),
    scheme("Anonymous", 0x050505, 0xe6e7e7, 0xd4d4d4),
    scheme("Ash", 0x1c1c1c, 0xc4c6c5, 0xa9abaa),
    scheme("Atari", 0x272027, 0xd6d0cc, 0x9fd1b8),
    scheme("Black Gold", 0x0e0e0c, 0xdfdcd0, 0xdeb953),
    full("Catppuccin", 0x1e1e2e, 0xcdd6f4, 0xcba6f7, [0xf38ba8, 0xf9e2af, 0xa6e3a1, 0x89b4fa, 0xf5c2e7, 0xfab387]),
    scheme("Ember & Ash", 0x1c1c1c, 0xd8cfc8, 0xe0896f),
    full("Everforest", 0x2d353b, 0xd3c6aa, 0xa7c080, [0xe67e80, 0xdbbc7f, 0xa7c080, 0x7fbbb3, 0xd699b6, 0xe69875]),
    scheme("Koyanagi", 0x161616, 0xc8c8c8, 0xd4b970),
    scheme("Lifeless", 0x1f3236, 0xd8deda, 0x53bea3),
    scheme("Lumon", 0x0c1c2c, 0xbcd4dc, 0xa8d8ec),
    scheme("Matte Black", 0x121212, 0xbebebe, 0xe8a030),
    full("Nord", 0x2e3440, 0xd8dee9, 0x88c0d0, [0xbf616a, 0xebcb8b, 0xa3be8c, 0x81a1c1, 0xb48ead, 0xd08770]),
    full("Tokyo Night", 0x1a1b26, 0xc0caf5, 0x7aa2f7, [0xf7768e, 0xe0af68, 0x9ece6a, 0x7aa2f7, 0xbb9af7, 0xff9e64]),
];

impl Scheme {
    pub fn theme(&self) -> Theme {
        let (background, fg, accent) = (self.background, self.foreground, self.accent);

        let [red, yellow, green, blue, purple, orange] = self.hues.unwrap_or_else(|| SEEDS.map(|seed| signal(seed, accent)));

        // chips and cards a step toward the text, as the wallpaper's are a step lighter than its background
        let raised = |amount: f32| mix(background, fg, amount);

        Theme {
            glass: alpha(background, 0.62),
            panel: alpha(background, 0.78),
            surface: alpha(raised(0.08), 0.80),
            surface_hi: alpha(raised(0.15), 0.90),
            border: alpha(fg, 0.10),
            fg,
            dim: alpha(fg, 0.62),
            faint: alpha(fg, 0.38),
            accent,
            accent_soft: alpha(accent, 0.22),
            on_accent: background,
            red,
            yellow,
            green,
            blue,
            purple,
            orange,
        }
    }
}

pub fn named(name: &str) -> Option<&'static Scheme> {
    SCHEMES.iter().find(|scheme| scheme.name.eq_ignore_ascii_case(name))
}

// the scheme in use, None for the wallpaper's colours
pub struct Chosen {
    pub scheme: Option<&'static Scheme>,
}

fn state_file() -> std::path::PathBuf {
    crate::cache::state_dir().join("scheme")
}

impl Service for Chosen {
    fn new() -> Self {
        let name = std::fs::read_to_string(state_file()).unwrap_or_default();

        Self { scheme: named(name.trim()) }
    }

    // it only changes when one is picked
    fn listen() {}
}

pub fn chosen() -> Option<&'static Scheme> {
    Chosen::read().scheme
}

/// picks a scheme, or the wallpaper's colours for None, remembers it for the next start, and recolours other apps
pub fn choose(scheme: Option<&'static Scheme>) {
    {
        let mut chosen = Chosen::write();

        if chosen.scheme.map(|s| s.name) == scheme.map(|s| s.name) {
            return;
        }

        chosen.scheme = scheme;
    }

    // a failed write only means it won't survive a restart
    let file = state_file();

    if let Some(folder) = file.parent() {
        let _ = std::fs::create_dir_all(folder);
    }

    let _ = std::fs::write(file, scheme.map_or("", |scheme| scheme.name));

    crate::recolor::export();
}

/// `amane ipc call scheme <name>`, `scheme wallpaper` for the wallpaper's colours, or `scheme` to list them
pub fn ipc(arguments: &[String]) -> String {
    let name = arguments.join(" ");

    if name.is_empty() {
        let names: Vec<&str> = SCHEMES.iter().map(|scheme| scheme.name).collect();

        return format!("wallpaper, {}", names.join(", "));
    }

    if name.eq_ignore_ascii_case("wallpaper") {
        choose(None);

        return String::from("scheme wallpaper");
    }

    match named(&name) {
        Some(scheme) => {
            choose(Some(scheme));

            format!("scheme {}", scheme.name)
        }
        None => format!("no scheme named {name}"),
    }
}

#[cfg(test)]
mod tests {
    use super::SCHEMES;
    use crate::theme::tests::contrast;

    #[test]
    fn every_scheme_reads() {
        for scheme in SCHEMES {
            let theme = scheme.theme();

            assert!(contrast(theme.fg, scheme.background) >= 7.0, "{} text {}", scheme.name, contrast(theme.fg, scheme.background));
            assert!(contrast(theme.accent, scheme.background) >= 4.5, "{} accent {}", scheme.name, contrast(theme.accent, scheme.background));
            assert!(contrast(theme.on_accent, theme.accent) >= 4.5, "{} on accent", scheme.name);
        }
    }

    #[test]
    fn names_are_unique_and_never_wallpaper() {
        for (index, scheme) in SCHEMES.iter().enumerate() {
            assert!(!scheme.name.eq_ignore_ascii_case("wallpaper"));
            assert!(SCHEMES[..index].iter().all(|other| !other.name.eq_ignore_ascii_case(scheme.name)));
        }
    }
}
