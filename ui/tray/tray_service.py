"""Tray Service manager running in GTK4 process to coordinate with tray indicator."""

import os
import sys
import json
import subprocess
import threading
from typing import Callable, Optional, List, Dict, Any
import gi
from gi.repository import GLib


class TrayService:
    """Manages the background system tray indicator companion process."""

    def __init__(
        self,
        on_toggle_window: Callable[[], None],
        on_toggle_unison: Callable[[], None],
        on_set_volume: Callable[[float], None],
        on_toggle_channel_enable: Callable[[str, bool], None],
        on_set_channel_role: Callable[[str, str], None],
        on_set_channel_volume: Callable[[str, float], None],
        on_test_channel: Callable[[str, str], None],
        on_quit: Callable[[], None]
    ):
        self.on_toggle_window = on_toggle_window
        self.on_toggle_unison = on_toggle_unison
        self.on_set_volume = on_set_volume
        self.on_toggle_channel_enable = on_toggle_channel_enable
        self.on_set_channel_role = on_set_channel_role
        self.on_set_channel_volume = on_set_channel_volume
        self.on_test_channel = on_test_channel
        self.on_quit = on_quit

        self.proc: Optional[subprocess.Popen] = None
        self._running = False
        self._reader_thread: Optional[threading.Thread] = None

    def start(self):
        """Starts the tray indicator companion process."""
        if self._running:
            return

        indicator_script = os.path.join(os.path.dirname(__file__), "tray_indicator.py")
        if not os.path.exists(indicator_script):
            return

        try:
            self.proc = subprocess.Popen(
                [sys.executable, indicator_script],
                stdin=subprocess.PIPE,
                stdout=subprocess.PIPE,
                stderr=subprocess.DEVNULL,
                text=True,
                bufsize=1
            )
            self._running = True
            self._reader_thread = threading.Thread(target=self._read_stdout, daemon=True)
            self._reader_thread.start()
        except Exception as e:
            sys.stderr.write(f"Failed to start tray service: {e}\n")

    def sync_state(self, is_active: bool, master_volume: float, window_visible: bool, channels: List[Dict[str, Any]] = None):
        """Sends updated application state and channels list to the tray indicator."""
        if not self._running or not self.proc or not self.proc.stdin:
            return

        msg = {
            "type": "sync_state",
            "is_active": is_active,
            "master_volume": master_volume,
            "window_visible": window_visible,
            "channels": channels or []
        }
        try:
            self.proc.stdin.write(json.dumps(msg) + "\n")
            self.proc.stdin.flush()
        except Exception:
            pass

    def stop(self):
        """Terminates the tray indicator companion process."""
        self._running = False
        if self.proc:
            try:
                if self.proc.stdin:
                    self.proc.stdin.write(json.dumps({"type": "quit"}) + "\n")
                    self.proc.stdin.flush()
            except Exception:
                pass
            try:
                self.proc.terminate()
            except Exception:
                pass
            self.proc = None

    def _read_stdout(self):
        """Listens for user commands from the tray indicator."""
        if not self.proc or not self.proc.stdout:
            return

        try:
            for line in self.proc.stdout:
                line = line.strip()
                if not line:
                    continue
                try:
                    data = json.loads(line)
                    cmd = data.get("command")
                    if cmd == "toggle_window":
                        GLib.idle_add(self.on_toggle_window)
                    elif cmd == "toggle_unison":
                        GLib.idle_add(self.on_toggle_unison)
                    elif cmd == "set_volume":
                        vol = data.get("volume", 1.0)
                        GLib.idle_add(self.on_set_volume, vol)
                    elif cmd == "toggle_channel_enable":
                        s_name = data.get("sink_name", "")
                        en = data.get("enabled", True)
                        GLib.idle_add(self.on_toggle_channel_enable, s_name, en)
                    elif cmd == "set_channel_role":
                        s_name = data.get("sink_name", "")
                        role_str = data.get("role", "STEREO")
                        GLib.idle_add(self.on_set_channel_role, s_name, role_str)
                    elif cmd == "set_channel_volume":
                        s_name = data.get("sink_name", "")
                        vol = data.get("volume", 1.0)
                        GLib.idle_add(self.on_set_channel_volume, s_name, vol)
                    elif cmd == "test_channel":
                        s_name = data.get("sink_name", "")
                        d_name = data.get("display_name", "")
                        GLib.idle_add(self.on_test_channel, s_name, d_name)
                    elif cmd == "quit":
                        GLib.idle_add(self.on_quit)
                except Exception as parse_err:
                    sys.stderr.write(f"Tray command parsing error: {parse_err}\n")
        except Exception:
            pass
