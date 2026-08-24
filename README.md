<p align="center">
  <img src="assets/io.polifonia.AudioStudio.svg" width="130" height="130" alt="Polifonia Audio Studio Logo">
</p>

<h1 align="center">Polifonia Audio Studio</h1>

<p align="center">
  <strong>Professional Cross-Desktop Multi-Speaker & Multi-Monitor Audio Unison Engine for Linux (PipeWire)</strong>
</p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://pipewire.org"><img src="https://img.shields.io/badge/Platform-Linux%20%28PipeWire%29-orange.svg" alt="Platform: Linux PipeWire"></a>
  <a href="https://www.python.org/"><img src="https://img.shields.io/badge/Python-3.10%2B-brightgreen.svg" alt="Python 3.10+"></a>
  <a href="#packaging--distribution"><img src="https://img.shields.io/badge/Packages-Flatpak%20%7C%20AppImage%20%7C%20DEB%20%7C%20RPM-purple.svg" alt="Packages"></a>
</p>

---

**Polifonia Audio Studio** is an advanced, PipeWire-native multi-speaker audio compositing and acoustic DSP management system for the Linux desktop (compatible with **GNOME**, **KDE Plasma**, **XFCE**, **Cinnamon**, **Hyprland**, and **Sway**).

It enables workstations with heterogeneous multi-monitor setups (HDMI/DisplayPort integrated screen speakers, USB audio DACs, auxiliary subwoofers, and analog outputs) to aggregate disparate physical outputs into a unified, phase-aligned, frequency-filtered **Multichannel Virtual Soundstage**.

---

## Table of Contents

