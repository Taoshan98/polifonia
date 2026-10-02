"""Audio Engine Service to coordinate discovery, routing, volume and test signals."""

import os
import subprocess
import threading
import time
import tempfile
import math
import struct
import wave
import re
from typing import List, Optional, Union, Dict
from core.models import SystemConfig, AudioSink, SpeakerRole, SpeakerConfig
from backend.pipewire_scanner import PipeWireScanner
from backend.pipewire_config import PipeWireConfigGenerator
from storage.settings_store import StorageService


class AudioEngineService:
    def __init__(self):
        self.scanner = PipeWireScanner()
        self.storage = StorageService()
        self.config: SystemConfig = self.storage.load()
        self._sync_processes: Dict[str, subprocess.Popen] = {}
        self._running_delays: Dict[str, float] = {}
        self._sync_lock = threading.Lock()
        self._master_module_id: Optional[str] = None
        self._prev_default_sink: Optional[int] = None
        self._is_active = False
        self._branch_seq = 0
        self._running_has_bt = False
        self._pending_gains: Dict[str, float] = {}
        self._pending_master_gain: Optional[float] = None
        self._gain_event = threading.Event()
        self._gain_worker = threading.Thread(target=self._process_gain_queue, daemon=True)
        self._gain_worker.start()
        self._last_sync_time = 0.0

    def update_config(self, config: SystemConfig):
        """Updates internal configuration reference and persists it."""
        self.config = config
        self.storage.save(self.config)

    def get_devices(self) -> List[AudioSink]:
        """Discovers current hardware sinks and synchronizes config list."""
        sinks = self.scanner.get_sinks()
        
        existing_channels = self.config.channels
        existing_map = {s.sink_name: s for s in existing_channels}
        new_channels = list(existing_channels)

        for sink in sinks:
            if sink.name in existing_map:
                # Update display name and dynamic hardware properties
                existing_map[sink.name].display_name = sink.description
                existing_map[sink.name].sink_id = sink.id
                existing_map[sink.name].hardware_latency_ms = sink.latency_ms
                existing_map[sink.name].bus_type = sink.bus_type
            else:
                role = SpeakerRole.EXCLUDED if (sink.is_internal or sink.bus_type == "bluetooth") else SpeakerRole.STEREO
                new_channels.append(
                    SpeakerConfig(
                        sink_id=sink.id,
                        sink_name=sink.name,
                        display_name=sink.description,
                        role=role,
                        hardware_latency_ms=sink.latency_ms,
                        bus_type=sink.bus_type
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
        if self._is_active:
            self._restart_single_branch(sink_name)

    def set_speaker_delay(self, sink_name: str, delay_ms: float):
        for spk in self.config.channels:
            if spk.sink_name == sink_name:
                spk.delay_ms = delay_ms
                break
        self.storage.save(self.config)
        if self._is_active:
            self._restart_single_branch(sink_name)

    def _process_gain_queue(self):
        """Worker thread to coalesce rapid volume fader changes and prevent Bluetooth AVRCP flooding."""
        while True:
            self._gain_event.wait()
            self._gain_event.clear()
            while True:
                with self._sync_lock:
                    if not self._pending_gains and self._pending_master_gain is None:
                        break
                    gains_batch = dict(self._pending_gains)
                    self._pending_gains.clear()
                    master_val = self._pending_master_gain
                    self._pending_master_gain = None

                for s_name, g in gains_batch.items():
                    try:
                        if g <= 0.001:
                            subprocess.run(["pactl", "set-sink-mute", s_name, "1"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=0.1)
                        else:
                            vol_pct = f"{int(g * 100)}%"
                            subprocess.run(["pactl", "set-sink-mute", s_name, "0"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=0.1)
                            subprocess.run(["pactl", "set-sink-volume", s_name, vol_pct], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=0.1)
                    except Exception:
                        pass

                if master_val is not None:
                    try:
                        m_pct = f"{int(master_val * 100)}%"
                        subprocess.run(["pactl", "set-sink-volume", "polifonia_master", m_pct], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=0.1)
                    except Exception:
                        pass

    def set_speaker_gain(self, sink_name: str, gain: float, persist: bool = True):
        for spk in self.config.channels:
            if spk.sink_name == sink_name:
                spk.volume_gain = max(0.0, min(1.5, gain))
                break
        if persist:
            self.storage.save(self.config)
        if self._is_active:
            with self._sync_lock:
                self._pending_gains[sink_name] = gain
            self._gain_event.set()

    def set_crossover(self, enabled: bool, freq: int):
        self.config.crossover_enabled = enabled
        self.config.crossover_freq = freq
        self.storage.save(self.config)

    def set_master_gain(self, gain: float, persist: bool = True):
        self.config.master_volume = max(0.0, min(1.5, gain))
        if persist:
            self.storage.save(self.config)
        if self._is_active:
            with self._sync_lock:
                self._pending_master_gain = self.config.master_volume
            self._gain_event.set()

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
        with self._sync_lock:
            for proc in self._sync_processes.values():
                try:
                    proc.terminate()
                    proc.wait(timeout=0.2)
                except Exception:
                    try:
                        proc.kill()
                    except Exception:
                        pass
            self._sync_processes.clear()
            self._running_delays.clear()

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

    def _ensure_master_sink(self) -> bool:
        """Ensures the polifonia_master virtual sink is loaded in PipeWire without interrupting active playback."""
        if self._master_module_id:
            return True

        # Check if already present in pactl list modules
        try:
            res = subprocess.run(["pactl", "list", "modules", "short"], capture_output=True, text=True)
            for line in res.stdout.splitlines():
                if "polifonia_master" in line and "null-sink" in line:
                    self._master_module_id = line.split()[0]
                    return True
        except Exception:
            pass

        # Save previous default sink, making sure it is a real hardware sink and not polifonia_master
        if self._prev_default_sink is None:
            cur_def = self.scanner.get_default_sink_id()
            try:
                def_name = subprocess.check_output(["pactl", "get-default-sink"], text=True).strip()
            except Exception:
                def_name = ""
            if "polifonia" not in def_name.lower():
                self._prev_default_sink = cur_def
            else:
                # Find first non-virtual physical sink
                real_sinks = self.scanner.get_sinks()
                if real_sinks:
                    self._prev_default_sink = real_sinks[0].id

        sink_desc = "Polifonia Audio Studio"
        cmd_master = [
            "pactl", "load-module", "module-null-sink",
            "sink_name=polifonia_master",
            "rate=48000",
            "channels=2",
            "channel_map=front-left,front-right",
            "sink_properties=device.description=Polifonia media.class=Audio/Sink node.always-process=true node.pause-on-idle=false"
        ]
        try:
            res = subprocess.run(cmd_master, capture_output=True, text=True, check=True)
            self._master_module_id = res.stdout.strip()
            time.sleep(0.05)

            # Set volume and default sink
            master_vol = f"{int(self.config.master_volume * 100)}%"
            subprocess.run(["pactl", "set-sink-volume", "polifonia_master", master_vol], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.run(["pactl", "set-sink-mute", "polifonia_master", "0"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            subprocess.run(["pactl", "set-default-sink", "polifonia_master"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            return True
        except Exception as e:
            print(f"Error creating master virtual sink: {e}")
            return False

    @staticmethod
    def _async_kill(proc: subprocess.Popen):
        """Asynchronously terminates a loopback process without blocking the UI main thread."""
        def _do_kill():
            try:
                proc.terminate()
                try:
                    proc.wait(timeout=0.1)
                except subprocess.TimeoutExpired:
                    proc.kill()
            except Exception:
                pass
        threading.Thread(target=_do_kill, daemon=True).start()

    @classmethod
    def _wake_sink(cls, sink_name: str):
        """Proactively unsuspends sleeping/suspended sinks and primes Bluetooth A2DP transport."""
        try:
            subprocess.run(["pactl", "suspend-sink", sink_name, "0"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=0.4)
            subprocess.run(["pactl", "set-sink-mute", sink_name, "0"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=0.4)
        except Exception:
            pass

        # If Bluetooth, prime the A2DP transport socket with a 40ms silent burst
        if "bluez" in sink_name.lower():
            tmp_path = None
            try:
                import wave, tempfile, os
                with tempfile.NamedTemporaryFile(suffix=".wav", delete=False) as tf:
                    tmp_path = tf.name
                with wave.open(tmp_path, "wb") as wf:
                    wf.setnchannels(2)
                    wf.setsampwidth(2)
                    wf.setframerate(44100)
                    wf.writeframes(b"\x00\x00\x00\x00" * 1764)
                subprocess.run(
                    ["pw-play", f"--target={sink_name}", tmp_path],
                    stdout=subprocess.DEVNULL,
                    stderr=subprocess.DEVNULL,
                    timeout=0.6
                )
            except Exception:
                pass
            finally:
                if tmp_path and os.path.exists(tmp_path):
                    try:
                        os.remove(tmp_path)
                    except Exception:
                        pass

    def _spawn_loopback(self, spk, sink_name: str, total_delay_ms: float, is_bt: bool, has_bt_active: bool):
        """Spawns an isolated pw-loopback process routing polifonia_master to the specified sink."""
        branch_name = f"polifonia_branch_{spk.sink_id}"
        target = spk.sink_name or str(spk.sink_id)
        cap_props = f"target.object=polifonia_master stream.capture.sink=true node.name=polifonia_cap_{spk.sink_id} node.dont-reconnect=true resample.disable=false resample.quality=4 audio.position=[FL,FR]"
        play_props = f"target.object={target} node.name=polifonia_play_{spk.sink_id} node.dont-reconnect=true resample.disable=false resample.quality=4 node.passive=false audio.position=[FL,FR]"

        cmd_loop = [
            "pw-loopback",
            f"--capture-props={cap_props}",
            f"--playback-props={play_props}",
            "-n", branch_name
        ]

        if not is_bt and not has_bt_active:
            cmd_loop.extend(["-l", "15"])

        if total_delay_ms > 0.05:
            cmd_loop.extend(["--delay", f"{total_delay_ms / 1000.0:.4f}"])

        def _set_pdeathsig():
            try:
                import ctypes, signal
                ctypes.CDLL("libc.so.6").prctl(1, signal.SIGTERM)
            except Exception:
                pass

        try:
            proc = subprocess.Popen(cmd_loop, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, preexec_fn=_set_pdeathsig)
            self._sync_processes[sink_name] = proc
            self._running_delays[sink_name] = total_delay_ms
        except Exception as e:
            print(f"Error starting loopback for {target}: {e}")

    def sync_active_branches(self):
        """Seamlessly starts or stops individual speaker loopbacks with clean crystal audio without stopping main playback."""
        with self._sync_lock:
            if not self._is_active:
                return

            self._last_sync_time = time.time()
            live_sinks = self.get_available_sinks()
            live_sink_names = {s.name for s in live_sinks}

            # Only stream to valid physical endpoints that actually exist in the audio topology
            active_speakers = {
                s.sink_name: s for s in self.config.channels
                if s.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)
                and (not live_sink_names or s.sink_name in live_sink_names)
            }

            # Determine maximum hardware latency across active speakers for acoustic auto-alignment
            max_hw_latency = max((getattr(s, "hardware_latency_ms", 0.0) for s in active_speakers.values()), default=0.0)

            user_min_delay = min(0.0, min((s.delay_ms for s in active_speakers.values()), default=0.0))

            has_bt_active = any(
                (getattr(s, "bus_type", "") == "bluetooth") or ("bluez" in s.sink_name.lower())
                for s in active_speakers.values()
            )
            bt_mode_changed = (has_bt_active != self._running_has_bt)
            self._running_has_bt = has_bt_active

            # Phase 1: Identify removed branches and branches requiring (re)launch
            to_terminate = [s_name for s_name in self._sync_processes.keys() if s_name not in active_speakers]

            to_launch = {}
            for sink_name, spk in active_speakers.items():
                hw_lat = getattr(spk, "hardware_latency_ms", 0.0)
                auto_comp_delay_ms = max(0.0, max_hw_latency - hw_lat)
                user_offset_delay_ms = spk.delay_ms - user_min_delay
                total_delay_ms = user_offset_delay_ms + auto_comp_delay_ms
                is_bt = (getattr(spk, "bus_type", "") == "bluetooth") or ("bluez" in sink_name.lower())

                current_proc = self._sync_processes.get(sink_name)
                proc_dead = (current_proc is None) or (current_proc.poll() is not None)
                delay_changed = abs(self._running_delays.get(sink_name, -1.0) - total_delay_ms) > 0.5
                needs_launch = proc_dead or delay_changed or bt_mode_changed

                if needs_launch:
                    to_launch[sink_name] = (spk, total_delay_ms, is_bt)

            # Phase 2: Batch termination of obsolete or restarting processes FIRST
            # Batch terminating prevents interleaved kills while another stream is negotiating.
            restart_or_stop_sinks = set(to_terminate).union(to_launch.keys())
            if restart_or_stop_sinks:
                for s_name in restart_or_stop_sinks:
                    proc = self._sync_processes.pop(s_name, None)
                    self._running_delays.pop(s_name, None)
                    if proc:
                        try:
                            proc.terminate()
                            try:
                                proc.wait(timeout=0.08)
                            except subprocess.TimeoutExpired:
                                proc.kill()
                        except Exception:
                            pass
                    try:
                        subprocess.run(["pkill", "-9", "-f", f"pw-loopback.*target.object={s_name}"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    except Exception:
                        pass

                # Graph settle time: allow PipeWire graph driver election to stabilize before launching streams
                time.sleep(0.06)

            # Phase 3: Launch Bluetooth endpoints FIRST and prime their A2DP transport
            bt_launches = {k: v for k, v in to_launch.items() if v[2]}
            wired_launches = {k: v for k, v in to_launch.items() if not v[2]}

            for sink_name, (spk, total_delay_ms, is_bt) in bt_launches.items():
                self._wake_sink(sink_name)

                # Ensure minimum audible volume gain for Bluetooth (avoid cubic attenuation silence)
                target_gain = max(0.50, spk.volume_gain)
                spk.volume_gain = target_gain
                try:
                    vol_pct = f"{int(target_gain * 100)}%"
                    subprocess.run(["pactl", "set-sink-mute", sink_name, "0"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=0.3)
                    subprocess.run(["pactl", "set-sink-volume", sink_name, vol_pct], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=0.3)
                except Exception:
                    pass

                self._spawn_loopback(spk, sink_name, total_delay_ms, is_bt=True, has_bt_active=has_bt_active)
                time.sleep(0.12)  # Give BlueZ socket time to lock to graph before launching wired clocks

            # Phase 4: Launch wired endpoints
            for sink_name, (spk, total_delay_ms, is_bt) in wired_launches.items():
                self._wake_sink(sink_name)
                try:
                    vol_pct = f"{int(max(0.10, spk.volume_gain) * 100)}%"
                    subprocess.run(["pactl", "set-sink-mute", sink_name, "0"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=0.3)
                    subprocess.run(["pactl", "set-sink-volume", sink_name, vol_pct], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=0.3)
                except Exception:
                    pass

                self._spawn_loopback(spk, sink_name, total_delay_ms, is_bt=False, has_bt_active=has_bt_active)

    def _restart_single_branch(self, sink_name: str):
        """Restarts only a single speaker's loopback for delay/role changes smoothly."""
        with self._sync_lock:
            if sink_name in self._sync_processes:
                proc = self._sync_processes.pop(sink_name)
                self._running_delays.pop(sink_name, None)
                self._async_kill(proc)
        self.sync_active_branches()

    def _start_watchdog(self):
        """Monitors active loopback streams and automatically recovers any dropped endpoints."""
        if hasattr(self, "_watchdog_thread") and self._watchdog_thread and self._watchdog_thread.is_alive():
            return
        self._watchdog_stop_event = threading.Event()

        def _watchdog_loop():
            while not self._watchdog_stop_event.is_set():
                time.sleep(2.0)
                if not self._is_active:
                    continue
                if time.time() - self._last_sync_time < 2.0:
                    continue
                live_sinks = self.get_available_sinks()
                live_sink_names = {s.name for s in live_sinks}
                active_sink_names = {
                    s.sink_name for s in self.config.channels
                    if s.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)
                    and (not live_sink_names or s.sink_name in live_sink_names)
                }
                has_dead = False
                with self._sync_lock:
                    for s_name in active_sink_names:
                        p = self._sync_processes.get(s_name)
                        if p is None or p.poll() is not None:
                            has_dead = True
                            break
                if has_dead:
                    try:
                        self.sync_active_branches()
                    except Exception:
                        pass

        self._watchdog_thread = threading.Thread(target=_watchdog_loop, daemon=True)
        self._watchdog_thread.start()

    def _stop_watchdog(self):
        """Stops the loopback stream recovery watchdog."""
        if hasattr(self, "_watchdog_stop_event"):
            self._watchdog_stop_event.set()

    def start_unison_sink(self) -> bool:
        """Starts unison: ensures master sink is loaded and starts active speaker branches."""
        active_speakers = [s for s in self.config.channels if s.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)]
        if not active_speakers:
            return False

        if not self._ensure_master_sink():
            return False

        self._is_active = True
        self.sync_active_branches()
        self._start_watchdog()
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
        self._stop_watchdog()
        self._cleanup_all_orphan_nodes()

        # Restore previous default system audio sink
        if self._prev_default_sink:
            try:
                subprocess.run(["wpctl", "set-default", str(self._prev_default_sink)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            except Exception:
                pass
            self._prev_default_sink = None

        self._is_active = False

    def get_routing_status(self) -> dict:
        """Returns comprehensive real-time audio routing graph data (sources, master DSP, destinations)."""
        sink_states = self.scanner.get_sink_states()
        sink_inputs = self.scanner.get_active_sink_inputs()
        live_sinks = self.get_available_sinks()
        live_sink_names = {s.name for s in live_sinks}

        master_state = sink_states.get("polifonia_master", "STOPPED" if not self._is_active else "IDLE")

        active_speakers = [
            s for s in self.config.channels
            if s.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)
            and (not live_sink_names or s.sink_name in live_sink_names)
        ]
        max_hw_latency = max((getattr(s, "hardware_latency_ms", 0.0) for s in active_speakers), default=0.0)
        user_min_delay = min(0.0, min((s.delay_ms for s in active_speakers), default=0.0))

        destinations = []
        for ch in self.config.channels:
            if live_sink_names and ch.sink_name not in live_sink_names:
                continue
            is_enabled = ch.role not in (SpeakerRole.EXCLUDED, SpeakerRole.DISABLED)
            hw_lat = getattr(ch, "hardware_latency_ms", 0.0)
            auto_comp_delay_ms = max(0.0, max_hw_latency - hw_lat) if is_enabled else 0.0
            user_offset_delay_ms = ch.delay_ms - user_min_delay if is_enabled else 0.0
            total_delay_ms = user_offset_delay_ms + auto_comp_delay_ms if is_enabled else 0.0

            state = sink_states.get(ch.sink_name, sink_states.get(str(ch.sink_id), "SUSPENDED"))
            with self._sync_lock:
                proc = self._sync_processes.get(ch.sink_name)
                proc_running = (proc is not None) and (proc.poll() is None)

            is_streaming = self._is_active and is_enabled and (proc_running or state == "RUNNING")

            destinations.append({
                "sink_id": ch.sink_id,
                "sink_name": ch.sink_name,
                "display_name": ch.custom_name or ch.display_name or ch.sink_name,
                "bus_type": getattr(ch, "bus_type", "other"),
                "role": ch.role.value if hasattr(ch.role, "value") else str(ch.role),
                "volume_gain": ch.volume_gain,
                "delay_ms": ch.delay_ms,
                "hardware_latency_ms": hw_lat,
                "auto_comp_delay_ms": auto_comp_delay_ms,
                "total_delay_ms": total_delay_ms,
                "is_enabled": is_enabled,
                "is_streaming": is_streaming,
                "state": state
            })

        return {
            "is_unison_active": self._is_active,
            "master_volume": self.config.master_volume,
            "master_state": master_state,
            "sources": sink_inputs,
            "destinations": destinations,
            "max_hw_latency": max_hw_latency
        }


# Aliases
AudioService = AudioEngineService

