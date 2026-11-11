use amane::{Color, Palette, Service};

pub const TEXT_FONT: &str = "Adwaita Sans";
pub const ICON_FONT: &str = "JetBrainsMono Nerd Font";

// corner radii. the bar chips are square on purpose. Raise CHIP_RADIUS to round them again (13 is a full pill)
pub const CHIP_RADIUS: f32 = 0.0;
pub const RADIUS_SMALL: f32 = 0.0;

/*
 * every color the shell uses, derived from the wallpaper through `Palette`. they're listed in two groups, so that
 * `faded` can't miss a color added later:
 *  - background: `glass`, the translucent base behind the bar's blur, and `panel`, a popup's background, a little
 *    more opaque. they stay solid while a panel fades in
 *  - content, everything drawn on them, which fades: `surface` (chips and cards) and `surface_hi` (the same,
 *    hovered), `border` for hairlines, the text colors `fg`, `dim` and `faint`, `accent` with `accent_soft` (a tint
 *    for active chips) and `on_accent` (text on an accent fill), and `red` and `yellow` for warnings
 */
macro_rules! theme {
    (background { $($solid:ident),* $(,)? } content { $($fading:ident),* $(,)? }) => {
        #[derive(Clone, Copy)]
        pub struct Theme {
            $(pub $solid: Color,)*
            $(pub $fading: Color,)*
        }

        impl Theme {
            // the content colors through `fade`, the background ones as they are
            fn map_content(self, fade: impl Fn(Color) -> Color) -> Theme {
                Theme {
                    $($solid: self.$solid,)*
                    $($fading: fade(self.$fading),)*
                }
            }
        }
    };
}

theme! {
    background { glass, panel }
    content { surface, surface_hi, border, fg, dim, faint, accent, accent_soft, on_accent, red, yellow, green, blue, purple, orange }
}

impl Theme {
    /*
     * what's on a panel at `amount` of its opacity, for a panel fading in. done on the colours rather than with
     * `Rectangle::opacity`: Amane draws anything with an opacity below 1 on a texture of its own the size of the
     * window and lays that over the rest, while plain colours are drawn straight in. pictures (icons, thumbnails)
     * have no colour to fade, so they appear at once.
     *
     * the panel's own background (and the bar's glass) stays as it is: a compositor's blur can show the wallpaper
     * behind it rather than the window below, so a background that starts out clear flashes the wallpaper over that
     * window
     */
    pub fn faded(self, amount: f32) -> Theme {
        if amount >= 1.0 {
            return self;
        }

        self.map_content(|color| Color::rgba(color.red(), color.green(), color.blue(), (f32::from(color.alpha()) * amount.max(0.0)).round() as u8))
    }
}

pub fn alpha(color: Color, amount: f32) -> Color {
    Color::rgba(color.red(), color.green(), color.blue(), (amount.clamp(0.0, 1.0) * 255.0) as u8)
}

pub fn mix(a: Color, b: Color, t: f32) -> Color {
    let lerp = |x: u8, y: u8| (f32::from(x) + (f32::from(y) - f32::from(x)) * t).round() as u8;

    Color::rgba(
        lerp(a.red(), b.red()),
        lerp(a.green(), b.green()),
        lerp(a.blue(), b.blue()),
        lerp(a.alpha(), b.alpha()),
    )
}

/// the colours in use: a named scheme when one is picked (see schemes.rs), the wallpaper's otherwise
pub fn theme() -> Theme {
    crate::schemes::chosen().map_or_else(from_wallpaper, crate::schemes::Scheme::theme)
}

pub fn from_wallpaper() -> Theme {
    let palette = Palette::read();

    derive(palette.background(), palette.foreground(), palette.accent())
}

/*
 * every role has a fixed lightness, and the wallpaper only lends its hues (and some of its saturation). mixing the
 * wallpaper's own colours toward black and white makes the bar's depth and contrast depend on the picture: a wallpaper
 * whose darkest colour is a mid grey gives a light bar with weak text. this way every wallpaper reads the same and
 * only the colour changes
 */
const BACKGROUND: f32 = 0.075; // the glass and panels
const SURFACE: f32 = 0.15; // chips and cards
const SURFACE_HI: f32 = 0.22; // the same, hovered
const TEXT: f32 = 0.92;
const ACCENT: f32 = 0.68; // bright enough to read as text on the background
const ON_ACCENT: f32 = 0.12; // text on an accent fill
const SIGNAL: f32 = 0.70; // red and yellow

// a hue is only kept when there's colour to keep: a grey has none, and its hue would turn it red
const GREY: f32 = 0.08;

