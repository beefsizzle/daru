#!/usr/bin/env python3
"""A fake tray item with a dbusmenu that includes a hidden entry, to test menu rendering: dev/py.sh dev/fake_menu.py
   MENU_DELAY=2 makes it answer slowly, like a busy app, to test menus that arrive late"""
import os, time, dbus, dbus.service
from dbus.mainloop.glib import DBusGMainLoop
from gi.repository import GLib

DBusGMainLoop(set_as_default=True)
bus = dbus.SessionBus()
name = f'org.kde.StatusNotifierItem-{os.getpid()}-9'
bus_name = dbus.service.BusName(name, bus)

class Item(dbus.service.Object):
    props = {'Id': 'menu-test', 'Title': 'Menu test', 'Status': 'Active', 'IconName': 'discord',
             'Menu': dbus.ObjectPath('/MenuBar')}

    @dbus.service.method('org.freedesktop.DBus.Properties', in_signature='ss', out_signature='v')
    def Get(self, i, n): return self.props.get(n, '')

    @dbus.service.method('org.freedesktop.DBus.Properties', in_signature='s', out_signature='a{sv}')
    def GetAll(self, i): return self.props

class Menu(dbus.service.Object):
    def entry(self, id, props):
        return dbus.Struct((dbus.Int32(id), dbus.Dictionary(props, signature='sv'), dbus.Array([], signature='v')), signature='ia{sv}av')

    @dbus.service.method('com.canonical.dbusmenu', in_signature='iias', out_signature='u(ia{sv}av)')
    def GetLayout(self, parent, depth, names):
        kids = [
            self.entry(1, {'label': 'Visible entry'}),
            self.entry(2, {'label': 'Hidden entry (must not show)', 'visible': dbus.Boolean(False)}),
            self.entry(3, {'type': 'separator'}),
            self.entry(4, {'label': 'Quit'}),
        ]
        root = dbus.Struct((dbus.Int32(0), dbus.Dictionary({'children-display': 'submenu'}, signature='sv'),
                            dbus.Array([dbus.Struct(k, variant_level=1) for k in kids], signature='v')), signature='ia{sv}av')
        return (dbus.UInt32(1), root)

    @dbus.service.method('com.canonical.dbusmenu', in_signature='i', out_signature='b')
    def AboutToShow(self, id):
        print('about to show', flush=True)
        time.sleep(float(os.environ.get('MENU_DELAY', '0')))
        return False

    @dbus.service.method('com.canonical.dbusmenu', in_signature='isvu')
    def Event(self, id, event, data, ts): print('event', id, event, flush=True)

Item(bus, '/StatusNotifierItem')
Menu(bus, '/MenuBar')
watcher = bus.get_object('org.kde.StatusNotifierWatcher', '/StatusNotifierWatcher', introspect=False)
watcher.RegisterStatusNotifierItem(name, dbus_interface='org.kde.StatusNotifierWatcher')
print('up', flush=True)
GLib.MainLoop().run()
