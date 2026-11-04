#!/usr/bin/env python3
"""Fake tray apps for testing the tray's robustness: one that never answers, one that sends a huge pixmap size.
   dev/py.sh dev/fake_hang.py"""
import os, time, dbus, dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

DBusGMainLoop(set_as_default=True)

def start(n, props, hang):
    bus = dbus.SessionBus(private=True)
    name = f'org.kde.StatusNotifierItem-{os.getpid()}-{n}'
    dbus.service.BusName(name, bus)

    class Item(dbus.service.Object):
        @dbus.service.method('org.freedesktop.DBus.Properties', in_signature='ss', out_signature='v')
        def Get(self, i, p):
            if hang:
                time.sleep(3600)   # never answers in time: blocks this connection's main loop
            return props.get(p, '')

        @dbus.service.method('org.freedesktop.DBus.Properties', in_signature='s', out_signature='a{sv}')
        def GetAll(self, i):
            return props

    Item(bus, '/StatusNotifierItem')
    watcher = bus.get_object('org.kde.StatusNotifierWatcher', '/StatusNotifierWatcher', introspect=False)
    watcher.RegisterStatusNotifierItem(name, dbus_interface='org.kde.StatusNotifierWatcher')
    return bus

huge = dbus.Array([dbus.Struct((dbus.Int32(65536), dbus.Int32(65536), dbus.Array([], signature='y')), signature='iiay')], signature='(iiay)')

if os.environ.get('MODE') == 'hang':
    start(1, {'Id': 'hung', 'Title': 'Hung app', 'Status': 'Active', 'IconName': 'discord'}, hang=True)
else:
    start(2, {'Id': 'huge', 'Title': 'Huge pixmap', 'Status': 'Active', 'IconName': '', 'IconPixmap': huge}, hang=False)
print('up', flush=True)
GLib.MainLoop().run()
