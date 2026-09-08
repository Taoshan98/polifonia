"""Unit tests for PipeWire graph scanner and sink hardware detection."""

import unittest
import json
from unittest.mock import patch, MagicMock
from core.models import AudioSink
from backend.pipewire_scanner import DeviceScanner, PipeWireScanner


class TestScanner(unittest.TestCase):
    """Test suite for PipeWire scanner and hardware sink classifier."""

    def test_get_default_sink_id_parsing(self):
        """Verify parsing default sink node ID from wpctl status output."""
        mock_output = """
PipeWire 'pipewire-0' [0.3.65, user@host, cookie:12345]
 └─ Sinks:
      40. Built-in Audio Analog Stereo        [vol: 0.60]
    * 55. LG UltraWide HDMI Audio             [vol: 1.00]
      60. USB Audio Device                    [vol: 0.80]
 └─ Sources:
    * 70. Built-in Audio Analog Stereo        [vol: 1.00]
"""
        with patch("subprocess.run") as mock_run:
            mock_run.return_value = MagicMock(stdout=mock_output, returncode=0)
            default_id = DeviceScanner.get_default_sink_id()
            self.assertEqual(default_id, 55)

    def test_get_default_sink_id_failure(self):
        """Verify get_default_sink_id returns None when command fails or output is empty."""
        with patch("subprocess.run", side_effect=Exception("Command not found")):
            default_id = DeviceScanner.get_default_sink_id()
            self.assertIsNone(default_id)

    def test_scan_sinks_filters_and_classifies_nodes(self):
        """Verify pw-dump parsing, filtering non-sinks and classifying internal vs external devices."""
        mock_pw_dump = [
            # 1. Valid External HDMI Sink
            {
                "id": 44,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "props": {
                        "media.class": "Audio/Sink",
                        "node.name": "alsa_output.pci-gpu.pro-output-3",
                        "node.description": "GPU High Definition Audio Controller Pro",
                        "alsa.card": 0,
                        "alsa.device": 3,
                        "device.bus": "pci"
                    }
                }
            },
            # 2. Valid External USB Sink
            {
                "id": 88,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "props": {
                        "media.class": "Audio/Sink",
                        "node.name": "alsa_output.usb-DAC_123.analog-stereo",
                        "node.nick": "USB Subwoofer DAC",
                        "device.bus": "usb"
                    }
                }
            },
            # 3. Laptop Internal Speaker
            {
                "id": 99,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "props": {
                        "media.class": "Audio/Sink",
                        "node.name": "alsa_output.pci-0000_00_1f.3.HiFi__Speaker__sink",
                        "node.description": "Speaker",
                        "device.bus": "pci",
                        "device.form-factor": "internal"
                    }
                }
            },
            # 4. Polifonia's own virtual node (Must be excluded to prevent loops)
            {
                "id": 105,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "props": {
                        "media.class": "Audio/Sink",
                        "node.name": "polifonia_sub_44",
                        "node.description": "Polifonia Virtual Loopback"
                    }
                }
            },
            # 5. Non-sink node (Audio/Source / Microphone)
            {
                "id": 110,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "props": {
                        "media.class": "Audio/Source",
                        "node.name": "alsa_input.pci-mic",
                        "node.description": "Internal Microphone"
                    }
                }
            },
            # 6. Non-node object (Link/Client)
            {
                "id": 120,
                "type": "PipeWire:Interface:Link",
                "info": {}
            }
        ]

        mock_eld = {0: [{"name": "LG UltraGear 4K", "connection": "HDMI", "pin": 0}]}

        with patch("subprocess.check_output") as mock_pw, \
             patch.object(DeviceScanner, "get_eld_monitors", return_value=mock_eld), \
             patch("subprocess.run") as mock_run:
            mock_pw.return_value = json.dumps(mock_pw_dump)
            sinks = DeviceScanner.scan_sinks()

            self.assertEqual(len(sinks), 3)

            sink_map = {s.id: s for s in sinks}

            # Check HDMI Sink
            self.assertIn(44, sink_map)
            self.assertEqual(sink_map[44].name, "alsa_output.pci-gpu.pro-output-3")
            self.assertEqual(sink_map[44].description, "Monitor LG UltraGear 4K (HDMI)")
            self.assertFalse(sink_map[44].is_internal)

            # Check USB Sink
            self.assertIn(88, sink_map)
            self.assertEqual(sink_map[88].description, "USB Audio (USB Subwoofer DAC)")
            self.assertFalse(sink_map[88].is_internal)

            # Check Internal Laptop Speaker
            self.assertIn(99, sink_map)
            self.assertEqual(sink_map[99].description, "Integrated Speakers (Internal)")
            self.assertTrue(sink_map[99].is_internal)

            # Ensure virtual node 105 and source node 110 are not present
            self.assertNotIn(105, sink_map)
            self.assertNotIn(110, sink_map)

    def test_scan_sinks_error_handling(self):
        """Verify scan_sinks returns empty list when pw-dump fails."""
        with patch("subprocess.check_output", side_effect=Exception("pw-dump failure")):
            sinks = DeviceScanner.scan_sinks()
            self.assertEqual(sinks, [])

    def test_scan_bluetooth_device_with_custom_name_and_latency(self):
        """Verify Bluetooth device name is correctly resolved from node.description and latency is extracted."""
        mock_pw_dump = [
            {
                "id": 139,
                "type": "PipeWire:Interface:Node",
                "info": {
                    "props": {
                        "media.class": "Audio/Sink",
                        "node.name": "bluez_output.4C_87_5D_9F_7E_60.1",
                        "node.description": "Black Diamond",
                        "device.bus": "bluetooth",
                        "api.bluez5.codec": "aac"
                    },
                    "params": {
                        "Latency": [
                            {
                                "direction": "Input",
                                "minNs": 188999999,
                                "maxNs": 188999999
                            }
                        ]
                    }
                }
            }
        ]
        with patch("subprocess.check_output", return_value=json.dumps(mock_pw_dump)), \
             patch.object(DeviceScanner, "get_eld_monitors", return_value={}), \
             patch("subprocess.run"):
            sinks = DeviceScanner.scan_sinks()
            self.assertEqual(len(sinks), 1)
            bt_sink = sinks[0]
            self.assertEqual(bt_sink.id, 139)
            self.assertEqual(bt_sink.name, "bluez_output.4C_87_5D_9F_7E_60.1")
            self.assertEqual(bt_sink.description, "Bluetooth Audio (Black Diamond)")
            self.assertEqual(bt_sink.bus_type, "bluetooth")
            self.assertEqual(bt_sink.latency_ms, 189.0)

    def test_scanner_aliases(self):
        """Verify alias class and helper methods."""
        self.assertIs(PipeWireScanner, DeviceScanner)
        with patch.object(DeviceScanner, "scan_sinks", return_value=[]) as mock_scan:
            DeviceScanner.get_sinks()
            DeviceScanner.get_available_sinks()
            self.assertEqual(mock_scan.call_count, 2)


if __name__ == "__main__":
    unittest.main()

