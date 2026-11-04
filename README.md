# daru

A desktop shell for [Umbriel](https://docs.noctalia.dev/umbriel/), built with [Amane](https://github.com/MystiaFin/amane).
Bar, tray, dashboard, launcher, notifications, lock screen, wallpaper picker and the usual dropdowns, all coloured
from the wallpaper.

It's a personal shell, not a plug-and-play one: expect to read the source if you want to try it. 

![screenshot](media/screenshot.jpg)



https://github.com/user-attachments/assets/4ad7ae44-b271-4ada-8535-0eeb0b0ae38e



## Running it

It needs the Amane CLI from [this fork](https://github.com/beefsizzle/amane), and uses ImageMagick, wl-clipboard,
cliphist, `pactl`, NetworkManager, BlueZ, curl, jq, grim and slurp.

```sh
git clone https://github.com/beefsizzle/amane
cd amane && ./install.sh && cd ..

git clone https://github.com/beefsizzle/daru
amane target daru
amane compile
amane run
```

Panels open with `amane ipc call toggle <name>` (see `src/panels.rs`). Other apps can follow the wallpaper's colours
too (see `src/recolor.rs`), and `greetd/` holds a login screen for greetd, with its own `install.sh`.

`umbriel/config.toml` is an example Umbriel config for daru: Umbriel's own starting config with daru starting at
login, its panels on the keybinds, and blur behind its bar and panels. Copy it to `~/.config/umbriel/config.toml`.

## License

[MIT](LICENSE)
