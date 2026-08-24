# Polifonia Audio Studio 🎵

**Polifonia Audio Studio** is an advanced, PipeWire-native multi-sink audio compositing and acoustic DSP management system designed for Linux desktop environments (GNOME / Wayland / X11).

It enables users with heterogeneous multi-monitor setups (HDMI/DisplayPort integrated speakers, USB audio DACs, auxiliary subwoofers, and analog outputs) to aggregate disparate physical outputs into a unified, phase-aligned, frequency-filtered **2.1 / Multichannel Virtual Soundstage**.

---

## 📑 Table of Contents

- [Overview & Problem Statement](#-overview--problem-statement)
- [Architecture & System Design](#-architecture--system-design)
  - [Core Domain Layer (`core/`)](#core-domain-layer-core)
  - [PipeWire Backend Engine (`backend/`)](#pipewire-backend-engine-backend)
  - [Audio Coordination Service (`services/`)](#audio-coordination-service-services)
  - [Profile & State Persistence (`storage/`)](#profile--state-persistence-storage)
  - [GTK4 / Libadwaita User Interface (`ui/`)](#gtk4--libadwaita-user-interface-ui)
- [Technical DSP & Acoustic Capabilities](#-technical-dsp--acoustic-capabilities)
  - [Dynamic Multi-Sink Routing](#1-dynamic-multi-sink-routing)
  - [Active 2.1 Crossover Filtering](#2-active-21-crossover-filtering)
  - [Time Alignment & Delay Compensation](#3-time-alignment--delay-compensation)
  - [Per-Channel Gain & Polarity Calibration](#4-per-channel-gain--polarity-calibration)
  - [Embedded Synthetic Acoustic Test Generator](#5-embedded-synthetic-acoustic-test-generator)
- [Prerequisites & Dependencies](#-prerequisites--dependencies)
- [Installation & Quick Start](#-installation--quick-start)
- [Running Automated Tests](#-running-automated-tests)
- [Configuration & Storage Specification](#-configuration--storage-specification)
- [License](#-license)

---

## 🔬 Overview & Problem Statement

Modern multi-monitor workstations frequently feature integrated speakers across multiple displays (e.g. left and right HDMI/DisplayPort screens) alongside a dedicated auxiliary or USB subwoofer. Standard Linux audio servers expose these as isolated, independent hardware sinks.

Operating them simultaneously in unison presents significant acoustic challenges:
1. **Frequency Collisions**: Small monitor speakers distort when attempting to reproduce low-frequency bass notes, while subwoofers muddy vocals when reproducing high-frequency content.
2. **Phase Cancellation & Buffer Latency**: HDMI display controllers introduce distinct frame-buffer latencies relative to direct analog or USB outputs, causing comb filtering and phase cancellation.
3. **Imbalanced Volumes**: Divergent DAC sensitivity across monitors results in skewed stereo imaging.

Polifonia solves this by providing a unified PipeWire routing layer with integrated active DSP filtering (Butterworth 2nd-order Crossover), sub-millisecond delay compensation, and single-sink unified desktop control.

---

## 🏛 Architecture & System Design

The application follows a clean layered architecture with separation between domain models, low-level PipeWire interfaces, state storage, and the GTK4 presentation layer:

```
polifonia/
├── backend/
│   ├── pipewire_config.py     # SPA filter-chain & loopback graph generator
│   └── pipewire_scanner.py    # Topology inspection via pw-dump & wpctl
├── core/
│   └── models.py              # Strongly-typed domain data models & enums
├── services/
│   ├── audio_engine.py        # Process supervisor, loopback lifecycle & test tone
│   └── audio_service.py       # Service provider interface alias
├── storage/
│   ├── preset_manager.py      # Profile management proxy
│   └── settings_store.py      # XDG-compliant JSON persistence
├── ui/
│   └── views/
│       ├── main_window.py     # Main Libadwaita application window & control bar
│       └── speaker_row.py     # Per-speaker expander row widget with DSP sliders
├── tests/
│   └── test_services.py       # Unit test suite for models, configs & storage
├── main.py                    # Application entrypoint (Adw.Application)
└── README.md
```

### Core Domain Layer (`core/`)
Defines strongly-typed dataclasses for all audio entities:
- `AudioSink`: Represents physical or virtual PipeWire nodes discovered from the audio server graph.
- `SpeakerConfig`: Represents acoustic DSP settings for an assigned speaker sink (role, gain multiplier, millisecond delay, polarity, mute).
- `CrossoverConfig`: Configuration for frequency cutoff and filtering state.
- `SystemConfig`: Full profile state with bidirectionally bound crossover parameters, master gain, channel maps, and auto-start preferences.
- `SpeakerRole`: Enumeration covering `LEFT`, `RIGHT`, `SUBWOOFER`, `CENTER`, `STEREO`, `SURROUND_LEFT`, `SURROUND_RIGHT`, and `EXCLUDED`.

### PipeWire Backend Engine (`backend/`)
- **`DeviceScanner` (`pipewire_scanner.py`)**: Interrogates the PipeWire graph via `pw-dump`, filtering nodes for `media.class = Audio/Sink`. Automatically distinguishes between external digital monitors, USB DACs, and built-in laptop speakers using hardware bus attributes (`device.bus`, `device.form-factor`, ALC/HDA vendor tags).
- **`PipeWireConfigGenerator` (`pipewire_config.py`)**: Synthesizes PipeWire SPA (Simple Plugin API) filter-chain configuration blocks using `libspa-filter-graph` and `libspa-audioconvert`.

### Audio Coordination Service (`services/`)
- **`AudioEngineService` (`audio_engine.py`)**: Manages the runtime execution of virtual audio nodes (`pw-loopback` instances) and synchronizes hardware state. Includes a background multi-threaded synthetic WAV tone synthesizer.

### Profile & State Persistence (`storage/`)
- **`StorageService` (`settings_store.py`)**: Manages XDG-compliant storage (`~/.config/polifonia/`), persisting active settings (`current.json`) and named setup profiles (`profiles/*.json`).

### GTK4 / Libadwaita User Interface (`ui/`)
Built with modern GNOME design patterns:
- **`MainWindow` (`ui/views/main_window.py`)**: Integrates `Adw.HeaderBar`, `Adw.PreferencesPage`, status banners, toast notifications (`Adw.ToastOverlay`), and responsive layout adaptation.
- **`SpeakerRow` (`ui/views/speaker_row.py`)**: Custom `Adw.ExpanderRow` offering combo-box role selection, millisecond-precision scale controls for latency, gain calibration, and per-speaker acoustic audit buttons.

---

## 🎛 Technical DSP & Acoustic Capabilities

### 1. Dynamic Multi-Sink Routing
Polifonia deploys dedicated PipeWire loopback nodes linked to designated physical hardware sinks. Each loopback node configures channel positioning and buffer latency:
```bash
pw-loopback --capture-props="node.name=polifonia_sub_<ID> media.class=Audio/Sink audio.position=[ FL FR ]" --target-object=<ID> --latency=<MS>/1000
```

### 2. Active 2.1 Crossover Filtering
When Crossover is active:
- **High-Pass Filter (`bq_highpass`)**: Applied to Satellite speakers (`LEFT`, `RIGHT`). Attenuates frequencies below the cutoff threshold (40 Hz – 250 Hz, Butterworth $Q = 0.707$), preventing cone over-excursion and distortion in small monitor speakers.
- **Low-Pass Filter (`bq_lowpass`)**: Applied to the Subwoofer (`SUBWOOFER`). Eliminates mid-to-high frequencies, directing all low-end acoustic energy exclusively to the bass driver.

### 3. Time Alignment & Delay Compensation
Sound travels approximately $34.3\text{ cm}$ per millisecond. HDMI video processing buffers can add between $10\text{ ms}$ and $80\text{ ms}$ of latency compared to auxiliary ports. Polifonia provides adjustable delay lines ($0.0\text{ ms}$ – $150.0\text{ ms}$ with $0.1\text{ ms}$ precision) to align arrival times at the listener's ear, restoring crisp transient response.

### 4. Per-Channel Gain & Polarity Calibration
- Independent gain multipliers ($0.0\times$ to $1.5\times$, representing $0\%$ to $150\%$) for volume matching.
- Phase inversion toggle ($0^\circ / 180^\circ$) to eliminate acoustic cancellation at the crossover frequency.

### 5. Embedded Synthetic Acoustic Test Generator
An in-memory 16-bit PCM stereo sine wave synthesizer generates test tones ($440\text{ Hz}$ standard, customizable duration and envelope) directly targeting specific PipeWire sink IDs via `pw-play`, eliminating external media player dependencies.

---

## 📦 Prerequisites & Dependencies

### System Packages (Ubuntu / Debian / Fedora / Arch)

Ensure PipeWire and PyGObject / GTK4 / Libadwaita are installed:

#### Ubuntu / Debian (22.04 LTS / 24.04 LTS):
```bash
sudo apt update
sudo apt install -y pipewire pipewire-pulse wireplumber libspa-0.2-modules \
                    python3 python3-gi python3-gi-cairo gir1.2-gtk-4.0 gir1.2-adw-1
```

#### Fedora (38 / 39 / 40):
```bash
sudo dnf install -y pipewire wireplumber python3-gobject gtk4 libadwaita
```

#### Arch Linux:
```bash
sudo pacman -S pipewire wireplumber python-gobject gtk4 libadwaita
```

---

## 🚀 Installation & Quick Start

1. Clone or navigate to the repository directory:
   ```bash
   cd /home/ntm/Develop/Taoshan/polifonia
   ```

2. Run the application:
   ```bash
   python3 main.py
   ```

3. In the Polifonia window:
   - Identify your detected audio sinks (e.g. Left HDMI monitor, Right HDMI monitor, USB Subwoofer).
   - Toggle the switch on the sinks you wish to include.
   - Assign roles (**Sinistra / Left**, **Destra / Right**, **Subwoofer**).
   - Adjust the **Crossover Frequency** (e.g. `90 Hz` or `120 Hz`).
   - Click **Attiva Unisono** to engage the virtual soundstage.

---

## 🧪 Running Automated Tests

The repository includes a comprehensive unit testing suite covering model serialization, lifecycle transformations, and PipeWire configuration generation:

```bash
python3 -m unittest discover tests
```

Output:
```text
...
----------------------------------------------------------------------
Ran 3 tests in 0.001s

OK
```

---

## 📄 Configuration & Storage Specification

Polifonia adheres to the XDG Base Directory specification:
- Active Configuration: `~/.config/polifonia/current.json`
- Saved Presets: `~/.config/polifonia/profiles/<profile_name>.json`

### Example Profile JSON
```json
{
  "profile_name": "2.1 Dual Monitor Studio",
  "master_volume": 1.0,
  "crossover_enabled": true,
  "crossover_freq": 110,
  "crossover": {
    "enabled": true,
    "frequency_hz": 110
  },
  "auto_start": false,
  "is_active": true,
  "set_as_default": true,
  "speakers": {
    "alsa_output.pci-0000_00_1f.3.HiFi__HDMI1__sink": {
      "sink_id": 55,
      "sink_name": "alsa_output.pci-0000_00_1f.3.HiFi__HDMI1__sink",
      "display_name": "HDMI 1 (Left Monitor)",
      "role": "left",
      "volume_gain": 1.0,
      "delay_ms": 12.5,
      "mute": false,
      "custom_name": null,
      "phase_inverted": false
    },
    "alsa_output.pci-0000_00_1f.3.HiFi__HDMI2__sink": {
      "sink_id": 43,
      "sink_name": "alsa_output.pci-0000_00_1f.3.HiFi__HDMI2__sink",
      "display_name": "HDMI 2 (Right Monitor)",
      "role": "right",
      "volume_gain": 1.0,
      "delay_ms": 12.5,
      "mute": false,
      "custom_name": null,
      "phase_inverted": false
    },
    "alsa_output.usb-Analog_Sub.analog-stereo": {
      "sink_id": 54,
      "sink_name": "alsa_output.usb-Analog_Sub.analog-stereo",
      "display_name": "USB DAC Subwoofer",
      "role": "subwoofer",
      "volume_gain": 1.15,
      "delay_ms": 0.0,
      "mute": false,
      "custom_name": null,
      "phase_inverted": false
    }
  }
}
```

---

## 📜 License

This project is licensed under the MIT License. See [LICENSE](LICENSE) for details.
