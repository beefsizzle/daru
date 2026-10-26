# daru

A desktop shell for [niri](https://github.com/YaLTeR/niri), built with [Amane](https://github.com/MystiaFin/amane). Bar, tray,
dashboard, launcher, notifications, calendar and weather, wallpaper picker, and sound, Bluetooth, network, clipboard and
power dropdowns. Colours follow the wallpaper.

![screenshot](media/screenshot.jpg)





https://github.com/user-attachments/assets/a843d4c3-b076-4d88-abff-8120f4c5d30a






## Requirements

- niri
- the Amane CLI from [this fork](https://github.com/beefsizzle/amane), for fingerprint unlock, a compositor-drawn
  wallpaper and lower GPU memory. With upstream Amane, use the [`upstream`](https://github.com/beefsizzle/daru/tree/upstream)
  branch instead, which does without those
- Adwaita Sans, JetBrainsMono Nerd Font
- ImageMagick, wl-clipboard, cliphist, `pactl` (PipeWire or PulseAudio), NetworkManager, BlueZ, curl, jq
- optional: fprintd with an enrolled finger, to unlock the lock screen with a fingerprint

## Install

```sh
git clone https://github.com/beefsizzle/amane
cd amane && ./install.sh && cd ..

git clone https://github.com/beefsizzle/daru
amane target daru
amane compile
amane run
```

Stop any other bar, notification daemon or tray host first.

## niri

```kdl
spawn-at-startup "amane" "run"
spawn-at-startup "wl-paste" "--type" "text" "--watch" "cliphist" "-max-items" "200" "store"
spawn-at-startup "wl-paste" "--type" "image" "--watch" "cliphist" "-max-items" "200" "store"

binds {
    Mod+Space   { spawn "amane" "ipc" "call" "toggle" "launcher"; }
    Mod+Shift+D { spawn "amane" "ipc" "call" "toggle" "dashboard"; }
    Mod+Shift+C { spawn "amane" "ipc" "call" "toggle" "calendar"; }
    Mod+Shift+N { spawn "amane" "ipc" "call" "toggle" "notifications"; }
    Mod+Shift+W { spawn "amane" "ipc" "call" "toggle" "wallpapers"; }
    Mod+Ctrl+V  { spawn "amane" "ipc" "call" "toggle" "clipboard"; }
    Alt+X       { spawn "amane" "ipc" "call" "toggle" "power"; }
    Super+Alt+L { spawn "amane" "ipc" "call" "lock"; }
}
```

Blur behind the bar and panels: add [`dev/niri-layer-rules.kdl`](dev/niri-layer-rules.kdl).

## Umbriel

It runs on [Umbriel](https://docs.noctalia.dev/umbriel/) too. Screenshots there need `grim` and `slurp`, and `satty` to draw on one. In
`~/.config/umbriel/config.toml`:

```toml
[general]
autostart = ["amane run"]

[keybinds]
"Mod+Space" = "spawn:amane ipc call toggle launcher"
"Mod+Alt+L" = "spawn:amane ipc call lock"
"Print" = "spawn:amane ipc call screenshot region"

[[layer_rule]]
match.namespace = "^amane-(bar|panel|dock)$"
blur = true
```

## Other apps' colours

The shell can write the wallpaper colours for other apps, live where the app allows it. List the apps in
`~/.config/amane-shell/recolor`, one per line, and point each app at its file once:

| app | file it writes | add to the app's config |
|---|---|---|
| `gtk` (GTK4 / libadwaita) | `~/.config/gtk-4.0/daru.css` (read when an app starts), and GNOME's `accent-color` (live, nearest of its 9) | `@import url("daru.css");` in `~/.config/gtk-4.0/gtk.css` |
| `niri` (focus ring) | `~/.config/niri/daru-colors.kdl` | `include "daru-colors.kdl"` at the end of `config.kdl` |
| `umbriel` (window borders) | `~/.config/umbriel/daru-colors.toml` (live) | `[include.optional] files = ["daru-colors.toml"]` in `~/.config/umbriel/config.toml` |
| `ghostty` | `~/.config/ghostty/daru-colors` | `config-file = ?daru-colors` |
| `zed` | `~/.config/zed/themes/daru.json` | pick the "Daru" theme |
| `kde` (Dolphin, Qt apps) | `~/.local/share/color-schemes/Daru.colors`, and the colours in `~/.config/kdeglobals` (live) | install `plasma-integration` and set `QT_QPA_PLATFORMTHEME=kde` |
| `strata` ([Strata](https://github.com/lgse/strata)) | `~/.config/strata/themes/daru.toml` (live) | pick the "Daru" theme |
| `btop` | `~/.config/btop/themes/daru.theme` (live) | `color_theme = "daru"` in `~/.config/btop/btop.conf` |
| `sddm` (login screen) | the wallpaper and colours in `/var/lib/daru-sddm` | run `sudo sddm/install.sh` once (below) |
| `regreet` (login screen under greetd) | the wallpaper and a stylesheet in `/var/lib/daru-greeter` | run `sudo greetd/install.sh` once (below) |

## Login screen

`sddm/` is the lock screen as an [SDDM](https://github.com/sddm/sddm) theme, showing the wallpaper and colours you
last had. SDDM's greeter runs as its own user and can't read your home folder, so the shell copies them to
`/var/lib/daru-sddm` when the wallpaper changes. Set it up once per machine:

```sh
sudo sddm/install.sh
echo sddm >> ~/.config/amane-shell/recolor
```

then pick a wallpaper. To look at it without logging out: `sddm-greeter-qt6 --test-mode --theme /usr/share/sddm/themes/daru`.
`sudo sddm/install.sh --remove` puts the previous theme back.

`greetd/` is a lighter alternative: [greetd](https://sr.ht/~kennylevinsen/greetd/) runs
[ReGreet](https://github.com/rharish101/ReGreet) in [cage](https://github.com/cage-kiosk/cage), and all three are gone
again once you're logged in, while SDDM keeps its X server running for the whole session (about 190 MB with SDDM
itself on a laptop). ReGreet's layout is its own, so this styles it with the wallpaper and colours rather than
looking like the lock screen. Set it up once per machine, with `--scale` for a high-dpi screen (cage draws at scale 1
otherwise, and needs `wlr-randr` to change that):

```sh
sudo pacman -S greetd greetd-regreet cage wlr-randr
sudo greetd/install.sh --scale 1.5
echo regreet >> ~/.config/amane-shell/recolor
sudo systemctl disable sddm && sudo systemctl enable greetd
```

To look at it without logging out: `regreet --demo --style /var/lib/daru-greeter/regreet.css`.
`sudo greetd/install.sh --remove` puts greetd's own setup back, and `sudo systemctl disable greetd && sudo systemctl
enable sddm` the SDDM login screen.

## Config

In `~/.config/amane-shell/`:

- `location`: `lat lon`, for the weather
- `wallpaper-dir`: wallpaper folder, default `~/Pictures/Wallpapers`

## IPC

```
amane ipc call toggle dashboard|calendar|notifications|wallpapers|launcher|power|network|audio|bluetooth|clipboard|all|close
amane ipc call wallpaper <path>
amane ipc call screenshot region|screen|annotate
amane ipc call recolor
amane ipc call lock
amane ipc call sleep
```

## License

[MIT](LICENSE)
