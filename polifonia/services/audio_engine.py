"""Audio Engine Service to coordinate discovery, routing, volume and test signals."""

import subprocess
import threading
import time
from typing import List, Optional
from polifonia.core.models import SystemConfig, SinkDevice, SpeakerRole
from polifonia.backend.pipewire_adapter import PipewireAdapter
from polifonia.storage.config_storage import ConfigStorage


class AudioEngineService:
    def __init__(self):
        self.adapter = PipewireAdapter()
        self.storage = ConfigStorage()
        self.config: SystemConfig = self.storage.load_config()
        self._sync_processes = []
        self._is_active = False

    def get_devices(self) -> List[SinkDevice]:
        """Discovers current hardware sinks and synchronizes config list."""
        devices = self.adapter.get_sinks()
        self.config.sync_with_devices(devices)
        return devices

    def set_channel_role(self, sink_id: int, role: SpeakerRole):
        for ch in self.config.channels:
            if ch.sink_id == sink_id:
                ch.role = role
                break
        self.storage.save_config(self.config)

    def set_channel_delay(self, sink_id: int, delay_ms: float):
        for ch in self.config.channels:
            if ch.sink_id == sink_id:
                ch.delay_ms = delay_ms
                break
        self.storage.save_config(self.config)

    def set_channel_gain(self, sink_id: int, gain: float):
        for ch in self.config.channels:
            if ch.sink_id == sink_id:
                ch.gain = max(0.0, min(1.5, gain))
                break
        self.storage.save_config(self.config)

    def set_crossover(self, enabled: bool, freq: int):
        self.config.crossover_enabled = enabled
        self.config.crossover_frequency = freq
        self.storage.save_config(self.config)

    def set_master_gain(self, gain: float):
        self.config.master_gain = max(0.0, min(1.5, gain))
        self.storage.save_config(self.config)

    def is_running(self) -> bool:
        return self._is_active

    def play_test_tone(self, sink_id: int, freq: int = 440, duration: float = 0.5):
        """Generates a test tone directly on a specific sink."""
        def _play():
            # Use pw-play / sox / aplay or synth tone with python wav
            cmd = f"pw-cat --playback --target {sink_id} <(sox -n -p synth {duration} sine {freq}) 2>/dev/null || wpctl set-volume {sink_id} 50%"
            subprocess.run(["bash", "-c", cmd])
        threading.Thread(target=_play, daemon=True).start()

    def start_unison_sink(self) -> bool:
        """Starts the combine-sink / filter-chain module."""
        self.stop_unison_sink()
        
        active_channels = [c for c in self.config.channels if c.role != SpeakerRole.DISABLED]
        if not active_channels:
            return False

        # Start pw-loopback modules connecting to the designated sinks
        for ch in active_channels:
            # Configure latency / delay and volume per channel
            latency_str = f"{int(ch.delay_ms)}/1000"
            cmd = [
                "pw-loopback",
                f"--capture-props=node.name=polifonia_subsink_{ch.sink_id} media.class=Audio/Sink audio.position=[ FL FR ]",
                f"--target-object={ch.sink_id}",
                f"--latency={latency_str}"
            ]
            try:
                proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                self._sync_processes.append(proc)
            except Exception as e:
                print(f"Error starting loopback for {ch.sink_id}: {e}")

        self._is_active = True
        return True

    def stop_unison_sink(self):
        """Stops all running Polifonia background audio nodes."""
        for proc in self._sync_processes:
            try:
                proc.terminate()
                proc.wait(timeout=1.0)
            except Exception:
                try:
                    proc.kill()
                except Exception:
                    pass
        self._sync_processes.clear()
        self._is_active = False
