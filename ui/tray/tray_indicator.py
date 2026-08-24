import os
import sys
import json
import threading

import gi
try:
    gi.require_version('Gtk', '3.0')
    gi.require_version('AyatanaAppIndicator3', '0.1')
    from gi.repository import Gtk, AyatanaAppIndicator3 as AppIndicator, GLib
except Exception:
    try:
        gi.require_version('AppIndicator3', '0.1')
        from gi.repository import Gtk, AppIndicator3 as AppIndicator, GLib
    except Exception as e:
        sys.stderr.write(f"AppIndicator not available: {e}\n")
        sys.exit(1)


class TrayIndicatorApp:
    def __init__(self):
        self.is_active = False
        self.master_volume = 1.0
        self.channels = []
        self._syncing_ui = False

        # AppIndicator uses gtk_icon_theme_append_search_path() to find icons.
        # Two critical requirements:
        #   1. The directory must contain ONLY icon files (no subdirs like icons/hicolor
        #      that make GTK interpret it as a theme root)
        #   2. Avoid the '-symbolic' suffix — AppIndicator applies special lookup
        #      rules for symbolic icons that break with custom search paths
        icon_dir = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "assets", "tray"))

        self.indicator = AppIndicator.Indicator.new_with_path(
            "polifonia_audio_tray",
            "polifonia-tray",
            AppIndicator.IndicatorCategory.APPLICATION_STATUS,
            icon_dir
        )

        self.indicator.set_status(AppIndicator.IndicatorStatus.ACTIVE)
        self.indicator.set_title("Polifonia Audio Studio")

        self.menu = Gtk.Menu()
        self._rebuild_menu()
        self.indicator.set_menu(self.menu)

        # Start stdin listener thread for IPC from GTK4 main process
        self.running = True
        self.listener_thread = threading.Thread(target=self._listen_stdin, daemon=True)
        self.listener_thread.start()

    def _rebuild_menu(self):
        self._syncing_ui = True
        # Clear existing items
        for child in self.menu.get_children():
            self.menu.remove(child)

        # 1. App Title (Left click / primary action opens/hides console)
        self.title_item = Gtk.MenuItem(label="Polifonia Audio Studio")
        self.title_item.connect("activate", lambda w: self._send_cmd("toggle_window"))
        self.menu.append(self.title_item)

        self.menu.append(Gtk.SeparatorMenuItem())

        # 2. Master Unison Check Toggle
        unison_label = "Unison Engine Active" if self.is_active else "Unison Engine Standby"
        self.toggle_unison_item = Gtk.CheckMenuItem(label=unison_label)
        self.toggle_unison_item.set_active(self.is_active)
        self.toggle_unison_item.connect("toggled", self._on_unison_toggled)
        self.menu.append(self.toggle_unison_item)

        # 3. Master Volume Submenu
        vol_item = Gtk.MenuItem(label=f"Master Volume ({int(self.master_volume * 100)}%)")
        vol_submenu = Gtk.Menu()

        vol_group = None
        for vol_pct in [100, 80, 60, 40, 20, 0]:
            label = f"{vol_pct}%" if vol_pct > 0 else "Mute (0%)"
            v_val = vol_pct / 100.0
            is_cur_vol = abs(self.master_volume - v_val) < 0.05
            v_sub = Gtk.RadioMenuItem.new_with_label(vol_group, label)
            vol_group = v_sub.get_group()
            v_sub.set_active(is_cur_vol)
            v_sub.connect("toggled", lambda w, v=v_val: self._on_master_vol_toggled(w, v))
            vol_submenu.append(v_sub)

        vol_item.set_submenu(vol_submenu)
        self.menu.append(vol_item)

        # 4. Speakers & Destinations Submenu
        if self.channels:
            self.menu.append(Gtk.SeparatorMenuItem())
            active_count = sum(1 for c in self.channels if c.get("is_enabled", True))
            speakers_menu_item = Gtk.MenuItem(label=f"Destinations ({active_count}/{len(self.channels)} Active)")
            speakers_submenu = Gtk.Menu()

            for ch in self.channels:
                s_name = ch.get("sink_name", "")
                d_name = ch.get("display_name") or s_name
                is_enabled = ch.get("is_enabled", True)
                role = ch.get("role", "stereo").upper()
                gain = ch.get("volume_gain", 1.0)

                # Channel Root Item (Standard MenuItem so clicking it opens the details submenu)
                status_char = "✓" if is_enabled else "✗"
                spk_item = Gtk.MenuItem(label=f"[{status_char}] {d_name} ({role})")
                spk_sub = Gtk.Menu()

                # 1. Enable/Mute Check Toggle inside submenu
                en_item = Gtk.CheckMenuItem(label="Active in Unison")
                en_item.set_active(is_enabled)
                en_item.connect("toggled", lambda w, sn=s_name: self._on_channel_toggled(w, sn))
                spk_sub.append(en_item)

                spk_sub.append(Gtk.SeparatorMenuItem())

                # 2. Role Selection Submenu with Radio buttons
                role_menu_item = Gtk.MenuItem(label=f"Role: {role}")
                role_sub = Gtk.Menu()
                roles = [
                    ("LEFT", "Left Channel (FL)"),
                    ("RIGHT", "Right Channel (FR)"),
                    ("CENTER", "Center (Mono)"),
                    ("SUBWOOFER", "Subwoofer"),
                    ("STEREO", "Full Stereo"),
                    ("SURROUND_LEFT", "Surround")
                ]
                r_group = None
                for r_code, r_desc in roles:
                    is_cur_role = (role == r_code)
                    r_item = Gtk.RadioMenuItem.new_with_label(r_group, r_desc)
                    r_group = r_item.get_group()
                    r_item.set_active(is_cur_role)
                    r_item.connect("toggled", lambda w, sn=s_name, r=r_code: self._on_role_toggled(w, sn, r))
                    role_sub.append(r_item)
                role_menu_item.set_submenu(role_sub)
                spk_sub.append(role_menu_item)

                # 3. Channel Volume Submenu with Radio buttons
                gain_menu_item = Gtk.MenuItem(label=f"Volume: {int(gain * 100)}%")
                gain_sub = Gtk.Menu()
                g_group = None
                for v_pct in [100, 80, 60, 40, 20, 0]:
                    v_lbl = f"{v_pct}%" if v_pct > 0 else "Mute (0%)"
                    v_val = v_pct / 100.0
                    is_cur_gain = abs(gain - v_val) < 0.05
                    v_item = Gtk.RadioMenuItem.new_with_label(g_group, v_lbl)
                    g_group = v_item.get_group()
                    v_item.set_active(is_cur_gain)
                    v_item.connect("toggled", lambda w, sn=s_name, v=v_val: self._on_channel_vol_toggled(w, sn, v))
                    gain_sub.append(v_item)
                gain_menu_item.set_submenu(gain_sub)
                spk_sub.append(gain_menu_item)

                spk_sub.append(Gtk.SeparatorMenuItem())

                # 4. Test Output
                test_item = Gtk.MenuItem(label="Test Output")
                test_item.connect("activate", lambda w, sn=s_name, dn=d_name: self._send_cmd("test_channel", {"sink_name": sn, "display_name": dn}))
                spk_sub.append(test_item)

                spk_item.set_submenu(spk_sub)
                speakers_submenu.append(spk_item)

            speakers_menu_item.set_submenu(speakers_submenu)
            self.menu.append(speakers_menu_item)

        self.menu.append(Gtk.SeparatorMenuItem())

        # 5. Open / Hide Window
        self.show_window_item = Gtk.MenuItem(label="Open Studio Console")
        self.show_window_item.connect("activate", lambda w: self._send_cmd("toggle_window"))
        self.menu.append(self.show_window_item)

        # 6. Quit
        self.quit_item = Gtk.MenuItem(label="Quit Polifonia")
        self.quit_item.connect("activate", lambda w: self._send_cmd("quit"))
        self.menu.append(self.quit_item)

        self.menu.show_all()
        self._syncing_ui = False

    def _on_unison_toggled(self, widget):
        if self._syncing_ui:
            return
        self._send_cmd("toggle_unison")

    def _on_master_vol_toggled(self, widget, vol: float):
        if self._syncing_ui or not widget.get_active():
            return
        self._send_cmd("set_volume", {"volume": vol})

    def _on_channel_toggled(self, widget, sink_name: str):
        if self._syncing_ui:
            return
        is_active = widget.get_active()
        self._send_cmd("toggle_channel_enable", {"sink_name": sink_name, "enabled": is_active})

    def _on_role_toggled(self, widget, sink_name: str, role_code: str):
        if self._syncing_ui or not widget.get_active():
            return
        self._send_cmd("set_channel_role", {"sink_name": sink_name, "role": role_code})

    def _on_channel_vol_toggled(self, widget, sink_name: str, vol: float):
        if self._syncing_ui or not widget.get_active():
            return
        self._send_cmd("set_channel_volume", {"sink_name": sink_name, "volume": vol})

    def _send_cmd(self, cmd: str, extra=None):
        payload = {"command": cmd}
        if extra:
            payload.update(extra)
        try:
            sys.stdout.write(json.dumps(payload) + "\n")
            sys.stdout.flush()
        except Exception:
            pass

    def _listen_stdin(self):
        for line in sys.stdin:
            line = line.strip()
            if not line:
                continue
            try:
                msg = json.loads(line)
                msg_type = msg.get("type")
                if msg_type == "sync_state":
                    GLib.idle_add(self._apply_state_sync, msg)
                elif msg_type == "quit":
                    GLib.idle_add(Gtk.main_quit)
                    break
            except Exception as e:
                sys.stderr.write(f"Error parsing IPC message: {e}\n")

    def _apply_state_sync(self, data: dict):
        self.is_active = data.get("is_active", False)
        self.master_volume = data.get("master_volume", 1.0)
        self.channels = data.get("channels", [])
        window_visible = data.get("window_visible", True)

        self._rebuild_menu()

        # Update window toggle label
        if hasattr(self, "show_window_item"):
            if window_visible:
                self.show_window_item.set_label("Hide Studio Console")
            else:
                self.show_window_item.set_label("Open Studio Console")


if __name__ == "__main__":
    app = TrayIndicatorApp()
    Gtk.main()
