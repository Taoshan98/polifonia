"""Horizontal Channel Row Card for Polifonia Audio Studio."""

import threading
import gi
gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
from gi.repository import Gtk, GLib
from core.models import SpeakerConfig, SpeakerRole


class SpeakerCard(Gtk.Box):
    """Horizontal channel row card representing an audio output endpoint."""

    def __init__(self, channel: SpeakerConfig, on_change_cb, on_test_cb, audio_service=None):
        super().__init__(orientation=Gtk.Orientation.VERTICAL, spacing=8)
        self.channel = channel
        self.on_change = on_change_cb
        self.on_test = on_test_cb
        self.audio_service = audio_service
        self._updating_ui = False
        self._vol_debounce_id = 0
        self._delay_debounce_id = 0
        self._enable_debounce_id = 0
        self._calib_apply_handler_id = 0

        self.add_css_class("channel-card")
        self.set_hexpand(True)

        self._build_ui()
        self._sync_state()

    def _build_ui(self):
        # 1. Header Row: Hardware Icon + Custom Title + Rename + Connection Badge + Hardware Subtitle
        title_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        title_row.set_valign(Gtk.Align.CENTER)

        self.icon_widget = Gtk.Image.new_from_icon_name(self._resolve_icon())
        self.icon_widget.set_pixel_size(20)
        self.icon_widget.add_css_class("strip-icon")
        title_row.append(self.icon_widget)

        self.title_label = Gtk.Label(label=self._get_display_title())
        self.title_label.set_tooltip_text(self._get_title_tooltip())
        self.title_label.add_css_class("strip-title")
        if self.channel.custom_name:
            self.title_label.add_css_class("strip-title-custom")
        self.title_label.set_halign(Gtk.Align.START)

        self.edit_btn = Gtk.Button.new_from_icon_name("document-edit-symbolic")
        self.edit_btn.set_tooltip_text("Customize output name")
        self.edit_btn.add_css_class("flat")
        self.edit_btn.add_css_class("strip-edit-btn")
        self.edit_btn.set_valign(Gtk.Align.CENTER)
        self._setup_rename_popover()
        self.edit_btn.connect("clicked", lambda b: self.popover.popup())

        title_click = Gtk.GestureClick()
        title_click.connect("released", lambda g, n, x, y: self.popover.popup())
        self.title_label.add_controller(title_click)

        title_row.append(self.title_label)
        title_row.append(self.edit_btn)

        self.badge_label = Gtk.Label(label=self._resolve_connection_badge())
        self.badge_label.add_css_class("conn-badge")
        title_row.append(self.badge_label)

        hw_desc = self._short_name(self.channel.display_name or self.channel.sink_name)
        self.desc_label = Gtk.Label(label=f"({hw_desc})")
        self.desc_label.add_css_class("dim-label")
        self.desc_label.set_halign(Gtk.Align.START)
        title_row.append(self.desc_label)
        self.append(title_row)

        # 2. Controls Row: Volume Slider -> Delay Controls -> Test Button -> Enable Switch
        controls_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=14)
        controls_row.set_valign(Gtk.Align.CENTER)
        controls_row.set_hexpand(True)

        vol_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        vol_box.set_valign(Gtk.Align.CENTER)
        vol_box.set_hexpand(True)

        vol_icon = Gtk.Image.new_from_icon_name("audio-volume-medium-symbolic")
        vol_icon.add_css_class("dim-label")
        vol_box.append(vol_icon)

        self.vol_scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, 0.0, 1.5, 0.05)
        self.vol_scale.set_hexpand(True)
        self.vol_scale.set_value(self.channel.volume_gain)
        self.vol_scale.connect("value-changed", self._on_vol_changed)
        vol_box.append(self.vol_scale)

        self.vol_badge = Gtk.Label(label=f"{int(self.channel.volume_gain * 100)}%")
        self.vol_badge.add_css_class("vol-badge")
        self.vol_badge.set_size_request(46, -1)
        vol_box.append(self.vol_badge)
        controls_row.append(vol_box)

        # Delay controls section (-5000ms to +5000ms, manual entry & acoustic calibration)
        delay_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=4)
        delay_box.set_valign(Gtk.Align.CENTER)
        delay_box.set_size_request(300, -1)
        self.delay_box = delay_box

        delay_lbl = Gtk.Label(label="Delay:")
        delay_lbl.add_css_class("dim-label")
        delay_box.append(delay_lbl)

        for lbl, tip, step in (("-10", "Decrease delay (-10 ms)", -10.0), ("-2", "Fine adjust (-2 ms)", -2.0)):
            btn = Gtk.Button(label=lbl)
            btn.set_tooltip_text(tip)
            btn.add_css_class("delay-btn")
            btn.connect("clicked", lambda b, s=step: self._step_delay(s))
            delay_box.append(btn)

        self.delay_badge = Gtk.Label()
        self.delay_badge.add_css_class("delay-badge")
        self.delay_badge.set_size_request(76, -1)
        self.delay_badge.set_tooltip_text("Click to enter numeric delay or calibrate")
        self.delay_badge.set_cursor_from_name("pointer")
        delay_box.append(self.delay_badge)

        for lbl, tip, step in (("+2", "Fine adjust (+2 ms)", 2.0), ("+10", "Increase delay (+10 ms)", 10.0)):
            btn = Gtk.Button(label=lbl)
            btn.set_tooltip_text(tip)
            btn.add_css_class("delay-btn")
            btn.connect("clicked", lambda b, s=step: self._step_delay(s))
            delay_box.append(btn)

        self.btn_delay_zero = Gtk.Button(label="0")
        self.btn_delay_zero.set_tooltip_text("Reset delay (0 ms)")
        self.btn_delay_zero.add_css_class("delay-reset-btn")
        self.btn_delay_zero.connect("clicked", lambda b: self._reset_delay())
        delay_box.append(self.btn_delay_zero)

        self.btn_delay_opts = Gtk.Button()
        self.btn_delay_opts.set_icon_name("document-edit-symbolic")
        self.btn_delay_opts.set_tooltip_text("Manual delay input, polarity & microphone calibration")
        self.btn_delay_opts.add_css_class("flat")
        self.btn_delay_opts.add_css_class("delay-opts-btn")
        delay_box.append(self.btn_delay_opts)

        self.delay_scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, -5000.0, 5000.0, 1.0)
        self.delay_scale.set_value(self.channel.delay_ms)
        self.delay_scale.connect("value-changed", self._on_delay_changed)

        self._setup_delay_popover()

        badge_click = Gtk.GestureClick()
        badge_click.connect("released", lambda g, n, x, y: self.delay_popover.popup())
        self.delay_badge.add_controller(badge_click)
        controls_row.append(delay_box)

        self.test_btn = Gtk.Button(label=" Test")
        self.test_btn.set_icon_name("audio-volume-high-symbolic")
        self.test_btn.add_css_class("flat")
        self.test_btn.add_css_class("strip-test-btn")
        self.test_btn.set_valign(Gtk.Align.CENTER)
        self.test_btn.set_size_request(90, -1)
        self.test_btn.connect("clicked", self._on_test_clicked)
        controls_row.append(self.test_btn)

        self.enable_switch = Gtk.Switch()
        self.enable_switch.set_valign(Gtk.Align.CENTER)
        self.enable_switch.connect("notify::active", self._on_enable_toggled)
        controls_row.append(self.enable_switch)

        self.append(controls_row)

    def _resolve_icon(self) -> str:
        name = (self.channel.display_name or self.channel.sink_name).lower()
        bus = getattr(self.channel, "bus_type", "").lower()
        if bus == "bluetooth" or "bluetooth" in name or "bluez" in name:
            return "audio-headphones-bluetooth"
        if bus == "network" or "airplay" in name or "raop" in name:
            return "network-wireless-symbolic"
        if bus == "hdmi" or any(k in name for k in ("monitor", "hdmi", "displayport")):
            return "video-display-symbolic"
        if bus == "usb" or "usb" in name or "dac" in name:
            return "audio-card-analog-usb-symbolic"
        if "headphone" in name:
            return "audio-headphones-symbolic"
        return "audio-speakers-symbolic"

    def _resolve_connection_badge(self) -> str:
        name = (self.channel.display_name or self.channel.sink_name).lower()
        bus = getattr(self.channel, "bus_type", "").lower()
        if bus == "bluetooth" or "bluetooth" in name or "bluez" in name:
            return "BT"
        if bus == "network" or "airplay" in name or "raop" in name:
            return "NET"
        if bus == "spdif" or any(k in name for k in ("spdif", "toslink", "optical")):
            return "SPDIF"
        if "displayport" in name or "dp" in name:
            return "DP"
        if bus == "hdmi" or "hdmi" in name:
            return "HDMI"
        if bus == "usb" or "usb" in name:
            return "USB"
        if bus == "pci" or "pci" in name or "speaker" in name:
            return "PCI"
        return "OUT"

    def _short_name(self, full_name: str) -> str:
        cleaned = full_name.replace("Monitor ", "").replace("Integrated Speakers ", "")
        for prefix in ("Bluetooth Audio (", "Bluetooth ("):
            if prefix in cleaned and cleaned.endswith(")"):
                return cleaned[len(prefix):-1].strip()
        if cleaned.endswith(" Audio"):
            cleaned = cleaned[:-6]
        cleaned = cleaned.replace("Audio ", "")
        if "/" in cleaned and ("HDMI" in cleaned or "DisplayPort" in cleaned):
            cleaned = cleaned.split("/")[-1].strip()
        if "(" in cleaned and len(cleaned) > 30:
            cleaned = cleaned.split("(")[0].strip()
        return cleaned.strip()

    def _sync_state(self):
        self._updating_ui = True
        is_active = self.channel.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)
        self.enable_switch.set_active(is_active)
        if is_active:
            self.remove_css_class("channel-card-disabled")
        else:
            self.add_css_class("channel-card-disabled")
        self._update_delay_badge()
        self._updating_ui = False

    def _on_enable_toggled(self, switch, param):
        if self._updating_ui:
            return
        active = switch.get_active()
        self.channel._acoustic_role = SpeakerRole.STEREO
        if active:
            self.channel.role = SpeakerRole.STEREO
            self.channel.mute = False
            if self.channel.volume_gain < 0.05:
                self.channel.volume_gain = 1.0
                self._updating_ui = True
                self.vol_scale.set_value(1.0)
                self.vol_badge.set_text("100%")
                self._updating_ui = False
            if self.audio_service:
                self.audio_service.set_speaker_gain(self.channel.sink_name, self.channel.volume_gain, persist=False)
                if hasattr(self.audio_service, "set_speaker_mute"):
                    self.audio_service.set_speaker_mute(self.channel.sink_name, False)
        else:
            self.channel.role = SpeakerRole.EXCLUDED
            self.channel.mute = True
            if self.audio_service and hasattr(self.audio_service, "set_speaker_mute"):
                self.audio_service.set_speaker_mute(self.channel.sink_name, True)
        self._sync_state()
        if self._enable_debounce_id:
            GLib.source_remove(self._enable_debounce_id)
            self._enable_debounce_id = 0
        self.on_change()

    def _on_role_clicked(self, new_role: SpeakerRole):
        self.channel.role = new_role
        is_active = new_role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)
        if is_active:
            self.channel._acoustic_role = new_role
        self.channel.mute = not is_active
        self.enable_switch.set_active(is_active)
        if self.audio_service and hasattr(self.audio_service, "set_speaker_mute"):
            self.audio_service.set_speaker_mute(self.channel.sink_name, not is_active)
        self._sync_state()
        if self._enable_debounce_id:
            GLib.source_remove(self._enable_debounce_id)
            self._enable_debounce_id = 0
        self.on_change()

    def _emit_enable_change(self):
        self._enable_debounce_id = 0
        self.on_change()
        return False

    def _on_vol_changed(self, scale):
        self.channel.volume_gain = round(scale.get_value(), 2)
        self.vol_badge.set_text(f"{int(self.channel.volume_gain * 100)}%")
        if self.audio_service:
            self.audio_service.set_speaker_gain(self.channel.sink_name, self.channel.volume_gain, persist=False)
        if self._vol_debounce_id:
            GLib.source_remove(self._vol_debounce_id)
        self._vol_debounce_id = GLib.timeout_add(300, self._emit_vol_change)

    def _emit_vol_change(self):
        self._vol_debounce_id = 0
        self.on_change()
        return False

    def _update_delay_badge(self):
        d = self.channel.delay_ms
        self.delay_badge.set_text(f"{'+' if d > 0 else ''}{d:.1f}ms" if d != 0.0 else "0.0ms")

    def _apply_delay_val(self, val: float, debounce_ms: int = 400):
        new_val = max(-5000.0, min(5000.0, round(val, 1)))
        self.channel.delay_ms = new_val
        self._updating_ui = True
        self.delay_scale.set_value(new_val)
        self._updating_ui = False
        self._update_delay_badge()
        if self._delay_debounce_id:
            GLib.source_remove(self._delay_debounce_id)
        self._delay_debounce_id = GLib.timeout_add(debounce_ms, self._emit_delay_change)

    def _step_delay(self, delta: float):
        self._apply_delay_val(self.channel.delay_ms + delta, 400)

    def _set_manual_delay(self, val: float):
        self._apply_delay_val(val, 150)

    def _reset_delay(self):
        self._apply_delay_val(0.0, 400)

    def _on_delay_changed(self, scale):
        if not self._updating_ui:
            self._apply_delay_val(scale.get_value(), 400)

    def _emit_delay_change(self):
        self._delay_debounce_id = 0
        self.on_change()
        return False

    def _setup_delay_popover(self):
        self.delay_popover = Gtk.Popover()
        self.delay_popover.set_parent(self.btn_delay_opts)
        self.delay_popover.set_autohide(True)
        self.btn_delay_opts.connect("clicked", lambda b: self.delay_popover.popup())

        content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        for m in ("top", "bottom", "start", "end"):
            getattr(content, f"set_margin_{m}")(12)
        content.set_size_request(280, -1)

        pop_title = Gtk.Label(label="Delay, Polarity & Calibration")
        pop_title.add_css_class("heading")
        pop_title.set_halign(Gtk.Align.START)
        content.append(pop_title)

        hw_name = self.channel.display_name or self.channel.sink_name
        hw_lat = getattr(self.channel, "hardware_latency_ms", 0.0)
        hw_info = f"Hardware: {self._short_name(hw_name)}" + (f" (HW buffer: {hw_lat:.0f} ms)" if hw_lat > 0.0 else "")
        hw_label = Gtk.Label(label=hw_info)
        hw_label.add_css_class("dim-label")
        hw_label.set_ellipsize(3)
        hw_label.set_halign(Gtk.Align.START)
        content.append(hw_label)

        # Manual delay entry
        input_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
        lbl_in = Gtk.Label(label="Delay (ms):")
        lbl_in.set_halign(Gtk.Align.START)
        input_box.append(lbl_in)

        self.delay_num_entry = Gtk.Entry()
        self.delay_num_entry.set_placeholder_text("e.g. -170.0 or +50.0")
        self.delay_num_entry.set_hexpand(True)
        self.delay_num_entry.set_text(f"{self.channel.delay_ms:.1f}")

        def _on_apply_entry(*_args):
            try:
                self._set_manual_delay(float(self.delay_num_entry.get_text().strip()))
                self.delay_popover.popdown()
            except ValueError:
                self.delay_num_entry.add_css_class("error")

        self.delay_num_entry.connect("activate", _on_apply_entry)
        apply_btn = Gtk.Button(label="Apply")
        apply_btn.add_css_class("suggested-action")
        apply_btn.connect("clicked", _on_apply_entry)
        input_box.append(self.delay_num_entry)
        input_box.append(apply_btn)
        content.append(input_box)

        # Phase / Polarity Inversion Row (180°)
        phase_row = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        phase_lbl = Gtk.Label(label="Phase Invert (180° Ø):")
        phase_lbl.set_tooltip_text("Invert acoustic polarity to fix bass/midrange cancellation between facing speakers")
        phase_lbl.set_halign(Gtk.Align.START)
        phase_lbl.set_hexpand(True)
        phase_row.append(phase_lbl)

        self.phase_switch = Gtk.Switch()
        self.phase_switch.set_valign(Gtk.Align.CENTER)
        self.phase_switch.set_active(bool(getattr(self.channel, "phase_inverted", False)))

        def _on_phase_toggled(sw, _param):
            if self._updating_ui:
                return
            inv = bool(sw.get_active())
            self.channel.phase_inverted = inv
            if self.audio_service and hasattr(self.audio_service, "set_speaker_phase"):
                self.audio_service.set_speaker_phase(self.channel.sink_name, inv)
            else:
                self.on_change()

        self.phase_switch.connect("notify::active", _on_phase_toggled)
        phase_row.append(self.phase_switch)
        content.append(phase_row)

        delay_tip = Gtk.Label(
            label="💡 Negative delay holds back all other speakers to align with a lagging device.\n"
                  "💡 Positive delay holds back only this individual speaker."
        )
        delay_tip.add_css_class("dim-label")
        delay_tip.set_wrap(True)
        delay_tip.set_max_width_chars(32)
        delay_tip.set_halign(Gtk.Align.START)
        content.append(delay_tip)

        content.append(Gtk.Separator(orientation=Gtk.Orientation.HORIZONTAL))

        # Single-channel Acoustic Calibration
        calib_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=6)
        calib_title = Gtk.Label(label="Automatic Acoustic Calibration")
        calib_title.add_css_class("heading")
        calib_title.set_halign(Gtk.Align.START)
        calib_box.append(calib_title)

        calib_desc = Gtk.Label(label="Emits test audio chirps and measures physical delay with your microphone.")
        calib_desc.add_css_class("dim-label")
        calib_desc.set_wrap(True)
        calib_desc.set_max_width_chars(32)
        calib_desc.set_halign(Gtk.Align.START)
        calib_box.append(calib_desc)

        self.btn_run_calib = Gtk.Button(label="🎙️ Start Acoustic Calibration")
        self.btn_run_calib.add_css_class("suggested-action")
        self.btn_run_calib.connect("clicked", self._on_start_acoustic_calibration)
        calib_box.append(self.btn_run_calib)

        self.calib_status_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        self.calib_status_box.set_visible(False)
        self.calib_spinner = Gtk.Spinner()
        self.calib_status_lbl = Gtk.Label(label="")
        self.calib_status_lbl.set_wrap(True)
        self.calib_status_lbl.set_max_width_chars(28)
        self.calib_status_box.append(self.calib_spinner)
        self.calib_status_box.append(self.calib_status_lbl)
        calib_box.append(self.calib_status_box)

        self.calib_result_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=6)
        self.calib_result_box.set_visible(False)
        self.calib_result_lbl = Gtk.Label(label="")
        self.calib_result_lbl.add_css_class("accent-label")
        self.btn_apply_calib = Gtk.Button(label="Apply")
        self.btn_apply_calib.add_css_class("suggested-action")
        self.calib_result_box.append(self.calib_result_lbl)
        self.calib_result_box.append(self.btn_apply_calib)
        calib_box.append(self.calib_result_box)

        content.append(calib_box)
        self.delay_popover.set_child(content)
        self.delay_popover.connect("show", lambda p: self.delay_num_entry.set_text(f"{self.channel.delay_ms:.1f}"))

    def _on_start_acoustic_calibration(self, btn):
        if not self.audio_service or not hasattr(self.audio_service, "measure_acoustic_delay"):
            return

        self.btn_run_calib.set_sensitive(False)
        self.calib_status_box.set_visible(True)
        self.calib_spinner.start()
        self.calib_result_box.set_visible(False)
        self.calib_status_lbl.set_text("Preparing calibration signals...")

        def _worker():
            delay_res, err = self.audio_service.measure_acoustic_delay(
                target_sink=self.channel.sink_name,
                progress_cb=lambda msg: GLib.idle_add(self.calib_status_lbl.set_text, msg)
            )
            def _finish():
                self.calib_spinner.stop()
                self.btn_run_calib.set_sensitive(True)
                if err or delay_res is None:
                    self.calib_status_box.set_visible(True)
                    self.calib_status_lbl.set_text(f"⚠️ {err or 'Error during calibration'}")
                    return
                self.calib_status_box.set_visible(False)
                self.calib_result_box.set_visible(True)
                if delay_res < 0:
                    self.calib_result_lbl.set_text(f"Lagging device: {abs(delay_res):.1f}ms\nCompensation: {delay_res:.1f}ms")
                elif delay_res > 0:
                    self.calib_result_lbl.set_text(f"Leading device: {delay_res:.1f}ms\nCompensation: +{delay_res:.1f}ms")
                else:
                    self.calib_result_lbl.set_text("Speakers perfectly synchronized (0.0ms)")

                if self._calib_apply_handler_id:
                    try:
                        self.btn_apply_calib.disconnect(self._calib_apply_handler_id)
                    except Exception:
                        pass
                def _apply_calib(_b):
                    self.delay_num_entry.set_text(f"{delay_res:.1f}")
                    self._set_manual_delay(delay_res)
                    self.delay_popover.popdown()
                self._calib_apply_handler_id = self.btn_apply_calib.connect("clicked", _apply_calib)

            GLib.idle_add(_finish)

        threading.Thread(target=_worker, daemon=True).start()

    def _setup_rename_popover(self):
        self.popover = Gtk.Popover()
        self.popover.set_parent(self.edit_btn)
        self.popover.set_autohide(True)
        self.edit_btn.get_popover = lambda: self.popover

        content = Gtk.Box(orientation=Gtk.Orientation.VERTICAL, spacing=10)
        for m in ("top", "bottom", "start", "end"):
            getattr(content, f"set_margin_{m}")(12)
        content.set_size_request(240, -1)

        pop_title = Gtk.Label(label="Customize Label")
        pop_title.add_css_class("heading")
        pop_title.set_halign(Gtk.Align.START)
        content.append(pop_title)

        hw_name = self.channel.display_name or self.channel.sink_name
        hw_label = Gtk.Label(label=f"Hardware: {self._short_name(hw_name)}")
        hw_label.add_css_class("dim-label")
        hw_label.set_ellipsize(3)
        hw_label.set_halign(Gtk.Align.START)
        content.append(hw_label)

        self.name_entry = Gtk.Entry()
        self.name_entry.set_placeholder_text("e.g. Left Monitor, Subwoofer...")
        if self.channel.custom_name:
            self.name_entry.set_text(self.channel.custom_name)
        self.name_entry.connect("activate", lambda e: self._on_save_custom_name(self.popover))
        content.append(self.name_entry)

        btn_box = Gtk.Box(orientation=Gtk.Orientation.HORIZONTAL, spacing=8)
        btn_box.set_halign(Gtk.Align.END)

        reset_btn = Gtk.Button(label="Reset")
        reset_btn.set_tooltip_text("Reset to default hardware name")
        reset_btn.add_css_class("flat")
        reset_btn.connect("clicked", lambda b: self._on_reset_custom_name(self.popover))
        btn_box.append(reset_btn)

        save_btn = Gtk.Button(label="Save")
        save_btn.add_css_class("suggested-action")
        save_btn.connect("clicked", lambda b: self._on_save_custom_name(self.popover))
        btn_box.append(save_btn)

        content.append(btn_box)
        self.popover.set_child(content)
        self.popover.connect("show", self._on_popover_show)

    def _on_popover_show(self, popover):
        self.name_entry.set_text(self.channel.custom_name or "")
        self.name_entry.grab_focus()

    def _get_display_title(self) -> str:
        return self.channel.custom_name or self._short_name(self.channel.display_name or self.channel.sink_name)

    def _get_title_tooltip(self) -> str:
        hw_name = self.channel.display_name or self.channel.sink_name
        return f"{self.channel.custom_name}\n(Device: {hw_name})" if self.channel.custom_name else hw_name

    def _on_save_custom_name(self, popover):
        text = self.name_entry.get_text().strip()
        hw_name = self.channel.display_name or self.channel.sink_name
        if text and text != hw_name:
            self.channel.custom_name = text
            self.title_label.add_css_class("strip-title-custom")
        else:
            self.channel.custom_name = None
            self.title_label.remove_css_class("strip-title-custom")
        self.title_label.set_text(self._get_display_title())
        self.title_label.set_tooltip_text(self._get_title_tooltip())
        popover.popdown()
        self.on_change()

    def _on_reset_custom_name(self, popover):
        self.channel.custom_name = None
        self.name_entry.set_text("")
        self.title_label.remove_css_class("strip-title-custom")
        self.title_label.set_text(self._get_display_title())
        self.title_label.set_tooltip_text(self._get_title_tooltip())
        popover.popdown()
        self.on_change()

    def _on_test_clicked(self, btn):
        target = self.channel.sink_name or str(self.channel.sink_id)
        display_label = self.channel.custom_name or self.channel.display_name or self.channel.sink_name
        self.on_test(target, display_label)
