#!/usr/bin/env python3
"""Polifonia Audio Studio - Main Application Entrypoint."""

import sys
import os

# Add parent directory to path so polifonia package can be resolved
sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import gi
gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
from gi.repository import Gtk, Adw, Gio

from polifonia.backend.pipewire_client import PipewireClient
from polifonia.services.audio_service import AudioService
from polifonia.storage.preset_manager import PresetManager
from polifonia.ui.views.main_window import MainWindow


class PolifoniaApplication(Adw.Application):
    def __init__(self):
        super().__init__(
            application_id="io.polifonia.AudioStudio",
            flags=Gio.ApplicationFlags.FLAGS_NONE
        )
        self.pw_client = PipewireClient()
        self.preset_manager = PresetManager()
        self.audio_service = AudioService(self.pw_client)

    def do_activate(self):
        win = self.props.active_window
        if not win:
            win = MainWindow(self, self.audio_service, self.preset_manager)
        win.present()


def main():
    app = PolifoniaApplication()
    return app.run(sys.argv)


if __name__ == "__main__":
    sys.exit(main())
