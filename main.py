#!/usr/bin/env python3
"""Polifonia Audio Studio - Main Application Entrypoint."""

import sys
import os

# Add parent directory to path so polifonia package can be resolved
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

# Set GSK_RENDERER to OpenGL to avoid Mesa Intel Vulkan driver warning and ensure smooth rendering
os.environ.setdefault("GSK_RENDERER", "gl")

import gi
gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
from gi.repository import Gtk, Adw, Gio

from services.audio_engine import AudioEngineService
from storage.settings_store import SettingsStore
from ui.views.main_window import MainWindow


class PolifoniaApplication(Adw.Application):
    def __init__(self):
        super().__init__(
            application_id="io.polifonia.AudioStudio",
            flags=Gio.ApplicationFlags.NON_UNIQUE
        )
        self.settings_store = SettingsStore()
        self.audio_engine = AudioEngineService()

    def do_activate(self):
        win = self.props.active_window
        if not win:
            win = MainWindow(self, self.audio_engine, self.settings_store)
        win.present()


def main():
    app = PolifoniaApplication()
    return app.run(sys.argv)


if __name__ == "__main__":
    sys.exit(main())
