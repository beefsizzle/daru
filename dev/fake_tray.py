#!/usr/bin/env python3
"""Two fake StatusNotifier items, to test the tray: dev/py.sh dev/fake_tray.py"""
import os, sys, math, dbus, dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
IFACE = 'org.kde.StatusNotifierItem'

def circle(size, rgb):
    data = []
    c = (size - 1) / 2
    for y in range(size):
        for x in range(size):
            d = math.hypot(x - c, y - c)
            a = 255 if d < size / 2 - 1 else 0
            data += [a, *rgb] if a else [0, 0, 0, 0]
    return dbus.Array([dbus.Byte(v) for v in data], signature='y')

class Item(dbus.service.Object):
    def __init__(self, n, props):
        self.name = dbus.service.BusName(f'org.kde.StatusNotifierItem-{os.getpid()}-{n}', bus)
        super().__init__(bus, '/StatusNotifierItem')
        self.props = props

    @dbus.service.method('org.freedesktop.DBus.Properties', in_signature='ss', out_signature='v')
    def Get(self, i, n):
        return self.props.get(n, '')

    @dbus.service.method('org.freedesktop.DBus.Properties', in_signature='s', out_signature='a{sv}')
    def GetAll(self, i):
        return self.props

    @dbus.service.method(IFACE, in_signature='ii')
    def Activate(self, x, y):
        print('activate', self.props['Id'], flush=True)

    @dbus.service.method(IFACE, in_signature='ii')
    def ContextMenu(self, x, y):
        print('context', self.props['Id'], flush=True)

def pixmap(size, rgb):
    return dbus.Array([dbus.Struct((dbus.Int32(size), dbus.Int32(size), circle(size, rgb)), signature='iiay')], signature='(iiay)')

items = [
    Item(1, {'Id': 'discord', 'Title': 'Discord', 'Status': 'Active', 'IconName': 'discord'}),
]
# second object needs its own bus name AND path; reuse path on a different connection
bus2 = dbus.SessionBus(private=True)
class Item2(Item):
    def __init__(self):
        self.name = dbus.service.BusName(f'org.kde.StatusNotifierItem-{os.getpid()}-2', bus2)
        dbus.service.Object.__init__(self, bus2, '/StatusNotifierItem')
        self.props = {'Id': 'pixels', 'Title': 'Pixel app', 'Status': 'Active', 'IconName': '', 'IconPixmap': pixmap(32, (122, 162, 247))}
items.append(Item2())

watcher = bus.get_object('org.kde.StatusNotifierWatcher', '/StatusNotifierWatcher', introspect=False)
for n, it in enumerate(items, 1):
    watcher.RegisterStatusNotifierItem(f'org.kde.StatusNotifierItem-{os.getpid()}-{n}', dbus_interface='org.kde.StatusNotifierWatcher')
print('registered', flush=True)
GLib.MainLoop().run()
