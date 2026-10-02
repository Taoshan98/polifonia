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
import signal
from ui.views.main_window import MainWindow


class PolifoniaApplication(Adw.Application):
    def __init__(self):
        super().__init__(
            application_id="io.github.taoshan98.Polifonia",
            flags=Gio.ApplicationFlags.FLAGS_NONE
        )
        self.settings_store = SettingsStore()
        self.audio_engine = AudioEngineService()

    def do_activate(self):
        win = self.props.active_window
        if not win:
            win = MainWindow(self, self.audio_engine, self.settings_store)
        win.set_visible(True)
        win.present()

    def do_shutdown(self):
        if hasattr(self, "audio_engine"):
            self.audio_engine.stop_unison_sink()
        Adw.Application.do_shutdown(self)


def main():
    app = PolifoniaApplication()

    def _sig_handler(sig, frame):
        try:
            if hasattr(app, "audio_engine"):
                app.audio_engine.stop_unison_sink()
        except Exception:
            pass
        app.quit()
        sys.exit(0)

    signal.signal(signal.SIGINT, _sig_handler)
    signal.signal(signal.SIGTERM, _sig_handler)
    return app.run(sys.argv)


if __name__ == "__main__":
    sys.exit(main())
