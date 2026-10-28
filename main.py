#!/usr/bin/env python3
"""Polifonia Audio Studio - Main Application Entrypoint."""

import os
import signal
import sys

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
os.environ.setdefault("GSK_RENDERER", "gl")

import gi
gi.require_version("Gtk", "4.0")
gi.require_version("Adw", "1")
from gi.repository import Adw, Gio, GLib

from services.audio_engine import AudioEngineService
from storage.settings_store import SettingsStore
from ui.views.main_window import MainWindow


class PolifoniaApplication(Adw.Application):
    def __init__(self):
        super().__init__(
            application_id="io.github.taoshan98.Polifonia",
            flags=Gio.ApplicationFlags.FLAGS_NONE,
        )
        self.settings_store = SettingsStore()
        self.audio_engine = AudioEngineService(storage=self.settings_store)

    def do_activate(self):
        win = self.props.active_window
        if not win:
            win = MainWindow(self, self.audio_engine, self.settings_store)
        win.set_visible(True)
        win.present()

    def do_shutdown(self):
        if hasattr(self, "audio_engine"):
            self.audio_engine.stop_unison_sink(unload_master=True)
        Adw.Application.do_shutdown(self)


def main():
    app = PolifoniaApplication()
    for sig in (signal.SIGINT, signal.SIGTERM):
        GLib.unix_signal_add(GLib.PRIORITY_DEFAULT, sig, lambda *_: (app.quit(), GLib.SOURCE_REMOVE)[1])
    return app.run(sys.argv)


if __name__ == "__main__":
    sys.exit(main())