// the signal hues start from these (red, yellow, green, blue, purple, orange) and lean a little toward the accent, so
// they belong to the wallpaper's colours
pub const SEEDS: [Color; 6] = [
    Color::rgb(0xf7, 0x76, 0x8e),
    Color::rgb(0xe0, 0xaf, 0x68),
    Color::rgb(0x9e, 0xce, 0x6a),
    Color::rgb(0x7a, 0xa2, 0xf7),
    Color::rgb(0xbb, 0x9a, 0xf7),
    Color::rgb(0xff, 0x9e, 0x64),
];

pub fn derive(background: Color, foreground: Color, accent: Color) -> Theme {
    let base = |lightness: f32| relit(background, lightness, 0.0, 0.35);

    let fg = relit(foreground, TEXT, 0.0, 0.12);
    let accent_color = relit(accent, ACCENT, 0.45, 0.82);

    let [red, yellow, green, blue, purple, orange] = SEEDS.map(|seed| signal(seed, accent_color));

    Theme {
        glass: alpha(base(BACKGROUND), 0.62),
        panel: alpha(base(BACKGROUND), 0.78),
        surface: alpha(base(SURFACE), 0.80),
        surface_hi: alpha(base(SURFACE_HI), 0.90),
        border: alpha(fg, 0.10),
        fg,
        dim: alpha(fg, 0.62),
        faint: alpha(fg, 0.38),
        accent: accent_color,
        accent_soft: alpha(accent_color, 0.22),
        on_accent: relit(accent, ON_ACCENT, 0.0, 0.50),
        red,
        yellow,
        green,
        blue,
        purple,
        orange,
    }
}

/// a signal colour (warnings, or syntax colours for other apps) from a seed hue, leaning a little toward the accent and
/// as light as the red and yellow, so it reads on the background
pub fn signal(seed: Color, accent: Color) -> Color {
    relit(mix(seed, accent, 0.20), SIGNAL, 0.48, 0.82)
}

/// a translucent colour as it looks laid over `under`, for apps that want solid colours
pub fn over(color: Color, under: Color) -> Color {
    let solid = Color::rgb(color.red(), color.green(), color.blue());

    mix(under, solid, f32::from(color.alpha()) / 255.0)
}

/// "#rrggbb"
pub fn hex(color: Color) -> String {
    format!("#{:02x}{:02x}{:02x}", color.red(), color.green(), color.blue())
}

// the colour's hue at the given lightness, its saturation kept within `least..=most` (a grey stays grey)
fn relit(color: Color, lightness: f32, least: f32, most: f32) -> Color {
    let (hue, saturation, _) = to_hsl(color);

    let saturation = if saturation < GREY { saturation } else { saturation.clamp(least, most) };

    from_hsl(hue, saturation, lightness)
}

// hue in degrees, saturation and lightness 0..1
pub fn to_hsl(color: Color) -> (f32, f32, f32) {
    let [r, g, b] = [color.red(), color.green(), color.blue()].map(|channel| f32::from(channel) / 255.0);

    let (high, low) = (r.max(g).max(b), r.min(g).min(b));
    let lightness = (high + low) / 2.0;
    let spread = high - low;

    if spread == 0.0 {
        return (0.0, 0.0, lightness);
    }

    let saturation = spread / (1.0 - (2.0 * lightness - 1.0).abs());

    let hue = if high == r {
        60.0 * ((g - b) / spread).rem_euclid(6.0)
    } else if high == g {
        60.0 * ((b - r) / spread + 2.0)
    } else {
        60.0 * ((r - g) / spread + 4.0)
    };

    (hue, saturation.min(1.0), lightness)
}

fn from_hsl(hue: f32, saturation: f32, lightness: f32) -> Color {
    let chroma = (1.0 - (2.0 * lightness - 1.0).abs()) * saturation;
    let second = chroma * (1.0 - ((hue / 60.0).rem_euclid(2.0) - 1.0).abs());
    let offset = lightness - chroma / 2.0;

    let (r, g, b) = match hue.rem_euclid(360.0) {
        h if h < 60.0 => (chroma, second, 0.0),
        h if h < 120.0 => (second, chroma, 0.0),
        h if h < 180.0 => (0.0, chroma, second),
        h if h < 240.0 => (0.0, second, chroma),
        h if h < 300.0 => (second, 0.0, chroma),
        _ => (chroma, 0.0, second),
    };

    let channel = |value: f32| ((value + offset).clamp(0.0, 1.0) * 255.0).round() as u8;

    Color::rgb(channel(r), channel(g), channel(b))
}

