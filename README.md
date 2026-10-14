<p align="center">
  <img src="assets/io.github.taoshan98.Polifonia.svg" width="130" height="130" alt="Polifonia Logo">
</p>

<h1 align="center">Polifonia Audio Studio</h1>

<p align="center">
  <strong>Professional Cross-Desktop Multi-Speaker & Multi-Monitor Audio Unison Engine for Linux (PipeWire)</strong>
</p>

<p align="center">
  <a href="https://opensource.org/licenses/MIT"><img src="https://img.shields.io/badge/License-MIT-blue.svg" alt="License: MIT"></a>
  <a href="https://pipewire.org"><img src="https://img.shields.io/badge/Platform-Linux%20%28PipeWire%29-orange.svg" alt="Platform: Linux PipeWire"></a>
  <a href="https://www.python.org/"><img src="https://img.shields.io/badge/Python-3.10%2B-brightgreen.svg" alt="Python 3.10+"></a>
  <a href="#packaging--distribution"><img src="https://img.shields.io/badge/Packages-Flatpak%20%7C%20AppImage-purple.svg" alt="Packages"></a>
</p>

---

**Polifonia Audio Studio** is an advanced, PipeWire-native multi-speaker audio compositing, synchronization, and acoustic DSP management system for the Linux desktop (compatible with **GNOME**, **KDE Plasma**, **XFCE**, **Cinnamon**, **Hyprland**, and **Sway**).

It enables workstations with heterogeneous audio topologies — **HDMI / DisplayPort** integrated monitor speakers, **USB DACs**, **S/PDIF optical** receivers, **PCIe / Analog Jacks**, **Bluetooth A2DP** speakers (SBC, AAC, aptX, LDAC), **Smart Speakers** with proprietary internal DSP buffers (e.g. Amazon Echo, HomePod), and **Network / AirPlay / RAOP** endpoints — to operate simultaneously as a single, phase-coherent, sample-aligned **Multichannel Virtual Soundstage** (`polifonia_master`).

---

## Table of Contents

