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
    def ensure_multimonitor_card_profiles():
        """Ensures HDMI/DP multi-head graphics cards are set to pro-audio profile to expose all monitors."""
        try:
            res = subprocess.run(["pactl", "list", "cards", "short"], capture_output=True, text=True)
            for line in res.stdout.splitlines():
                if "01_00.1" in line or "NVidia" in line:
                    card_name = line.split()[1]
                    subprocess.run(["pactl", "set-card-profile", card_name, "pro-audio"], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except Exception:
            pass

    @classmethod
    def scan_sinks(cls) -> List[AudioSink]:
        """Scans PipeWire graph for active audio sinks excluding Polifonia virtual nodes."""
        cls.ensure_multimonitor_card_profiles()
        sinks = []
        try:
            output = subprocess.check_output(["pw-dump"], text=True)
            objects = json.loads(output)
        except Exception:
            return sinks

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

            # Ignore disconnected Pro-Audio outputs or phantom endpoints
            if "pro-output-9" in name:
                continue
            if "HiFi__HDMI" in name:
                # Intel phantom ports without connected monitor
                continue

            node_id = obj.get("id")
            raw_desc = (
                props.get("node.nick") or 
                props.get("node.description") or 
                props.get("device.description") or 
                name
            )

            desc_lower = raw_desc.lower()
            name_lower = name.lower()

            # Accurate hardware monitor model assignment from ELD/EDID
            if "pro-output-3" in name_lower or "01_00.1.hdmi" in name_lower:
                description = "Monitor Samsung Odyssey G61SD (HDMI / Cassa AUX)"
            elif "pro-output-7" in name_lower:
                description = "Monitor BenQ EW2480 (DisplayPort 1)"
            elif "pro-output-8" in name_lower:
                description = "Monitor BenQ EW2480 (DisplayPort 2)"
            elif "smi" in desc_lower or "silicon_motion" in name_lower:
                description = "Monitor USB Display (Adattatore SMI)"
            elif "speaker" in desc_lower or "hifi__speaker" in name_lower:
                description = "Altoparlanti Integrati (Laptop Speaker)"
            else:
                description = raw_desc

            # Detect if it's the laptop's internal speakers
            is_internal = False
            form_factor = props.get("device.form-factor", "").lower()
            bus = props.get("device.bus", "").lower()

            if "speaker" in desc_lower or "internal" in desc_lower or "built-in" in desc_lower or "laptop" in desc_lower:
                if "pci" in bus or "alc" in name_lower or "hda" in name_lower or "alder" in desc_lower or "speaker" in name_lower:
                    is_internal = True
            if form_factor == "internal":
                is_internal = True

            sink = AudioSink(
                id=node_id,
                name=name,
                description=description,
                media_class=media_class,
                is_internal=is_internal
            )
            sinks.append(sink)

        return sinks

    @classmethod
    def get_sinks(cls) -> List[AudioSink]:
        return cls.scan_sinks()

    @classmethod
    def get_available_sinks(cls) -> List[AudioSink]:
        return cls.scan_sinks()


# Alias
PipeWireScanner = DeviceScanner

