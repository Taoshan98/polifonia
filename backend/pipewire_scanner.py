"""Inspects system hardware and PipeWire topology to detect audio sinks and roles."""

import json
import subprocess
import re
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
                if ("*" in line and "Audio/Sink" in line) or ("*" in line and "[" in line and "vol:" in line):
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

        import glob
        eld_map = {}  # card_id -> list of dict(name, conn_type, pin)
        for eld_path in glob.glob("/proc/asound/card*/eld*"):
            try:
                with open(eld_path, "r") as f:
                    content = f.read()
                if "monitor_present\t1" in content or "monitor_present\t\t1" in content or "monitor_present 1" in content:
                    m_name = re.search(r"monitor_name\s+(.+)", content)
                    conn = re.search(r"connection_type\s+(.+)", content)
                    name = m_name.group(1).strip() if m_name else "Display Audio"
                    conn_type = conn.group(1).strip() if conn else "HDMI"

                    c_match = re.search(r"card(\d+)", eld_path)
                    p_match = re.search(r"eld#\d+\.(\d+)", eld_path)
                    if c_match:
                        c_id = int(c_match.group(1))
                        pin_id = int(p_match.group(1)) if p_match else 0
                        eld_map.setdefault(c_id, []).append({
                            "name": name,
                            "connection": conn_type,
                            "pin": pin_id
                        })
            except Exception:
                pass
        cls._cached_eld_map = eld_map
        return eld_map

    @classmethod
    def ensure_multimonitor_card_profiles(cls):
        """Ensures dedicated multi-monitor GPU sound cards (e.g. Nvidia/AMD) are set to pro-audio profile so all displays are exposed as distinct sinks."""
        now = time.time()
        if cls._last_profile_check and (now - cls._last_profile_check < 5.0):
            return
        cls._last_profile_check = now

        try:
            eld_map = cls.get_eld_monitors()
            has_multi_eld = any(len(mons) > 1 for mons in eld_map.values())
            res = subprocess.run(["pactl", "list", "cards", "short"], capture_output=True, text=True, timeout=1.0)
            for line in res.stdout.splitlines():
                parts = line.split()
                if len(parts) >= 2:
                    card_name = parts[1]
                    # Only target dedicated GPU cards (Nvidia, AMD) or cards with multiple ELD displays
                    # Strictly avoid Intel/Realtek internal audio controllers (sof-hda-dsp, alc, etc.)
                    is_gpu_card = (
                        ("01_00.1" in card_name) or
                        ("nvidia" in card_name.lower()) or
                        ("radeon" in card_name.lower())
                    )
                    if is_gpu_card or has_multi_eld:
                        if "sof" in card_name.lower() or "realtek" in card_name.lower() or "00_1f.3" in card_name:
                            continue
                        try:
                            card_info = subprocess.check_output(["pactl", "list", "cards"], text=True, timeout=1.0)
                            card_blocks = card_info.split("Card #")
                            for block in card_blocks:
                                if card_name in block:
                                    if "Active Profile: pro-audio" not in block:
                                        subprocess.run(["pactl", "set-card-profile", card_name, "pro-audio"],
                                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=1.0)
                                        time.sleep(0.08)
                                        cls._cached_sinks = []
                                    break
                        except Exception:
                            pass
        except Exception:
            pass

    @classmethod
    def scan_sinks(cls, force: bool = False) -> List[AudioSink]:
        """Scans PipeWire graph for active audio sinks with generic, dynamic hardware classification."""
        import sys
        is_test = "unittest" in sys.modules or bool(subprocess.os.environ.get("PYTEST_CURRENT_TEST"))
        now = time.time()
        if not is_test and not force and cls._cached_sinks and (now - cls._last_scan_time < 2.5):
            return list(cls._cached_sinks)

        if not is_test:
            cls.ensure_multimonitor_card_profiles()
        eld_map = cls.get_eld_monitors(force=force)
        sinks = []
        try:
            output = subprocess.check_output(["pw-dump"], text=True, timeout=2.0)
            objects = json.loads(output)
            if not isinstance(objects, list):
                return sinks
        except Exception:
            return sinks

        card_allocated_monitors = {}

        for obj in objects:
            if obj.get("type") != "PipeWire:Interface:Node":
                continue

            props = obj.get("info", {}).get("props", {})
            media_class = props.get("media.class", "")

            if media_class != "Audio/Sink":
                continue

            name = props.get("node.name", "")
            # Exclude our own virtual sink nodes from discovery to avoid loops
            if "polifonia" in name.lower() or "filter-chain" in name.lower():
                continue

            node_id = obj.get("id")
            card_id = props.get("alsa.card")
            raw_desc = (
                props.get("node.nick") or 
                props.get("node.description") or 
                props.get("device.description") or 
                name
            )
            bus = props.get("device.bus", "").lower()
            form_factor = props.get("device.form-factor", "").lower()
            raw_desc_lower = raw_desc.lower()
            name_lower = name.lower()

            # Dynamic Classification across all hardware and network transport types
            is_internal = False
            api_lower = props.get("device.api", "").lower()
            is_bluetooth = (
                bus == "bluetooth" or 
                "bluez" in name_lower or 
                "bluez" in api_lower or
                "bluetooth" in props.get("device.icon_name", "").lower()
            )
            is_network = (
                bus == "network" or
                any(k in name_lower or k in api_lower for k in ("raop", "airplay", "rtp", "roc", "snapcast", "cast", "tunnel"))
            )
            is_usb = (bus == "usb" or "usb" in name_lower)
            is_digital_hdmi = (
                not is_usb and (
                    "hdmi" in name_lower or
                    "displayport" in name_lower or
                    "pro-output" in name_lower
                )
            )
            is_spdif = (
                not is_digital_hdmi and (
                    "iec958" in name_lower or
                    "spdif" in name_lower or
                    "optical" in raw_desc_lower or
                    "digital" in raw_desc_lower
                )
            )
            bus_type = "other"

            if is_bluetooth:
                clean_name = (
                    props.get("device.alias") or 
                    props.get("device.description") or 
                    props.get("node.description") or 
                    props.get("media.name") or 
                    props.get("node.nick") or 
                    props.get("device.product.name") or 
                    "Bluetooth Device"
                ).strip()
                for prefix in ["Bluetooth Audio (", "Bluetooth ("]:
                    if clean_name.startswith(prefix) and clean_name.endswith(")"):
                        clean_name = clean_name[len(prefix):-1].strip()
                description = clean_name
                bus_type = "bluetooth"
                is_internal = False
            elif is_network:
                clean_name = (
                    props.get("node.description") or
                    props.get("device.description") or
                    props.get("media.name") or
                    raw_desc
                ).strip()
                description = f"Network Audio ({clean_name})" if "network" not in clean_name.lower() else clean_name
                bus_type = "network"
                is_internal = False
            elif is_digital_hdmi:
                bus_type = "hdmi"
                c_num = int(card_id) if card_id is not None else None
                if c_num is not None and c_num in eld_map:
                    monitors = eld_map[c_num]
                    curr_idx = card_allocated_monitors.get(c_num, 0)
                    if curr_idx < len(monitors):
                        mon = monitors[curr_idx]
                        card_allocated_monitors[c_num] = curr_idx + 1
                        description = f"Monitor {mon['name']} ({mon['connection']})"
                    else:
                        # Unconnected phantom HDMI/DP port on this graphics card
                        continue
                else:
                    # Unconnected HDMI/DP port on card without active ELD
                    continue
            elif is_usb:
                clean_name = props.get("node.nick") or props.get("device.product.name") or raw_desc.replace("Analog Stereo", "").replace("Stereo analogico", "").strip()
                description = f"USB Audio ({clean_name})"
                bus_type = "usb"
            elif is_spdif:
                clean_name = props.get("node.nick") or props.get("device.product.name") or raw_desc.replace("Digital Stereo (IEC958)", "").strip() or "S/PDIF"
                description = f"Digital S/PDIF ({clean_name})"
                bus_type = "spdif"
            elif "headphone" in raw_desc_lower or "headphone" in name_lower:
                clean_name = props.get("node.nick") or props.get("device.product.name") or "Analog"
                description = f"Headphones / Audio Jack ({clean_name})"
                bus_type = "jack"
            elif "speaker" in raw_desc_lower or "speaker" in name_lower or form_factor == "internal":
                description = "Integrated Speakers (Internal)"
                is_internal = True
                bus_type = "internal"
            else:
                description = raw_desc
                bus_type = bus or "pci"

            # Calculate hardware latency in ms from Latency and ProcessLatency SPA params
            latency_ms = 0.0
            params = obj.get("info", {}).get("params", {})
            for lat in params.get("Latency", []):
                if isinstance(lat, dict):
                    min_ns = lat.get("minNs", 0)
                    max_ns = lat.get("maxNs", 0)
                    if min_ns and min_ns > 0:
                        if max_ns and min_ns <= max_ns <= 2_000_000_000 and (max_ns - min_ns) <= 200_000_000:
                            est_ns = 0.5 * (min_ns + max_ns)
                        else:
                            est_ns = float(min_ns)
                        latency_ms = max(latency_ms, est_ns / 1_000_000.0)

            if latency_ms <= 0.0:
                for proc_lat in params.get("ProcessLatency", []):
                    if isinstance(proc_lat, dict):
                        ns_val = proc_lat.get("ns", 0)
                        if ns_val and ns_val > 0:
                            latency_ms = max(latency_ms, ns_val / 1_000_000.0)

            # Assign sensible codec/bus-aware default buffer latency if idle/unreported
            if bus_type == "bluetooth" and latency_ms <= 0.0:
                bt_codec = props.get("api.bluez5.codec", "").lower()
                if bt_codec in ("aptx_ll", "faststream", "lc3"):
                    latency_ms = 65.0
                elif bt_codec in ("aptx", "aptx_hd"):
                    latency_ms = 130.0
                elif bt_codec == "ldac":
                    latency_ms = 170.0
                else:
                    latency_ms = 180.0
            elif bus_type == "network" and latency_ms <= 0.0:
                latency_ms = 250.0
            elif bus_type in ("hdmi", "pci", "spdif") and latency_ms <= 0.0:
                latency_ms = 10.0
            elif bus_type == "usb" and latency_ms <= 0.0:
                latency_ms = 12.0
            elif bus_type == "jack" and latency_ms <= 0.0:
                latency_ms = 6.0
            elif bus_type == "internal" and latency_ms <= 0.0:
                latency_ms = 5.0

            sink = AudioSink(
                id=node_id,
                name=name,
                description=description,
                media_class=media_class,
                is_internal=is_internal,
                latency_ms=round(latency_ms, 1),
                bus_type=bus_type
            )
            sinks.append(sink)

        # Disambiguate duplicate monitor names (e.g. multiple identical models)
        name_counts = {}
        for s in sinks:
            name_counts[s.description] = name_counts.get(s.description, 0) + 1

        if any(cnt > 1 for cnt in name_counts.values()):
            seen_indices = {}
            for s in sinks:
                if name_counts[s.description] > 1:
                    idx = seen_indices.get(s.description, 1)
                    seen_indices[s.description] = idx + 1
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
        """Returns a mapping of sink ID and sink name to playback state (e.g. 'RUNNING', 'SUSPENDED', 'IDLE')."""
        states = {}
        try:
            res = subprocess.run(["pactl", "list", "sinks", "short"], capture_output=True, text=True, timeout=0.8)
            for line in res.stdout.splitlines():
                parts = line.strip().split()
                if len(parts) >= 5:
                    s_id = parts[0]
                    s_name = parts[1]
                    state = parts[-1].upper()
                    states[s_id] = state
                    states[s_name] = state
        except Exception:
            pass
        return states

    @classmethod
    def get_active_sink_inputs(cls) -> List[dict]:
        """Parses active client application streams playing audio into PipeWire sinks."""
        inputs = []
        try:
            res = subprocess.run(["pactl", "list", "sink-inputs"], capture_output=True, text=True, timeout=0.8)
            blocks = res.stdout.split("Sink Input #")
            for b in blocks:
                if not b.strip():
                    continue
                lines = b.splitlines()
                input_id = lines[0].strip()

                app_name_match = re.search(r'application\.name\s+=\s+\"([^\"]+)\"', b)
                icon_match = re.search(r'application\.icon_name\s+=\s+\"([^\"]+)\"', b)
                binary_match = re.search(r'application\.process\.binary\s+=\s+\"([^\"]+)\"', b)
                media_name_match = re.search(r'media\.name\s+=\s+\"([^\"]+)\"', b)
                node_name_match = re.search(r'node\.name\s+=\s+\"([^\"]+)\"', b)

                app_name = (
                    app_name_match.group(1) if app_name_match else (
                        binary_match.group(1) if binary_match else (
                            node_name_match.group(1) if node_name_match else "Audio Stream"
                        )
                    )
                )
                media_name = media_name_match.group(1) if media_name_match else ""
                node_name = node_name_match.group(1) if node_name_match else ""

                b_lower = b.lower()
                is_loopback = (
                    (app_name == "pw-loopback") or
                    node_name.startswith("polifonia") or
                    media_name.startswith("polifonia") or
                    ("stream.capture.sink" in b_lower) or
                    ("loopback" in b_lower and "output" in b_lower)
                )
                if is_loopback:
                    continue

                sink_match = re.search(r"Sink:\s+(\d+)", b)
                sink_id = int(sink_match.group(1)) if sink_match else None

                corked_match = re.search(r"Corked:\s+(\w+)", b)
                is_corked = (corked_match.group(1).lower() == "yes") if corked_match else False

                mute_match = re.search(r"Mute:\s+(\w+)", b)
                is_muted = (mute_match.group(1).lower() == "yes") if mute_match else False

                icon_name = icon_match.group(1) if icon_match else "audio-x-generic"
                media_name_str = media_name if media_name else "Playback"

                vol_match = re.search(r"/ (\d+)% /", b)
                vol_pct = int(vol_match.group(1)) if vol_match else 100

                inputs.append({
                    "id": input_id,
                    "sink_id": sink_id,
                    "app_name": app_name,
                    "icon_name": icon_name,
                    "media_name": media_name,
                    "volume_pct": vol_pct,
                    "is_active": (not is_corked) and (not is_muted),
                    "corked": is_corked,
                    "muted": is_muted
                })
        except Exception:
            pass
        return inputs


# Alias
PipeWireScanner = DeviceScanner

