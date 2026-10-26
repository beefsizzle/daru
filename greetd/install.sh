#!/bin/sh
# sets up daru's login screen for greetd, once per machine:   sudo greetd/install.sh [--scale 1.5]
# and puts greetd's own setup back:                          sudo greetd/install.sh --remove
#
# greetd starts cage, which shows ReGreet over the whole screen and is gone again once you're logged in, so nothing of
# the login screen stays running in your session (SDDM keeps its X server for as long as you're logged in). --scale
# is for a high-dpi screen, which cage would otherwise draw at scale 1; it needs wlr-randr.
#
# the greeter runs as its own user and can't read your home folder, so the shell copies the wallpaper and its colours
# to /var/lib/daru-greeter, a folder this makes for you (add `regreet` to ~/.config/amane-shell/recolor). ReGreet's
# own config stays in /etc and belongs to root; the shell only writes the picture and the stylesheet.
#
# this doesn't switch from your current login screen. it says how at the end, so it happens when you choose
set -eu

etc=/etc/greetd
data=/var/lib/daru-greeter
pam=/etc/pam.d/greetd

if [ "$(id -u)" != 0 ]; then
    echo "run this with sudo"
    exit 1
fi

user="${SUDO_USER:-}"

if [ -z "$user" ] || [ "$user" = root ]; then
    echo "run this with sudo from your own account: the folder the shell writes to belongs to you"
    exit 1
fi

here="$(cd "$(dirname "$0")" && pwd)"

# greetd's own files as they were before the first install, for --remove
backup() {
    if [ -f "$1" ] && [ ! -f "$1.before-daru" ]; then
        cp -p "$1" "$1.before-daru"
    fi
}

restore() {
    if [ -f "$1.before-daru" ]; then
        mv "$1.before-daru" "$1"
    fi
}

if [ "${1:-}" = "--remove" ]; then
    restore "$etc/config.toml"
    restore "$etc/regreet.toml"
    restore "$pam"
    rm -f "$etc/daru-greeter" "$etc/daru-scale"
    rm -rf "$data"

    echo "removed. greetd is back to its own setup. to log in through SDDM again:"
    echo "    sudo systemctl disable greetd && sudo systemctl enable sddm"
    exit 0
fi

scale=1

if [ "${1:-}" = "--scale" ]; then
    scale="${2:-}"

    # a plain number, since the greeter's start script reads it
    if ! printf '%s' "$scale" | grep -Eq '^[0-9]+(\.[0-9]+)?$'; then
        echo "--scale takes a number, like 1.5"
        exit 1
    fi
fi

for program in greetd cage regreet; do
    if ! command -v "$program" >/dev/null; then
        echo "$program isn't installed: sudo pacman -S greetd greetd-regreet cage"
        exit 1
    fi
done

if [ "$scale" != 1 ] && ! command -v wlr-randr >/dev/null; then
    echo "--scale needs wlr-randr: sudo pacman -S wlr-randr"
    exit 1
fi

for file in config.toml regreet.toml daru-greeter; do
    if [ ! -f "$here/$file" ]; then
        echo "$here/$file is missing"
        exit 1
    fi
done

backup "$etc/config.toml"
backup "$etc/regreet.toml"
backup "$pam"

install -m 644 "$here/config.toml" "$here/regreet.toml" "$etc/"
install -m 755 "$here/daru-greeter" "$etc/"
printf '%s\n' "$scale" >"$etc/daru-scale"

install -d -o "$user" -m 755 "$data"

# SDDM's login unlocks the keyring with your password, and so does this one. the leading - skips a module that isn't
# installed
if ! grep -q pam_gnome_keyring "$pam"; then
    printf '%s\n' '-auth       optional     pam_gnome_keyring.so' '-session    optional     pam_gnome_keyring.so auto_start' >>"$pam"
fi

# a shell that's running writes the wallpaper and colours now, rather than at the next start or wallpaper change.
# it listens on a socket in the user's runtime folder
uid="$(id -u "$user")"
home="$(getent passwd "$user" | cut -d: -f6)"

if [ -x "$home/.cargo/bin/amane" ] && grep -qx regreet "$home/.config/amane-shell/recolor" 2>/dev/null; then
    runuser -u "$user" -- env XDG_RUNTIME_DIR="/run/user/$uid" "$home/.cargo/bin/amane" ipc call recolor >/dev/null 2>&1 || true
fi

echo "installed. add 'regreet' to ~/.config/amane-shell/recolor and pick a wallpaper (or restart the shell), and the"
echo "login screen follows it from then on. to look at it without logging out:"
echo "    regreet --demo --style $data/regreet.css"
echo "to log in through it from the next boot (SDDM stays installed, so this works both ways):"
echo "    sudo systemctl disable sddm && sudo systemctl enable greetd"
