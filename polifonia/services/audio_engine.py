"""Audio Engine Service to coordinate discovery, routing, volume and test signals."""

import os
import subprocess
import threading
import time
import tempfile
import math
import struct
import wave
from typing import List, Optional
from polifonia.core.models import SystemConfig, AudioSink, SpeakerRole, SpeakerConfig
from polifonia.backend.pipewire_scanner import PipeWireScanner
from polifonia.backend.pipewire_config import PipeWireConfigGenerator
from polifonia.storage.settings_store import StorageService


class AudioEngineService:
    def __init__(self):
        self.scanner = PipeWireScanner()
        self.storage = StorageService()
        self.config: SystemConfig = self.storage.load()
        self._sync_processes = []
        self._is_active = False

    def get_devices(self) -> List[AudioSink]:
        """Discovers current hardware sinks and synchronizes config list."""
        sinks = self.scanner.get_sinks()
        
        existing_keys = {s.sink_name for s in self.config.speakers}
        for sink in sinks:
            if sink.node_name not in existing_keys:
                role = SpeakerRole.EXCLUDED if "pci-0000" in sink.node_name and "analog" in sink.node_name else SpeakerRole.LEFT
                self.config.speakers.append(
                    SpeakerConfig(
                        sink_id=sink.id,
                        sink_name=sink.node_name,
                        display_name=sink.description,
                        role=role
                    )
                )
        self.storage.save(self.config)
        return sinks

    def set_speaker_role(self, sink_name: str, role: SpeakerRole):
        for spk in self.config.speakers:
            if spk.sink_name == sink_name:
                spk.role = role
                break
        self.storage.save(self.config)

    def set_speaker_delay(self, sink_name: str, delay_ms: float):
        for spk in self.config.speakers:
            if spk.sink_name == sink_name:
                spk.delay_ms = delay_ms
                break
        self.storage.save(self.config)

    def set_speaker_gain(self, sink_name: str, gain: float):
        for spk in self.config.speakers:
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

    def play_test_tone(self, sink_id: int, freq: int = 440, duration: float = 0.6):
        """Generates a test tone directly on a specific sink using a synthetic WAV."""
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
                            val = int(32767.0 * 0.4 * env * math.sin(2.0 * math.pi * freq * i / sample_rate))
                            data = struct.pack("<hh", val, val)
                            wav_file.writeframesraw(data)
                
                subprocess.run(["pw-play", f"--target={sink_id}", wav_path], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                if os.path.exists(wav_path):
                    os.remove(wav_path)
            except Exception as e:
                print(f"Error playing test tone: {e}")
        threading.Thread(target=_play, daemon=True).start()

    def start_unison_sink(self) -> bool:
        """Starts the combine-sink / filter-chain module."""
        self.stop_unison_sink()
        
        active_speakers = [s for s in self.config.speakers if s.role != SpeakerRole.EXCLUDED]
        if not active_speakers:
            return False

        for spk in active_speakers:
            latency_str = f"{int(spk.delay_ms)}/1000" if spk.delay_ms > 0 else "10/1000"
            cmd = [
                "pw-loopback",
                f"--capture-props=node.name=polifonia_sub_{spk.sink_id} media.class=Audio/Sink audio.position=[ FL FR ]",
                f"--target-object={spk.sink_id}",
                f"--latency={latency_str}"
            ]
            try:
                proc = subprocess.Popen(cmd, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                self._sync_processes.append(proc)
            except Exception as e:
                print(f"Error starting loopback for {spk.sink_id}: {e}")

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

# Aliases
AudioService = AudioEngineService