- [Overview & Problem Statement](#overview--problem-statement)
- [Architecture & System Design](#architecture--system-design)
  - [Hardware-Agnostic Dynamic Discovery](#1-hardware-agnostic-dynamic-discovery)
  - [Seamless Real-Time Unison Engine](#2-seamless-real-time-unison-engine)
  - [Custom Cross-Desktop UI Theme](#3-custom-cross-desktop-ui-theme)
- [Technical Capabilities](#technical-capabilities)
  - [Dynamic Multi-Sink Routing](#1-dynamic-multi-sink-routing)
  - [Time Alignment & Delay Compensation](#2-time-alignment--delay-compensation)
- [Installation & Quick Start](#installation--quick-start)
- [Packaging & Distribution](#packaging--distribution)
  - [Flatpak (Flathub)](#1-flatpak-flathub)
  - [AppImage](#2-appimage)
  - [Debian / Ubuntu (.deb)](#3-debian--ubuntu-deb)
  - [Fedora / RHEL / openSUSE (.rpm)](#4-fedora--rhel--opensuse-rpm)
- [Running Automated Tests](#running-automated-tests)
- [License](#license)

---

## Overview & Problem Statement

Modern multi-monitor workstations frequently feature integrated speakers across multiple displays (e.g. left, center and right HDMI/DisplayPort screens) alongside a dedicated auxiliary speaker or USB subwoofer. Standard Linux audio servers expose these as isolated, independent hardware sinks.

Operating them simultaneously in unison presents significant acoustic challenges:
1. **Frequency Collisions**: Small monitor speakers distort when attempting to reproduce low-frequency bass notes, while subwoofers muddy vocals when reproducing high-frequency content.
2. **Phase Cancellation & Buffer Latency**: HDMI display controllers introduce distinct frame-buffer latencies relative to direct analog or USB outputs, causing comb filtering and phase cancellation.
3. **Imbalanced Volumes**: Divergent DAC sensitivity across monitors results in skewed stereo imaging.

Polifonia solves this by providing a unified PipeWire routing layer with integrated active DSP filtering, sample-accurate delay compensation, and dynamic hardware unlocking.

---

## Architecture & System Design

```
polifonia/
├── backend/
│   ├── pipewire_config.py     # SPA filter-chain & loopback graph generator
│   └── pipewire_scanner.py    # Dynamic ELD & PipeWire graph discovery
├── core/
│   └── models.py              # Strongly-typed domain data models & enums
├── services/
│   ├── audio_engine.py        # Process supervisor & loopback sync
│   └── audio_service.py       # Service provider interface alias
├── storage/
│   ├── preset_manager.py      # Profile management proxy
│   └── settings_store.py      # XDG-compliant JSON persistence
├── ui/
│   ├── styles/
│   │   └── studio_theme.css   # Custom cross-desktop dark studio CSS theme
│   ├── views/
│   │   ├── main_window.py     # Main application window & control bar
│   │   └── speaker_card.py    # Per-speaker channel strip row widget
│   └── tray/
│       ├── tray_service.py    # Tray manager
│       └── tray_indicator.py  # System tray indicator process
├── packaging/
│   ├── flatpak/               # Flathub manifest (io.polifonia.AudioStudio.json)
│   ├── appimage/              # AppImage AppRun and build script
│   ├── debian/                # Debian / Ubuntu package control files
│   └── rpm/                   # Fedora / openSUSE RPM specfile
├── assets/
│   └── io.polifonia.AudioStudio.svg  # High-resolution vector icon
├── pyproject.toml             # Standard PEP 621 Python package configuration
├── io.polifonia.AudioStudio.desktop # FreeDesktop XDG launcher entry
├── main.py                    # Application entrypoint
└── README.md
```

### 1. Hardware-Agnostic Dynamic Discovery
- **Universal ELD/EDID Monitor Resolver**: Automatically scans `/proc/asound/card*/eld*` across any graphics vendor (NVIDIA, AMD Radeon, Intel Arc / Iris). Dynamically extracts commercial monitor model names (e.g. *Odyssey G61SD*, *BenQ EW2480*, *LG UltraGear*, *Dell UltraSharp*) and connection types (*HDMI*, *DisplayPort*).
- **Pro-Audio Multi-Head Unlocker**: Detects graphics cards with multi-head audio capabilities and automatically sets the card profile to `pro-audio`, turning every connected display into an independent audio sink.
- **Smart Jack Filtering**: Disconnected phantom ports and internal unused DSP endpoints are automatically filtered out.

### 2. Seamless Real-Time Unison Engine
- **Persistent Virtual Sink (`polifonia_master`)**: Created once and maintained continuously. Toggling speaker channels or adjusting volume sliders never interrupts active audio streams or causes media players (Spotify, YouTube, VLC) to pause.
- **Granular Branch Synchronization**: Adding or removing speakers spawns or terminates only the relevant `pw-loopback` branches asynchronously without blocking the UI main loop.
- **Hardware Volume Synchronization**: Individual speaker gain changes are applied directly to ALSA/Pulse sinks in real time with 0ms latency.

### 3. Custom Cross-Desktop UI Theme
- Custom dark cyber studio stylesheet (`ui/styles/studio_theme.css`) ensures an identical, visually stunning, high-contrast experience across all desktop environments (GNOME, KDE Plasma, XFCE, Sway, Hyprland).

---

## Technical Capabilities

### 1. Dynamic Multi-Sink Routing
Polifonia deploys dedicated PipeWire loopback nodes linked to designated physical hardware sinks:
```
[ Desktop Audio ] ---> [ polifonia_master Virtual Sink ]
                                │
        ┌───────────────────────┼───────────────────────┐
        ▼                       ▼                       ▼
 [ Left Loopback ]      [ Right Loopback ]      [ Subwoofer Loopback ]
  (Audio: FL)            (Audio: FR)             (Audio: FL+FR Mono)
  (Delay: e.g. 15ms)     (Delay: e.g. 15ms)      (Delay: 0ms)
        │                       │                       │
        ▼                       ▼                       ▼
 [ Left Monitor DP ]    [ Right Monitor DP ]    [ AUX / USB Subwoofer ]
```

### 2. Time Alignment & Delay Compensation
Compensates for differential path distances and DAC latency with millisecond precision:
$$\Delta t = \frac{d}{c}$$
Where $c \approx 343\text{ m/s}$ (speed of sound in air). A distance delta of $1\text{ meter} \approx 2.9\text{ ms}$.

---

## Installation & Quick Start

### Prerequisites
- Linux Kernel 5.15+
- PipeWire 0.3.50+ with `pipewire-pulse` and `wireplumber`
- Python 3.10+ with `python3-gi` (GTK4 / Libadwaita bindings)

```bash
# Debian / Ubuntu
sudo apt install python3-gi gir1.2-gtk-4.0 gir1.2-adw-1 pipewire pipewire-pulse wireplumber

# Fedora / RHEL
sudo dnf install python3-gobject gtk4 libadwaita pipewire pipewire-pulseaudio wireplumber

# Arch Linux
sudo pacman -S python-gobject gtk4 libadwaita pipewire pipewire-pulse wireplumber
```

### Running from Source
```bash
git clone https://github.com/taoshan/polifonia.git
cd polifonia
python3 main.py
```

### Installing via pip
```bash
pip install .
polifonia
```

---

## Packaging & Distribution

Polifonia provides ready-to-use configurations for all major Linux distribution channels:

### 1. Flatpak (Flathub)
```bash
flatpak-builder --user --install --force-clean build-dir packaging/flatpak/io.polifonia.AudioStudio.json
flatpak run io.polifonia.AudioStudio
```

### 2. AppImage
```bash
./packaging/appimage/build_appimage.sh
# Generate bundle with appimagetool:
appimagetool build/AppDir Polifonia-x86_64.AppImage
```

### 3. Debian / Ubuntu (.deb)
```bash
dpkg-buildpackage -us -uc -b
```

### 4. Fedora / RHEL / openSUSE (.rpm)
```bash
rpmbuild -ba packaging/rpm/polifonia.spec
```

---

## Running Automated Tests

Run the complete 38-test automated suite:
```bash
python3 -m unittest discover tests
```

---

## License

Distributed under the **MIT License**. See [LICENSE](file:///home/ntm/Develop/Taoshan/polifonia/LICENSE) for details.