#[cfg(test)]
pub mod tests {
    use super::{BACKGROUND, Color, derive, from_hsl, to_hsl};

    // WCAG contrast between two opaque colours
    pub fn contrast(a: Color, b: Color) -> f32 {
        let luminance = |color: Color| {
            let linear = |channel: u8| {
                let value = f32::from(channel) / 255.0;

                if value <= 0.04045 { value / 12.92 } else { ((value + 0.055) / 1.055).powf(2.4) }
            };

            0.2126 * linear(color.red()) + 0.7152 * linear(color.green()) + 0.0722 * linear(color.blue())
        };

        let (light, dark) = (luminance(a).max(luminance(b)), luminance(a).min(luminance(b)));

        (light + 0.05) / (dark + 0.05)
    }

    fn opaque(color: Color) -> Color {
        Color::rgb(color.red(), color.green(), color.blue())
    }

    // wallpapers of every kind: a dark desert, a pale grey, a vivid magenta, and a black and white one
    const WALLPAPERS: [(Color, Color, Color); 4] = [
        (Color::rgb(0x2a, 0x1a, 0x10), Color::rgb(0xf0, 0xd0, 0xa0), Color::rgb(0xe0, 0x90, 0x40)),
        (Color::rgb(0x88, 0x88, 0x8c), Color::rgb(0xee, 0xee, 0xee), Color::rgb(0x70, 0x90, 0xb0)),
        (Color::rgb(0x40, 0x00, 0x30), Color::rgb(0xff, 0xc0, 0xf0), Color::rgb(0xff, 0x00, 0xc0)),
        (Color::rgb(0x00, 0x00, 0x00), Color::rgb(0xff, 0xff, 0xff), Color::rgb(0x80, 0x80, 0x80)),
    ];

    #[test]
    fn hsl_round_trips() {
        for color in [Color::rgb(0xf7, 0x76, 0x8e), Color::rgb(0x12, 0x34, 0x56), Color::rgb(0x80, 0x80, 0x80), Color::rgb(0, 0xff, 0)] {
            let (hue, saturation, lightness) = to_hsl(color);
            let back = from_hsl(hue, saturation, lightness);

            for (a, b) in [(color.red(), back.red()), (color.green(), back.green()), (color.blue(), back.blue())] {
                assert!(a.abs_diff(b) <= 1, "{color:?} came back as {back:?}");
            }
        }
    }

    #[test]
    fn every_wallpaper_gets_the_same_depth() {
        for (background, foreground, accent) in WALLPAPERS {
            let (_, _, lightness) = to_hsl(opaque(derive(background, foreground, accent).panel));

            assert!((lightness - BACKGROUND).abs() < 0.01, "panel lightness {lightness}");
        }
    }

    #[test]
    fn text_and_accent_read_on_every_wallpaper() {
        for (background, foreground, accent) in WALLPAPERS {
            let theme = derive(background, foreground, accent);
            let panel = opaque(theme.panel);

            assert!(contrast(theme.fg, panel) >= 12.0, "text contrast {}", contrast(theme.fg, panel));
            assert!(contrast(theme.accent, panel) >= 6.0, "accent contrast {}", contrast(theme.accent, panel));
            assert!(contrast(theme.on_accent, theme.accent) >= 4.5, "on-accent contrast {}", contrast(theme.on_accent, theme.accent));
            assert!(contrast(theme.red, panel) >= 4.5, "red contrast {}", contrast(theme.red, panel));
        }
    }

    #[test]
    fn fading_scales_every_colour() {
        let theme = derive(WALLPAPERS[0].0, WALLPAPERS[0].1, WALLPAPERS[0].2);
        let half = theme.faded(0.5);

        assert_eq!(half.fg.alpha(), 128);
        assert_eq!(half.surface.alpha(), (f32::from(theme.surface.alpha()) * 0.5).round() as u8);

        // the background stays, so the blur behind it never shows through clear
        assert_eq!(half.panel.alpha(), theme.panel.alpha());
        assert_eq!(theme.faded(0.0).accent.alpha(), 0);
        assert_eq!(theme.faded(1.0).border.alpha(), theme.border.alpha());
    }

    #[test]
    fn a_grey_wallpaper_stays_grey() {
        let theme = derive(Color::rgb(0x50, 0x50, 0x50), Color::rgb(0xdd, 0xdd, 0xdd), Color::rgb(0x99, 0x99, 0x99));
        let panel = opaque(theme.panel);

        assert!(panel.red() == panel.green() && panel.green() == panel.blue(), "{panel:?}");
    }
}
