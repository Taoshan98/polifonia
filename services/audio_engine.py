"""Audio Engine Service to coordinate discovery, routing, volume and test signals."""

import os
import subprocess
import threading
import time
import tempfile
import math
import struct
import wave
from typing import List, Optional, Union
from core.models import SystemConfig, AudioSink, SpeakerRole, SpeakerConfig
from backend.pipewire_scanner import PipeWireScanner
from backend.pipewire_config import PipeWireConfigGenerator
from storage.settings_store import StorageService


class AudioEngineService:
    def __init__(self):
        self.scanner = PipeWireScanner()
        self.storage = StorageService()
        self.config: SystemConfig = self.storage.load()
        self._sync_processes = []
        self._master_module_id: Optional[str] = None
        self._prev_default_sink: Optional[int] = None
        self._is_active = False

    def get_devices(self) -> List[AudioSink]:
        """Discovers current hardware sinks and synchronizes config list."""
        sinks = self.scanner.get_sinks()
        
        existing_channels = self.config.channels
        existing_map = {s.sink_name: s for s in existing_channels}
        new_channels = list(existing_channels)

        for sink in sinks:
            if sink.name in existing_map:
                # Update display name if improved
                existing_map[sink.name].display_name = sink.description
                existing_map[sink.name].sink_id = sink.id
            else:
                role = SpeakerRole.EXCLUDED if sink.is_internal else SpeakerRole.LEFT
                new_channels.append(
                    SpeakerConfig(
                        sink_id=sink.id,
                        sink_name=sink.name,
                        display_name=sink.description,
                        role=role
                    )
                )
        self.config.channels = new_channels
        self.storage.save(self.config)
        return sinks

    def get_available_sinks(self) -> List[AudioSink]:
        return self.scanner.scan_sinks()

    def set_speaker_role(self, sink_name: str, role: SpeakerRole):
        for spk in self.config.channels:
            if spk.sink_name == sink_name:
                spk.role = role
                break
        self.storage.save(self.config)

    def set_speaker_delay(self, sink_name: str, delay_ms: float):
        for spk in self.config.channels:
            if spk.sink_name == sink_name:
                spk.delay_ms = delay_ms
                break
        self.storage.save(self.config)

    def set_speaker_gain(self, sink_name: str, gain: float):
        for spk in self.config.channels:
            if spk.sink_name == sink_name:
                spk.volume_gain = max(0.0, min(1.5, gain))
                break
        self.storage.save(self.config)

    def set_crossover(self, enabled: bool, freq: int):
        self.config.crossover_enabled = enabled
        self.config.crossover_freq = freq
        self.storage.save(self.config)

    def set_master_gain(self, gain: float):
        self.config.master_volume = max(0.0, min(1.5, gain))
        self.storage.save(self.config)

    def is_running(self) -> bool:
        return self._is_active

    def play_test_tone(self, target: Union[int, str] = 0, freq: int = 440, duration: float = 0.6, sink_id: Optional[Union[int, str]] = None):
        """Generates a test tone directly on a specific sink name or ID."""
        actual_target = target if target != 0 else (sink_id or 0)
        def _play():
            try:
                sample_rate = 44100
                num_samples = int(sample_rate * duration)
                with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as f:
                    wav_path = f.name
                    with wave.open(wav_path, "w") as wav_file:
                        wav_file.setnchannels(2)
                        wav_file.setsampwidth(2)
                        wav_file.setframerate(sample_rate)
                        for i in range(num_samples):
                            env = 1.0
                            if i < 441:
                                env = i / 441.0
                            elif i > num_samples - 441:
                                env = (num_samples - i) / 441.0
                            val = int(32767.0 * 0.5 * env * math.sin(2.0 * math.pi * freq * i / sample_rate))
                            data = struct.pack("<hh", val, val)
                            wav_file.writeframesraw(data)
                
                subprocess.run(["pw-play", f"--target={actual_target}", wav_path], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                if os.path.exists(wav_path):
                    os.remove(wav_path)
            except Exception as e:
                print(f"Error playing test tone on {actual_target}: {e}")
        threading.Thread(target=_play, daemon=True).start()

    def test_tone(self, target: Union[int, str] = 0, freq: int = 440, duration: float = 0.6, sink_id: Optional[Union[int, str]] = None):
        self.play_test_tone(target=target, freq=freq, duration=duration, sink_id=sink_id)

    def _cleanup_all_orphan_nodes(self):
        """Kills any orphan loopback processes and unloads all polifonia master modules."""
        for proc in self._sync_processes:
            try:
                proc.terminate()
                proc.wait(timeout=0.2)
            except Exception:
                try:
                    proc.kill()
                except Exception:
                    pass
        self._sync_processes.clear()

        try:
            subprocess.run(["pkill", "-f", "pw-loopback.*polifonia"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except Exception:
            pass

        try:
            res = subprocess.run(["pactl", "list", "modules", "short"], capture_output=True, text=True)
            for line in res.stdout.splitlines():
                if "polifonia" in line or ("null-sink" in line and "polifonia_master" in line):
                    mod_id = line.split()[0]
                    subprocess.run(["pactl", "unload-module", mod_id], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except Exception:
            pass

        self._master_module_id = None

    def start_unison_sink(self) -> bool:
        """
        Creates a Master Virtual Sink exposed to the OS, sets it as the default output,
        and broadcasts audio simultaneously to all selected hardware speakers.
        """
        self.stop_unison_sink()
        
        active_speakers = [s for s in self.config.channels if s.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)]
        if not active_speakers:
            return False

        # 1. Save currently active default sink for restoration on deactivation
        self._prev_default_sink = self.scanner.get_default_sink_id()

        # 2. Create the Master Virtual Sink via PipeWire Pulse module
        sink_desc = "Polifonia Audio Studio (2.1)"
        cmd_master = [
            "pactl", "load-module", "module-null-sink",
            "sink_name=polifonia_master",
            f'sink_properties=device.description="{sink_desc}" media.class=Audio/Sink'
        ]
        try:
            res = subprocess.run(cmd_master, capture_output=True, text=True, check=True)
            self._master_module_id = res.stdout.strip()
        except Exception as e:
            print(f"Error creating master virtual sink: {e}")
            self._master_module_id = None

        time.sleep(0.1)

        # 3. Create real-time audio loopbacks from polifonia_master to each active hardware sink
        for spk in active_speakers:
            latency_ms = max(1, int(spk.delay_ms)) if spk.delay_ms > 0 else 10
            target = spk.sink_name or str(spk.sink_id)

            # Ensure the physical destination is unmuted and has audible volume
            try:
                subprocess.run(["pactl", "set-sink-mute", target, "0"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                vol_pct = f"{max(10, int(spk.volume_gain * 100))}%"
                subprocess.run(["pactl", "set-sink-volume", target, vol_pct], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            except Exception:
                pass

            cap_props = f"target.object=polifonia_master stream.capture.sink=true node.name=polifonia_cap_{spk.sink_id}"
            play_props = f"target.object={target} node.name=polifonia_play_{spk.sink_id} node.passive=true"

            if spk.role == SpeakerRole.LEFT:
                cap_props += " audio.position=[ FL ]"
                play_props += " audio.position=[ FL, FR ]"
            elif spk.role == SpeakerRole.RIGHT:
                cap_props += " audio.position=[ FR ]"
                play_props += " audio.position=[ FL, FR ]"
            elif spk.role in (SpeakerRole.SUBWOOFER, SpeakerRole.CENTER):
                cap_props += " audio.position=[ FL, FR ]"
                play_props += " audio.position=[ FL, FR ]"

            cmd_loop = [
                "pw-loopback",
                f"--capture-props={cap_props}",
                f"--playback-props={play_props}",
                "-l", str(latency_ms),
                "-n", f"polifonia_branch_{spk.sink_id}"
            ]

            try:
                proc = subprocess.Popen(cmd_loop, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                self._sync_processes.append(proc)
            except Exception as e:
                print(f"Error starting loopback for {target} ({spk.sink_id}): {e}")

        # 4. Set polifonia_master volume to 100% unmuted and make it the default system output
        try:
            master_vol = f"{int(self.config.master_volume * 100)}%"
            subprocess.run(["pactl", "set-sink-volume", "polifonia_master", master_vol], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.run(["pactl", "set-sink-mute", "polifonia_master", "0"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.run(["pactl", "set-default-sink", "polifonia_master"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except Exception as e:
            print(f"Error configuring polifonia_master default sink: {e}")

        self._is_active = True
        return True

    def activate_unison(self, config: Optional[SystemConfig] = None) -> bool:
        if config:
            self.config = config
        return self.start_unison_sink()

    def deactivate_unison(self) -> bool:
        self.stop_unison_sink()
        return True

    def stop_unison_sink(self):
        """Stops all running loopback nodes, unloads master virtual sink, and restores previous default output."""
        self._cleanup_all_orphan_nodes()

        # Restore previous default system audio sink
        if self._prev_default_sink:
            try:
                subprocess.run(["wpctl", "set-default", str(self._prev_default_sink)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            except Exception:
                pass
            self._prev_default_sink = None

        self._is_active = False


# Aliases
AudioService = AudioEngineService

