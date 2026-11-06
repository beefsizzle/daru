# daru

A desktop shell for [Umbriel](https://docs.noctalia.dev/umbriel/), built with [Amane](https://github.com/MystiaFin/amane).
Bar, tray, dashboard, launcher, notifications, lock screen, wallpaper picker and the usual dropdowns, all coloured
from the wallpaper, or from one of the colour schemes in the wallpaper picker.

It's a personal shell, not a plug-and-play one: expect to read the source if you want to try it. 

![screenshot](media/screenshot.jpg)



https://github.com/user-attachments/assets/4ad7ae44-b271-4ada-8535-0eeb0b0ae38e



## Running it

It needs the Amane CLI from [this fork](https://github.com/beefsizzle/amane), and uses ImageMagick, wl-clipboard,
cliphist, `pactl`, NetworkManager, BlueZ, curl, jq and grim.

```sh
git clone https://github.com/beefsizzle/amane
cd amane && ./install.sh && cd ..

git clone https://github.com/beefsizzle/daru
amane target daru
amane compile
amane run
```

Panels open with `amane ipc call toggle <name>` (see `src/panels.rs`). Other apps can follow the shell's colours
too (see `src/recolor.rs`), and `amane ipc call scheme <name>|wallpaper` switches the colour scheme from a keybind
(see `src/schemes.rs`). `greetd/` holds a login screen for greetd, with its own `install.sh`.

`umbriel/config.toml` is an example Umbriel config for daru: Umbriel's own starting config with daru starting at
login, its panels on the keybinds, and blur behind its bar and panels. Copy it to `~/.config/umbriel/config.toml`.

## LLM usage
Daru is written with the help of Claude through Claude Code. Much of the code comes from those sessions. Every change is still built, reviewed and used every day on both an intel/nvidia laptop and a full AMD desktop before it ships.

## License

[MIT](LICENSE)
