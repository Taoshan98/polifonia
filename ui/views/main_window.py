import os
import gi
gi.require_version('Gtk', '4.0')
gi.require_version('Adw', '1')
from gi.repository import Gtk, Adw, GLib, Gio, Gdk
from services.audio_service import AudioService
from storage.preset_manager import PresetManager
from core.models import SystemConfig, SpeakerRole, SpeakerConfig
from ui.views.speaker_row import SpeakerRow


class MainWindow(Adw.ApplicationWindow):
    def __init__(self, app, audio_service: AudioService, preset_manager: PresetManager):
        super().__init__(application=app, title="Polifonia Audio Studio")
        self.set_default_size(750, 800)

        # Load Custom Cross-Desktop Studio Theme
        css_provider = Gtk.CssProvider()
        css_path = os.path.join(os.path.dirname(__file__), "..", "styles", "studio_theme.css")
        if os.path.exists(css_path):
            css_provider.load_from_path(css_path)
            display = Gdk.Display.get_default()
            if display:
                Gtk.StyleContext.add_provider_for_display(
                    display,
                    css_provider,
                    Gtk.STYLE_PROVIDER_PRIORITY_APPLICATION
                )

        self.audio_service = audio_service
        self.preset_manager = preset_manager
        self.config: SystemConfig = self.preset_manager.load_config()
        self._speaker_rows = []

        # Merge live sinks with saved configuration
        self._sync_live_sinks()

        self._build_ui()
        self._refresh_status_banner()

    def _sync_live_sinks(self):
        live_sinks = self.audio_service.get_available_sinks()
        saved_channels_map = {c.sink_name: c for c in self.config.channels}
        
        updated_channels = []
        for live in live_sinks:
            if live.name in saved_channels_map:
                saved = saved_channels_map[live.name]
                saved.sink_id = live.id
                saved.display_name = live.description
                updated_channels.append(saved)
            else:
                # Default exclusion of internal laptop speakers
                role = SpeakerRole.EXCLUDED if (live.is_internal or "pci" in live.name.lower() or "speaker" in live.description.lower()) else SpeakerRole.LEFT
                spk = SpeakerConfig(
                    sink_id=live.id,
                    sink_name=live.name,
                    display_name=live.description,
                    role=role
                )
                updated_channels.append(spk)

        self.config.channels = updated_channels

    def _build_ui(self):
        # Toast overlay for modern notifications
        self.toast_overlay = Adw.ToastOverlay()
        self.set_content(self.toast_overlay)

        # Main Box
        main_box = Gtk.Box(orientation=Gtk.Orientation.VERTICAL)
        self.toast_overlay.set_child(main_box)

        # Header Bar
        header_bar = Adw.HeaderBar()
        title_widget = Adw.WindowTitle(title="Polifonia Audio", subtitle="Gestione Sistema Audio Multi-Monitor / 2.1")
        header_bar.set_title_widget(title_widget)

        # Reload / Rescan button in header
        rescan_btn = Gtk.Button.new_from_icon_name("view-refresh-symbolic")
        rescan_btn.set_tooltip_text("Riscansiona uscite audio PipeWire")
        rescan_btn.connect("clicked", self._on_rescan_clicked)
        header_bar.pack_start(rescan_btn)

        # Apply & Activate Button in header
        self.apply_btn = Gtk.Button(label="Attiva Unisono")
        self.apply_btn.add_css_class("suggested-action")
        self.apply_btn.connect("clicked", self._on_toggle_unison_clicked)
        header_bar.pack_end(self.apply_btn)

        main_box.append(header_bar)

        # Status Banner
        self.banner = Adw.Banner()
        self.banner.set_use_markup(True)
        main_box.append(self.banner)

        # Preferences Page / Scrollable content
        scroll = Gtk.ScrolledWindow()
        scroll.set_vexpand(True)
        main_box.append(scroll)

        pref_page = Adw.PreferencesPage()
        scroll.set_child(pref_page)

        # Group 1: Speakers & Monitors
        self.speakers_group = Adw.PreferencesGroup()
        self.speakers_group.set_title("Monitor e Altoparlanti Rilevati")
        self.speakers_group.set_description(
            "Configura il ruolo (SX, DX, Subwoofer), il ritardo temporale e il volume per ogni dispositivo."
        )
        pref_page.add(self.speakers_group)

        self._populate_speakers_list()

        # Group 2: Crossover DSP & Tuning (2.1)
        dsp_group = Adw.PreferencesGroup()
        dsp_group.set_title("Filtro Crossover e Equalizzazione (2.1)")
        dsp_group.set_description(
            "Isola le basse frequenze sulla cassa aux/subwoofer e alleggerisce i monitor satelliti."
        )
        pref_page.add(dsp_group)

        # Crossover Enable Switch
        self.cross_row = Adw.SwitchRow()
        self.cross_row.set_title("Abilita Filtro Crossover 2.1")
        self.cross_row.set_subtitle("Invia le basse frequenze al Subwoofer e le medie/alte ai satelliti SX/DX")
        self.cross_row.set_active(self.config.crossover.enabled)
        self.cross_row.connect("notify::active", self._on_crossover_toggled)
        dsp_group.add(self.cross_row)

        # Crossover Frequency Slider
        self.freq_row = Adw.ActionRow()
        self.freq_row.set_title("Frequenza di Taglio (Crossover)")
        self.freq_row.set_subtitle(f"{self.config.crossover.frequency_hz} Hz")

        self.freq_scale = Gtk.Scale.new_with_range(Gtk.Orientation.HORIZONTAL, 40, 250, 5)
        self.freq_scale.set_value(self.config.crossover.frequency_hz)
        self.freq_scale.set_hexpand(True)
        self.freq_scale.set_size_request(200, -1)
        self.freq_scale.connect("value-changed", self._on_freq_changed)
        self.freq_row.add_suffix(self.freq_scale)
        dsp_group.add(self.freq_row)

        # Group 3: Master Control & Presets
        master_group = Adw.PreferencesGroup()
        master_group.set_title("Controllo Generale e Predefiniti")
        pref_page.add(master_group)

        # Set as default audio output switch
        self.default_switch = Adw.SwitchRow()
        self.default_switch.set_title("Imposta come Uscita Audio Predefinita di Sistema")
        self.default_switch.set_subtitle("Tutte le app (browser, spotify, giochi) useranno l'impianto unisono")
        self.default_switch.set_active(self.config.set_as_default)
        self.default_switch.connect("notify::active", self._on_default_toggled)
        master_group.add(self.default_switch)

        # Quick Preset Buttons
        preset_row = Adw.ActionRow()
        preset_row.set_title("Preset Rapidi")
        preset_row.set_subtitle("Carica impostazioni ottimali predefinite")

        p21_btn = Gtk.Button(label="Setup 2.1 Ottimale")
        p21_btn.set_valign(Gtk.Align.CENTER)
        p21_btn.connect("clicked", self._on_apply_preset_21)
        preset_row.add_suffix(p21_btn)

        master_group.add(preset_row)

    def _populate_speakers_list(self):
        for r in self._speaker_rows:
            self.speakers_group.remove(r)
        self._speaker_rows.clear()
        
        for ch in self.config.channels:
            row = SpeakerRow(ch, self._on_config_changed, self._on_test_speaker)
            self.speakers_group.add(row)
            self._speaker_rows.append(row)

    def _on_config_changed(self):
        self.preset_manager.save_config(self.config)
        if self.config.is_active or self.audio_service.is_running():
            self.audio_service.sync_active_branches()

    def _on_crossover_toggled(self, row, param):
        self.config.crossover.enabled = row.get_active()
        self._on_config_changed()

    def _on_freq_changed(self, scale):
        val = int(scale.get_value())
        self.config.crossover.frequency_hz = val
        self.freq_row.set_subtitle(f"{val} Hz")
        self._on_config_changed()

    def _on_default_toggled(self, row, param):
        self.config.set_as_default = row.get_active()
        self._on_config_changed()

    def _on_test_speaker(self, target, display_name=None):
        self.audio_service.test_tone(target)
        name_str = display_name or str(target)
        toast = Adw.Toast.new(f"Tono di test inviato a: {name_str}")
        self.toast_overlay.add_toast(toast)

    def _on_toggle_unison_clicked(self, btn):
        if self.config.is_active or self.audio_service.is_running():
            # Stop unison
            self.audio_service.deactivate_unison()
            self.config.is_active = False
            self.preset_manager.save_config(self.config)
            self._refresh_status_banner()
            self.toast_overlay.add_toast(Adw.Toast.new("Impianto all'unisono disattivato."))
        else:
            # Start unison
            active_channels = [c for c in self.config.channels if c.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)]
            if not active_channels:
                self.toast_overlay.add_toast(Adw.Toast.new("Nessun altoparlante attivo selezionato!"))
                return
            
            success = self.audio_service.activate_unison(self.config)
            if success:
                self.config.is_active = True
                self.preset_manager.save_config(self.config)
                self._refresh_status_banner()
                self.toast_overlay.add_toast(Adw.Toast.new("Impianto all'unisono ATTIVO con successo!"))
            else:
                self.toast_overlay.add_toast(Adw.Toast.new("Errore durante l'attivazione di PipeWire."))

    def _on_rescan_clicked(self, btn):
        self._sync_live_sinks()
        self._populate_speakers_list()
        self._on_config_changed()
        self.toast_overlay.add_toast(Adw.Toast.new("Elenco uscite audio aggiornato."))

    def _on_apply_preset_21(self, btn):
        # Auto-configure 2 monitors as L/R and USB/Aux as Subwoofer
        hdmi_sinks = [c for c in self.config.channels if "hdmi" in c.sink_name.lower()]
        usb_sinks = [c for c in self.config.channels if "usb" in c.sink_name.lower()]
        
        if len(hdmi_sinks) >= 2:
            hdmi_sinks[0].role = SpeakerRole.LEFT
            hdmi_sinks[1].role = SpeakerRole.RIGHT
        elif len(hdmi_sinks) == 1:
            hdmi_sinks[0].role = SpeakerRole.LEFT
            
        if usb_sinks:
            usb_sinks[0].role = SpeakerRole.SUBWOOFER
            
        self.config.crossover.enabled = True
        self.config.crossover.frequency_hz = 120
        self.cross_row.set_active(True)
        self.freq_scale.set_value(120)
        self._populate_speakers_list()
        self._on_config_changed()
        self.toast_overlay.add_toast(Adw.Toast.new("Preset 2.1 applicato!"))

    def _refresh_status_banner(self):
        if self.config.is_active:
            self.banner.set_title("<b>Stato: IMPIANTO ALL'UNISONO ATTIVO</b>")
            self.banner.set_revealed(True)
            self.apply_btn.set_label("Disattiva")
            self.apply_btn.remove_css_class("suggested-action")
            self.apply_btn.add_css_class("destructive-action")
        else:
            self.banner.set_title("Stato: Inattivo (Uscite audio separate standard)")
            self.banner.set_revealed(True)
            self.apply_btn.set_label("Attiva Unisono")
            self.apply_btn.remove_css_class("destructive-action")
            self.apply_btn.add_css_class("suggested-action")

