# daru

A desktop shell for [Umbriel](https://docs.noctalia.dev/umbriel/), built with [Amane](https://github.com/MystiaFin/amane).
Bar, tray, dashboard, launcher, notifications, lock screen, wallpaper picker and the usual dropdowns, all coloured
from the wallpaper.

It's a personal shell, not a plug-and-play one: expect to read the source.

![screenshot](media/screenshot.jpg)

https://github.com/user-attachments/assets/a843d4c3-b076-4d88-abff-8120f4c5d30a

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
too (see `src/recolor.rs`), and `greetd/` and `sddm/` hold two login screens, each with its own `install.sh`.

## License

[MIT](LICENSE)