- [Overview & Problem Statement](#overview--problem-statement)
- [Architecture & System Design](#architecture--system-design)
  - [1. Universal Hardware-Agnostic Discovery](#1-universal-hardware-agnostic-discovery)
  - [2. Seamless Real-Time Unison Engine](#2-seamless-real-time-unison-engine)
  - [3. Interactive Live Routing Graph & Studio UI](#3-interactive-live-routing-graph--studio-ui)
- [Mathematical & DSP Algorithms](#mathematical--dsp-algorithms)
  - [Level 1: Real-Time PipeWire SPA Telemetry & Differential Compensation](#level-1-real-time-pipewire-spa-telemetry--differential-compensation)
  - [Level 2: Closed-Loop Acoustic System Identification (Auto-Sync)](#level-2-closed-loop-acoustic-system-identification-auto-sync)
  - [FFT Hilbert Analytic Envelope Cross-Correlation](#fft-hilbert-analytic-envelope-cross-correlation)
  - [Direct-Wavefront Detection & Sub-Sample Parabolic Interpolation](#direct-wavefront-detection--sub-sample-parabolic-interpolation)
  - [Audiophile DSP Pipeline, LR4 Crossover & Phase Alignment](#audiophile-dsp-pipeline-lr4-crossover--phase-alignment)
- [Installation & Quick Start](#installation--quick-start)
- [Packaging & Distribution](#packaging--distribution)
  - [Flatpak (Flathub)](#1-flatpak-flathub--recommended)
  - [AppImage](#2-appimage-standalone-portable-bundle)
- [Running Automated Tests](#running-automated-tests)
- [License](#license)

---

## Overview & Problem Statement

Combining multiple disparate audio outputs on a modern workstation into a single unified soundstage exposes four fundamental physical and protocol challenges:

1. **Multi-Order Transport Latency Disparity**: Direct PCIe/USB/HDMI sinks operate with $5\text{–}15\text{ ms}$ DMA buffers, standard Bluetooth A2DP sinks operate at $120\text{–}250\text{ ms}$, and smart/network speakers introduce additional internal multi-room DSP buffers ($150\text{–}1000+\text{ ms}$).
2. **Unreported "Black-Box" DSP Buffers**: While standard Bluetooth speakers report their A2DP buffer via the **AVDTP v1.3 Delay Reporting** service (`Latency` in PipeWire), smart speakers with internal OS/DSP pipelines (such as FireOS on Amazon Echo devices) report only the Bluetooth radio transceiver delay while hiding an additional internal DSP buffer before the DAC.
3. **Phase Rotation & Acoustic Comb Filtering**: Different speaker crossovers, bass-reflex ports, and physical distances rotate carrier phase ($\phi \in [0, 2\pi)$) and produce room reflections that cause destructive interference and comb filtering when played together.
4. **Stream Interruption on Topology Changes**: Naive loopback approaches drop or pause media streams (Spotify, Chrome, VLC) whenever a speaker is toggled, hot-plugged, or re-aligned.

Polifonia solves these challenges using a **100% hardware-agnostic, zero-hardcoding mathematical engine** combining live PipeWire SPA protocol telemetry (**Level 1**) with closed-loop microphone impulse-response system identification (**Level 2**).

> For the complete mathematical derivations and systems engineering reference, see [`docs/ALGORITHMS_AND_ARCHITECTURE.md`](file:///home/ntm/Develop/Taoshan/polifonia/docs/ALGORITHMS_AND_ARCHITECTURE.md).

---

## Architecture & System Design

```
polifonia/
├── backend/
│   ├── pipewire_config.py     # SPA filter-chain, LR4 crossover & delay graph generator
│   └── pipewire_scanner.py    # Dynamic ELD/EDID, USB/SPDIF, Bluetooth & SPA latency scanner
├── core/
│   └── models.py              # Strongly-typed domain data models (AudioSink, SpeakerConfig, SpeakerRole)
├── services/
│   ├── audio_engine.py        # Real-time Unison supervisor, watchdog & closed-loop acoustic calibration
│   └── audio_service.py       # Service provider interface alias
├── storage/
│   ├── preset_manager.py      # Named profile management proxy
│   └── settings_store.py      # XDG-compliant JSON persistence (~/.config/polifonia/)
├── ui/
│   ├── styles/
│   │   └── studio_theme.css   # Custom cross-desktop dark studio CSS theme
│   ├── views/
│   │   ├── main_window.py     # Main studio window, header bar & view stack
│   │   ├── routing_graph.py   # Interactive Cairo node-graph & real-time telemetry view
│   │   └── speaker_card.py    # Per-speaker channel strip, role matrix & calibration popover
│   └── tray/
│       ├── tray_service.py    # Background tray lifecycle manager
│       └── tray_indicator.py  # System tray indicator process
├── docs/
│   └── ALGORITHMS_AND_ARCHITECTURE.md  # Full mathematical & algorithmic specification
├── tests/                     # 80-test unit & integration suite
├── packaging/                 # Flatpak, AppImage, Debian (.deb) & RPM specs
├── main.py                    # Application entrypoint
└── README.md
```

### 1. Universal Hardware-Agnostic Discovery
- **ELD/EDID Display Audio Resolver**: Scans `/proc/asound/card*/eld*` across NVIDIA, AMD Radeon, and Intel GPUs to resolve commercial display model names (e.g. *Odyssey G61SD*, *BenQ EW2480*) and physical bus types (`hdmi`, `dp`).
- **Pro-Audio Multi-Head Unlocker**: Automatically unlocks multi-head GPU audio controllers so every connected HDMI/DisplayPort monitor is exposed as an independent stereo sink.
- **Full Bus Classification**: Automatically classifies and preserves **HDMI**, **DisplayPort**, **USB Audio / USB IEC958**, **S/PDIF Optical (`iec958`)**, **PCIe / Analog Jack**, **Bluetooth A2DP (`bluez_output.*`)**, and **Network / AirPlay / RAOP / RTP (`raop_sink.*`, `rtp.*`)** endpoints while filtering out phantom unconnected ports and internal laptop SOF speakers.

### 2. Seamless Real-Time Unison Engine
- **High-Priority Virtual Master Sink (`polifonia_master`)**: Created with `priority.driver=30000` and `priority.session=30000` so Bluetooth hotplug events can never hijack the default desktop output while Unison is active.
- **Zero-Glitch Branch Supervision**:
  - Wired endpoints run with `node.passive=true` and tight quantum buffers (`-l 15`).
  - Bluetooth and wireless endpoints run in continuous active follower mode (`node.passive=false`, `node.dont-reconnect=true`) and are muted/unmuted in $<10\text{ ms}$ via `wpctl` without tearing down the A2DP transport.
  - Modifying the delay or phase of one speaker restarts **only** the affected `pw-loopback` branch, leaving all other speakers playing without a single dropped sample.

### 3. Interactive Live Routing Graph & Studio UI
- **Channel Rack & Live Cairo Routing Graph**: Visualizes active application streams (`sink-inputs`) flowing into `polifonia_master` and branching out to each physical speaker with real-time hardware latency, auto-delay compensation, user/calibrated offset, and total buffer metrics.

---

## Mathematical & DSP Algorithms

```
[ Desktop Apps ] ──► [ polifonia_master Virtual Sink ]
                                   │
         ┌─────────────────────────┼─────────────────────────┐
         ▼                         ▼                         ▼
 [ Wired HDMI Branch ]     [ Standard BT Branch ]    [ Smart DSP BT Branch ]
  HW Latency: 10.0 ms       HW Latency: 144.1 ms      HW Latency: 155.9 ms
  Calib Offset: 0.0 ms      Calib Offset: 0.0 ms      Calib Offset: -170.0 ms (Unreported DSP)
  Applied Delay: 315.9 ms   Applied Delay: 181.8 ms   Applied Delay: 0.0 ms
         │                         │                         │
         ▼                         ▼                         ▼
  Physical Arrival:         Physical Arrival:         Physical Arrival:
      325.9 ms                  325.9 ms                  325.9 ms  ──► PERFECT UNISON
```

### Level 1: Real-Time PipeWire SPA Telemetry & Differential Compensation
Polifonia inspects `pw-dump` SPA node parameters (`Props` and `Params`) on every scan and watchdog cycle, extracting both transport latency (`Latency`: `minNs`, `maxNs`) and internal node processing latency (`ProcessLatency`: `ns`):

$$\tau_{\text{hw},i} = \frac{\frac{1}{2}(\text{minNs}_i + \text{maxNs}_i) + \text{ProcessLatency}_{\text{ns},i}}{10^6}\text{ [ms]}$$

When a Bluetooth A2DP sink is temporarily suspended (`IDLE`) and has not yet published dynamic AVDTP telemetry, Polifonia estimates the baseline transport latency from `api.bluez5.codec` (`aptX LL`, `FastStream`, `LC3`, `aptX`, `SBC-XQ`, `SBC`, `AAC`, `LDAC`) and automatically replaces it with live AVDTP telemetry as soon as the stream transitions to `RUNNING`.

For $N$ active speakers with hardware latencies $\{\tau_{\text{hw},1}, \dots, \tau_{\text{hw},N}\}$ and acoustic/fine offsets $\{\delta_1, \dots, \delta_N\}$, let:

$$\tau_{\text{hw},\max} = \max_{j} \tau_{\text{hw},j}, \qquad \delta_{\min} = \min_{j} \delta_j$$

Each `pw-loopback` branch $i$ is assigned the exact non-negative delay:

$$D_i = \underbrace{(\tau_{\text{hw},\max} - \tau_{\text{hw},i})}_{\text{Level 1 Auto-Delay}} + \underbrace{(\delta_i - \delta_{\min})}_{\text{Normalized Fine/Calibrated Delay}} \ge 0$$

A background **Drift Watchdog** monitors live PipeWire telemetry every $2\text{ s}$ and automatically re-synchronizes branches if any wireless transport drifts by $>25\text{ ms}$.

---

### Level 2: Closed-Loop Acoustic System Identification (`Auto-Sync`)
To align speakers with unreported internal DSP buffers (or different physical distances from the listener) without any device-name hardcoding, Polifonia implements a **Closed-Loop Acoustic System Identification** engine:

1. **Common-Clock Virtual Sink Injection**:
   Instead of spawning `pw-play` directly against individual hardware sinks (which introduces $20\text{–}180\text{ ms}$ of target-dependent OS/BlueZ socket negotiation jitter), Polifonia keeps the live `polifonia_master` $\rightarrow$ `pw-loopback` graph running, mutes external desktop applications (`pactl set-sink-input-mute`), isolates one speaker branch at a time, and injects a windowed linear FM chirp ($1.2\text{ kHz} \rightarrow 3.5\text{ kHz}$, $60\text{ ms}$ Hann envelope, padded with $50\text{ ms}$ pre-silence and $250\text{ ms}$ post-silence to prevent premature `EOS` truncation) into `polifonia_master`.
   Because `polifonia_master` is a zero-latency RAM null-sink, the `pw-play` startup overhead $\tau_0$ is constant across all probes and **cancels out identically** in differential arrival calculations.
2. **Full Buffer Drain Isolation**:
   After injecting the probe for speaker $i$, the engine waits for both the active `pw-loopback` delay $D_i$ and the physical/wireless pipeline to drain completely:
   $$T_{\text{drain},i} = \frac{D_i}{1000} + \begin{cases} 1.10\text{ s} & \text{wireless / network} \\ 0.45\text{ s} & \text{wired} \end{cases}$$
   before muting speaker $i$ and advancing to speaker $i+1$.
3. **Exact Residual DSP & Time-of-Flight Deduction**:
   From the microphone recording, the engine measures the total arrival latency $\tau_{\text{meas},i}$, subtracts the known active `pw-loopback` delay $D_i$ to obtain the intrinsic physical latency $\tau_{\text{intr},i} = \tau_{\text{meas},i} - D_i$, and subtracts the Level 1 hardware telemetry delta already compensated by `sync_active_branches`:
   $$\delta_i = -\Big((\tau_{\text{intr},i} - \tau_{\text{intr,ref}}) - (\tau_{\text{hw},i} - \tau_{\text{hw,ref}})\Big) = -\left(\tau_{\text{dsp},i} + \frac{d_i - d_{\text{ref}}}{c}\right)$$
   This automatically isolates the exact unreported DSP buffer plus acoustic distance delta (e.g., $\delta_i = -170.0\text{ ms}$) with sub-millisecond accuracy.

---

### FFT Hilbert Analytic Envelope Cross-Correlation
Standard time-domain cross-correlation $|R_{ys}[m]| = \left|\sum_n y[n]s[n-m]\right|$ exhibits oscillations at the $2.2\text{ kHz}$ carrier frequency and suffers a null at the true arrival peak if a speaker's crossover or acoustic reflection rotates carrier phase by $90^\circ$ (quadrature).

Polifonia computes the **Hilbert Analytic Matched-Filter Envelope** in $O(L \log L)$ via FFT:

$$X[k] = \text{FFT}(y - \bar{y})[k] \cdot \overline{\text{FFT}(s - \bar{s})[k]}$$

Applying the single-sided frequency-domain Hilbert operator $H[k]$ ($H[0]=1$, $H[1 \dots N_{\text{fft}}/2 - 1]=2$, $H[N_{\text{fft}}/2]=1$, $H[>N_{\text{fft}}/2]=0$):

$$E[m] = \left|\text{IFFT}\big(X[k] \cdot H[k]\big)[m]\right| = \sqrt{R_{ys}[m]^2 + \mathcal{H}\{R_{ys}\}[m]^2}$$

Because a carrier phase shift $e^{j\phi}$ simply rotates the complex analytic phasor without changing its modulus ($|z \cdot e^{j\phi}| = |z|$), $E[m]$ is **100% invariant to any phase rotation $\phi \in [0, 2\pi)$** and yields a smooth, ripple-free group-delay envelope.

---

### Direct-Wavefront Detection & Sub-Sample Parabolic Interpolation
In real rooms, a desk or wall reflection arriving $2\text{–}15\text{ ms}$ after the direct line-of-sight sound wave can exhibit higher amplitude than the direct arrival due to constructive interference.
- **Early Direct-Path Discrimination**: `_detect_first_arrival_peak` scans the window $[m_{\max} - 15\text{ ms}, m_{\max} - 2\text{ ms}]$ ahead of the global maximum $m_{\max}$ and locks onto the earliest wavefront exceeding $3.5\times$ the noise floor and $65\%$ of $E[m_{\max}]$ separated by a distinct valley ($\le 55\%$).
- **3-Point Parabolic Sub-Sample Refinement**: Refines the discrete sample index $m_0$ at $48\text{ kHz}$ to continuous sub-sample precision ($<0.02\text{ ms}$):

$$m_{\text{exact}} = m_0 + \frac{1}{2}\frac{E[m_0 - 1] - E[m_0 + 1]}{E[m_0 - 1] - 2E[m_0] + E[m_0 + 1]}$$

---

### Audiophile DSP Pipeline, LR4 Crossover & Phase Alignment
- **Audiophile Resampling & Headroom Protection**: Every loopback branch and filter-chain runs with `resample.quality = 10` (highest PipeWire Kaiser-windowed sinc interpolation), `channelmix.normalize = true` (prevents digital clipping when downmixing stereo to mono), and `channelmix.upmix = true`.
- **4th-Order Linkwitz-Riley ($24\text{ dB/oct}$) Active Crossover**: Cascades two matched 2nd-order Butterworth lowpass/highpass biquad stages (`bq_lowpass` $\times 2$ for `SUBWOOFER`, `bq_highpass` $\times 2$ for satellites). At the crossover frequency $f_c$, both branches are at $-6.02\text{ dB}$ and $360^\circ$ ($0^\circ$ net) phase shift, guaranteeing a flat $0\text{ dB}$ acoustic sum without phase cancellation.
- **7.1 / 5.1 Spatial Roles & $180^\circ$ Polarity Inversion (`Ø`)**: Supports `STEREO`, `LEFT`, `RIGHT`, `CENTER`, `SUBWOOFER`, `SURROUND_LEFT` (`RL`), and `SURROUND_RIGHT` (`RR`), plus per-speaker $180^\circ$ polarity inversion (`phase_inverted`).

---

## Installation & Quick Start

### Prerequisites
- Linux Kernel 5.15+
- PipeWire 0.3.50+ with `pipewire-pulse`, `pipewire-bin` (`pw-loopback`, `pw-play`, `pw-record`, `pw-dump`), and `wireplumber` (`wpctl`)
- Python 3.10+ with `python3-gi` (GTK4 / Libadwaita bindings) and `python3-numpy`

```bash
# Debian / Ubuntu
sudo apt install python3-gi python3-numpy gir1.2-gtk-4.0 gir1.2-adw-1 pipewire pipewire-pulse pipewire-bin wireplumber pulseaudio-utils

# Fedora / RHEL
sudo dnf install python3-gobject python3-numpy gtk4 libadwaita pipewire pipewire-pulseaudio pipewire-utils wireplumber pulseaudio-utils

# Arch Linux
sudo pacman -S python-gobject python-numpy gtk4 libadwaita pipewire pipewire-pulse wireplumber libpulse
```

### Running from Source
```bash
git clone https://github.com/Taoshan98/polifonia.git
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

Polifonia focuses on universal, cross-distribution delivery formats:

### 1. Flatpak (Flathub) — *Recommended*
Flathub is the primary desktop distribution channel for Polifonia across all Linux distributions (GNOME, Fedora, Ubuntu, Arch, SteamOS):

```bash
# Install via Flatpak / Flathub
flatpak install flathub io.github.taoshan98.Polifonia

# Run application
flatpak run io.github.taoshan98.Polifonia
```

Or search for **Polifonia** directly in GNOME Software or KDE Discover.

### 2. AppImage (Standalone Portable Bundle)
For users who prefer a single standalone portable executable without containerization:

1. Download `Polifonia-<version>-x86_64.AppImage` from [GitHub Releases](https://github.com/Taoshan98/polifonia/releases).
2. Make it executable and run:
   ```bash
   chmod +x Polifonia-*-x86_64.AppImage
   ./Polifonia-*-x86_64.AppImage
   ```

To build locally:
```bash
./packaging/appimage/build_appimage.sh
# Generate bundle using appimagetool:
appimagetool build/AppDir Polifonia-x86_64.AppImage
```

---

### Roadmap (Native Distro Repositories)
Native package specifications (`packaging/aur/`, `packaging/debian/`, `packaging/rpm/`) are maintained in the repository and will be activated on respective community repositories (Arch AUR, Ubuntu PPA, Fedora COPR) in upcoming releases.

---

## Running Automated Tests

Run the complete **80-test** automated unit and integration suite:
```bash
python3 -m unittest discover -s tests -v
```

---

## License

Distributed under the **MIT License**. See [LICENSE](file:///home/ntm/Develop/Taoshan/polifonia/LICENSE) for details.

