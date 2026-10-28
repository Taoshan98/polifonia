"""Inspects system hardware and PipeWire topology to detect audio sinks and roles."""

import glob
import json
import os
import re
import subprocess
import sys
import time
from typing import List, Optional
from core.models import AudioSink


class DeviceScanner:
    _last_profile_check: float = 0.0
    _last_eld_check: float = 0.0
    _cached_eld_map: dict = {}
    _last_scan_time: float = 0.0
    _cached_sinks: List[AudioSink] = []

    @staticmethod
    def get_default_sink_id() -> Optional[int]:
        try:
            res = subprocess.run(["wpctl", "status"], capture_output=True, text=True, timeout=1.5)
            for line in res.stdout.splitlines():
                if "*" in line and ("Audio/Sink" in line or ("[" in line and "vol:" in line)):
                    match = re.search(r"\*\s+(\d+)\.", line)
                    if match:
                        return int(match.group(1))
        except Exception:
            pass
        return None

    @classmethod
    def get_eld_monitors(cls, force: bool = False) -> dict:
        """Dynamically parses kernel ALSA ELD files for all sound cards and connected video displays."""
        now = time.time()
        if not force and cls._cached_eld_map and (now - cls._last_eld_check < 15.0):
            return cls._cached_eld_map
        cls._last_eld_check = now

        eld_map = {}
        for eld_path in sorted(glob.glob("/proc/asound/card*/eld*")):
            try:
                with open(eld_path, "r") as f:
                    content = f.read()
                if re.search(r"monitor_present\s+1", content):
                    m_name = re.search(r"monitor_name\s+(.+)", content)
                    conn = re.search(r"connection_type\s+(.+)", content)
                    ch_m = re.search(r"sad0_channels\s+(\d+)", content)
                    c_match = re.search(r"card(\d+)", eld_path)
                    p_match = re.search(r"eld#\d+\.(\d+)", eld_path)
                    if c_match:
                        eld_map.setdefault(int(c_match.group(1)), []).append({
                            "name": m_name.group(1).strip() if m_name else "Display Audio",
                            "connection": conn.group(1).strip() if conn else "HDMI",
                            "channels": int(ch_m.group(1)) if ch_m else 2,
                            "pin": int(p_match.group(1)) if p_match else 0,
                        })
            except Exception:
                pass
        cls._cached_eld_map = eld_map
        return eld_map

    @staticmethod
    def _init_card_iec958(card_idx: int):
        """Initializes consumer LPCM IEC958 status bits on HDA HDMI/DisplayPort converters."""
        for idx in range(4):
            try:
                subprocess.run(
                    ["iecset", "-c", str(card_idx), "-n", str(idx), "audio", "on", "pro", "off", "orig", "on", "category", "0x02", "rate", "48000"],
                    stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=0.4,
                )
            except Exception:
                pass

    @classmethod
    def ensure_multimonitor_card_profiles(cls):
        """Ensures multi-monitor GPU sound cards are set to pro-audio with valid 2ch stereo converters."""
        now = time.time()
        if cls._last_profile_check and (now - cls._last_profile_check < 30.0):
            return
        cls._last_profile_check = now

        try:
            eld_map = cls.get_eld_monitors()
            if not any(len(mons) > 1 for mons in eld_map.values()):
                return
            card_info = subprocess.check_output(["pactl", "list", "cards"], text=True, timeout=1.0)
            try:
                sinks_short = subprocess.check_output(["pactl", "list", "sinks", "short"], text=True, timeout=1.0)
            except Exception:
                sinks_short = ""

            for block in card_info.split("Card #"):
                name_m = re.search(r"Name:\s+(\S+)", block)
                if not name_m:
                    continue
                card_name = name_m.group(1)
                cn_low = card_name.lower()
                if any(x in cn_low for x in ("sof", "realtek", "00_1f.3")):
                    continue

                card_idx_m = re.search(r'alsa\.card\s+=\s+"(\d+)"', block)
                card_idx = int(card_idx_m.group(1)) if card_idx_m else 0
                mons = eld_map.get(card_idx, [])
                card_prefix = card_name.replace("alsa_card.", "alsa_output.")
                two_ch_pro = sum(
                    1 for line in sinks_short.splitlines()
                    if card_prefix in line and "pro-output" in line and "2ch" in line
                ) if sinks_short else len(mons)

                is_pro = "Active Profile: pro-audio" in block
                if not is_pro or (mons and two_ch_pro < len(mons)):
                    if is_pro:
                        subprocess.run(
                            ["pactl", "set-card-profile", card_name, "off"],
                            stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=1.0,
                        )
                        time.sleep(0.08)
                    subprocess.run(
                        ["pactl", "set-card-profile", card_name, "pro-audio"],
                        stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=1.0,
                    )
                    time.sleep(0.08)
                    cls._init_card_iec958(card_idx)
                    cls._cached_sinks = []
                elif mons:
                    cls._init_card_iec958(card_idx)
        except Exception:
            pass

    @classmethod
    def scan_sinks(cls, force: bool = False) -> List[AudioSink]:
        """Scans PipeWire graph for active audio sinks with dynamic hardware and telemetry classification."""
        is_test = "unittest" in sys.modules or bool(os.environ.get("PYTEST_CURRENT_TEST"))
        now = time.time()
        if not is_test and not force and cls._cached_sinks and (now - cls._last_scan_time < 2.5):
            return list(cls._cached_sinks)

        if not is_test:
            cls.ensure_multimonitor_card_profiles()
        eld_map = cls.get_eld_monitors(force=force)
        sinks: List[AudioSink] = []
        try:
            objects = json.loads(subprocess.check_output(["pw-dump"], text=True, timeout=2.0))
            if not isinstance(objects, list):
                return sinks
        except Exception:
            return sinks

        card_allocated_monitors = {}
        for obj in objects:
            if obj.get("type") != "PipeWire:Interface:Node":
                continue
            info = obj.get("info", {})
            props = info.get("props", {})
            if props.get("media.class") != "Audio/Sink":
                continue

            name = props.get("node.name", "")
            name_lower = name.lower()
            if "polifonia" in name_lower or "filter-chain" in name_lower:
                continue

            node_id = obj.get("id")
            card_id = props.get("alsa.card")
            raw_desc = props.get("node.nick") or props.get("node.description") or props.get("device.description") or name
            raw_desc_lower = raw_desc.lower()
            bus = props.get("device.bus", "").lower()
            api_lower = props.get("device.api", "").lower()
            form_factor = props.get("device.form-factor", "").lower()

            is_bluetooth = bus == "bluetooth" or "bluez" in name_lower or "bluez" in api_lower or "bluetooth" in props.get("device.icon_name", "").lower()
            is_network = bus == "network" or any(k in name_lower or k in api_lower for k in ("raop", "airplay", "rtp", "roc", "snapcast", "cast", "tunnel"))
            is_usb = bus == "usb" or "usb" in name_lower
            is_digital_hdmi = not is_usb and any(k in name_lower for k in ("hdmi", "displayport", "pro-output"))
            is_spdif = not is_digital_hdmi and (any(k in name_lower for k in ("iec958", "spdif")) or any(k in raw_desc_lower for k in ("optical", "digital")))

            is_internal = False
            if is_bluetooth:
                clean_name = (
                    props.get("device.alias") or props.get("device.description") or props.get("node.description")
                    or props.get("media.name") or props.get("node.nick") or props.get("device.product.name") or "Bluetooth Device"
                ).strip()
                for prefix in ("Bluetooth Audio (", "Bluetooth ("):
                    if clean_name.startswith(prefix) and clean_name.endswith(")"):
                        clean_name = clean_name[len(prefix):-1].strip()
                description, bus_type = clean_name, "bluetooth"
            elif is_network:
                clean_name = (props.get("node.description") or props.get("device.description") or props.get("media.name") or raw_desc).strip()
                description = f"Network Audio ({clean_name})" if "network" not in clean_name.lower() else clean_name
                bus_type = "network"
            elif is_digital_hdmi:
                bus_type = "hdmi"
                c_num = int(card_id) if card_id is not None else None
                monitors = eld_map.get(c_num, []) if c_num is not None else []
                ch_cnt = int(props.get("audio.channels") or 2)
                if "pro-output" in name_lower and ch_cnt > 2 and any(m.get("channels", 2) <= 2 for m in monitors):
                    continue
                curr_idx = card_allocated_monitors.get(c_num, 0)
                if curr_idx >= len(monitors):
                    continue
                mon = monitors[curr_idx]
                card_allocated_monitors[c_num] = curr_idx + 1
                description = f"Monitor {mon['name']} ({mon['connection']})"
            elif is_usb:
                clean_name = props.get("node.nick") or props.get("device.product.name") or raw_desc.replace("Analog Stereo", "").replace("Stereo analogico", "").strip()
                description, bus_type = f"USB Audio ({clean_name})", "usb"
            elif is_spdif:
                clean_name = props.get("node.nick") or props.get("device.product.name") or raw_desc.replace("Digital Stereo (IEC958)", "").strip() or "S/PDIF"
                description, bus_type = f"Digital S/PDIF ({clean_name})", "spdif"
            elif "headphone" in raw_desc_lower or "headphone" in name_lower:
                clean_name = props.get("node.nick") or props.get("device.product.name") or "Analog"
                description, bus_type = f"Headphones / Audio Jack ({clean_name})", "jack"
            elif "speaker" in raw_desc_lower or "speaker" in name_lower or form_factor == "internal":
                description, bus_type, is_internal = "Integrated Speakers (Internal)", "internal", True
            else:
                description, bus_type = raw_desc, (bus or "pci")

            # Extract hardware latency in ms from SPA Latency and ProcessLatency parameters
            latency_ms = 0.0
            params = info.get("params", {})
            for lat in params.get("Latency", []):
                if isinstance(lat, dict):
                    min_ns, max_ns = lat.get("minNs", 0), lat.get("maxNs", 0)
                    if min_ns and min_ns > 0:
                        est_ns = 0.5 * (min_ns + max_ns) if (max_ns and min_ns <= max_ns <= 2_000_000_000 and (max_ns - min_ns) <= 200_000_000) else float(min_ns)
                        latency_ms = max(latency_ms, est_ns / 1_000_000.0)
            if latency_ms <= 0.0:
                for proc_lat in params.get("ProcessLatency", []):
                    if isinstance(proc_lat, dict) and proc_lat.get("ns", 0) > 0:
                        latency_ms = max(latency_ms, proc_lat["ns"] / 1_000_000.0)

            if latency_ms <= 0.0:
                if bus_type == "bluetooth":
                    bt_codec = props.get("api.bluez5.codec", "").lower()
                    latency_ms = 65.0 if bt_codec in ("aptx_ll", "faststream", "lc3") else (130.0 if bt_codec in ("aptx", "aptx_hd") else (170.0 if bt_codec == "ldac" else 180.0))
                else:
                    latency_ms = {"network": 250.0, "hdmi": 10.0, "pci": 10.0, "spdif": 10.0, "usb": 12.0, "jack": 6.0, "internal": 5.0}.get(bus_type, 10.0)

            sinks.append(AudioSink(
                id=node_id, name=name, description=description, media_class="Audio/Sink",
                is_internal=is_internal, latency_ms=round(latency_ms, 1), bus_type=bus_type,
            ))

        # Disambiguate duplicate descriptions
        counts = {}
        for s in sinks:
            counts[s.description] = counts.get(s.description, 0) + 1
        if any(c > 1 for c in counts.values()):
            seen = {}
            for s in sinks:
                if counts[s.description] > 1:
                    idx = seen.get(s.description, 1)
                    seen[s.description] = idx + 1
                    s.description = f"{s.description} ({idx})"

        cls._cached_sinks = sinks
        cls._last_scan_time = now
        return sinks

    @classmethod
    def get_sinks(cls, force: bool = False) -> List[AudioSink]:
        return cls.scan_sinks(force=force)

    @classmethod
    def get_available_sinks(cls, force: bool = False) -> List[AudioSink]:
        return cls.scan_sinks(force=force)

    @classmethod
    def get_sink_states(cls) -> dict:
        """Returns a mapping of sink ID and sink name to playback state ('RUNNING', 'SUSPENDED', 'IDLE')."""
        states = {}
        try:
            res = subprocess.run(["pactl", "list", "sinks", "short"], capture_output=True, text=True, timeout=0.8)
            for line in res.stdout.splitlines():
                parts = line.strip().split()
                if len(parts) >= 5:
                    state = parts[-1].upper()
                    states[parts[0]] = state
                    states[parts[1]] = state
        except Exception:
            pass
        return states

    @classmethod
    def get_active_sink_inputs(cls) -> List[dict]:
        """Parses active client application streams playing audio into PipeWire sinks."""
        inputs = []
        try:
            res = subprocess.run(["pactl", "list", "sink-inputs"], capture_output=True, text=True, timeout=0.8)
            for b in res.stdout.split("Sink Input #"):
                if not b.strip():
                    continue
                input_id = b.splitlines()[0].strip()
                b_lower = b.lower()

                def _prop(key: str) -> str:
                    m = re.search(rf'{key}\s+=\s+\"([^\"]+)\"', b)
                    return m.group(1) if m else ""

                app_name = _prop("application\\.name") or _prop("application\\.process\\.binary") or _prop("node\\.name") or "Audio Stream"
                media_name = _prop("media\\.name")
                node_name = _prop("node\\.name")

                if (
                    app_name == "pw-loopback"
                    or node_name.startswith("polifonia")
                    or media_name.startswith("polifonia")
                    or "stream.capture.sink" in b_lower
                    or ("loopback" in b_lower and "output" in b_lower)
                ):
                    continue

                sink_m = re.search(r"Sink:\s+(\d+)", b)
                corked_m = re.search(r"Corked:\s+(\w+)", b)
                mute_m = re.search(r"Mute:\s+(\w+)", b)
                vol_m = re.search(r"/ (\d+)% /", b)

                is_corked = (corked_m.group(1).lower() == "yes") if corked_m else False
                is_muted = (mute_m.group(1).lower() == "yes") if mute_m else False

                inputs.append({
                    "id": input_id,
                    "sink_id": int(sink_m.group(1)) if sink_m else None,
                    "app_name": app_name,
                    "icon_name": _prop("application\\.icon_name") or "audio-x-generic",
                    "media_name": media_name,
                    "volume_pct": int(vol_m.group(1)) if vol_m else 100,
                    "is_active": (not is_corked) and (not is_muted),
                    "corked": is_corked,
                    "muted": is_muted,
                })
        except Exception:
            pass
        return inputs


PipeWireScanner = DeviceScanner
