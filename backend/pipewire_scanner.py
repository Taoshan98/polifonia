"""Inspects system hardware and PipeWire topology to detect audio sinks and roles."""

import json
import subprocess
import re
from typing import List, Optional
from core.models import AudioSink


class DeviceScanner:
    @staticmethod
    def get_default_sink_id() -> Optional[int]:
        try:
            res = subprocess.run(["wpctl", "status"], capture_output=True, text=True, check=True)
            for line in res.stdout.splitlines():
                if "*" in line and "Audio/Sink" in line or ("*" in line and "[" in line and "vol:" in line):
                    match = re.search(r"\*\s+(\d+)\.", line)
                    if match:
                        return int(match.group(1))
        except Exception:
            pass
        return None

    @staticmethod
    def get_eld_monitors() -> dict:
        """Dynamically parses kernel ALSA ELD files for all sound cards and connected video displays."""
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
        return eld_map

    @staticmethod
    def ensure_multimonitor_card_profiles():
        """
        Dynamically detects any multi-output graphics card (NVIDIA, AMD Radeon, Intel Arc)
        and sets its profile to pro-audio so all physical monitors are exposed as independent audio sinks.
        """
        try:
            res = subprocess.run(["pactl", "list", "cards"], capture_output=True, text=True)
            cards = res.stdout.split("Card #")
            for card_block in cards:
                if not card_block.strip():
                    continue
                c_name_match = re.search(r"Name:\s+(\S+)", card_block)
                if not c_name_match:
                    continue
                card_name = c_name_match.group(1)
                # If card has multi-HDMI/DP capabilities and supports pro-audio profile
                if "pro-audio:" in card_block and "hdmi-output-" in card_block:
                    if "Active Profile: pro-audio" not in card_block:
                        subprocess.run(["pactl", "set-card-profile", card_name, "pro-audio"],
                                       stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except Exception:
            pass

    @classmethod
    def scan_sinks(cls) -> List[AudioSink]:
        """Scans PipeWire graph for active audio sinks with generic, dynamic hardware classification."""
        cls.ensure_multimonitor_card_profiles()
        eld_map = cls.get_eld_monitors()
        sinks = []
        try:
            output = subprocess.check_output(["pw-dump"], text=True)
            objects = json.loads(output)
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

            # Dynamic Classification
            is_internal = False
            is_digital_hdmi = "hdmi" in name_lower or "pro-output" in name_lower or "iec958" in name_lower

            if is_digital_hdmi:
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
            elif "speaker" in raw_desc_lower or "speaker" in name_lower or form_factor == "internal":
                description = "Integrated Speakers (Internal)"
                is_internal = True
            elif "headphone" in raw_desc_lower or "headphone" in name_lower:
                clean_name = props.get("node.nick") or props.get("device.product.name") or "Analog"
                description = f"Headphones / Audio Jack ({clean_name})"
            elif bus == "usb" or "usb" in name_lower:
                clean_name = props.get("node.nick") or props.get("device.product.name") or raw_desc.replace("Analog Stereo", "").replace("Stereo analogico", "").strip()
                description = f"USB Audio ({clean_name})"
            elif bus == "bluetooth" or "bluez" in name_lower:
                clean_name = props.get("node.nick") or props.get("device.product.name") or "Bluetooth Device"
                description = f"Bluetooth Audio ({clean_name})"
            else:
                description = raw_desc

            sink = AudioSink(
                id=node_id,
                name=name,
                description=description,
                media_class=media_class,
                is_internal=is_internal
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

        return sinks

    @classmethod
    def get_sinks(cls) -> List[AudioSink]:
        return cls.scan_sinks()

    @classmethod
    def get_available_sinks(cls) -> List[AudioSink]:
        return cls.scan_sinks()


# Alias
PipeWireScanner = DeviceScanner

