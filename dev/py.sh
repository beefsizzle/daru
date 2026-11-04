#!/bin/sh
# runs one of the Python test helpers here with the modules they need: dev/py.sh dev/fake_tray.py
# NixOS has no Python of its own, so this one comes from nixpkgs for the run (cached after the first time)
exec nix shell --impure --expr 'with import <nixpkgs> { }; python3.withPackages (p: [ p.dbus-python p.pygobject3 ])' \
    -c python3 "$@"
