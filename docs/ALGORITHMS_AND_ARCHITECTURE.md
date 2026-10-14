# Polifonia Audio Studio — Mathematical Architecture, DSP Pipeline & Closed-Loop Acoustic Synchronization

This document provides the complete mathematical formulation, digital signal processing (DSP) algorithms, and systems engineering architecture implemented in **Polifonia Audio Studio**.

---

## 1. Engineering Objective & Hardware-Agnostic Design Constraint

The core engineering objective of Polifonia is to **aggregate $N$ heterogeneous physical audio outputs** — HDMI/DisplayPort integrated monitor speakers, USB DACs, S/PDIF optical receivers (`iec958`), PCIe/Analog jacks, Bluetooth A2DP speakers (`SBC`, `AAC`, `aptX`, `LDAC`, `LC3`), smart speakers with proprietary internal DSP pipelines, and Network/AirPlay/RAOP/RTP sinks — into a single phase-coherent, sample-aligned virtual device (`polifonia_master`).

### Fundamental Constraint: Zero Hardcoding
Polifonia enforces a strict scientific constraint: **no latency offset is ever hardcoded by device name, brand, or model string**. All time-alignment compensation is derived exclusively from:
1. **Level 1 (Protocol & Driver Telemetry)**: Deterministic extraction of real-time PipeWire SPA node parameters (`Latency` and `ProcessLatency`).
2. **Level 2 (Closed-Loop Acoustic System Identification via Microphone)**: Empirical measurement of each speaker's physical impulse response through the live `polifonia_master` $\rightarrow$ `pw-loopback` graph.

---

## 2. End-to-End Physical Latency Model

For any active speaker $i \in \{1, \dots, N\}$, the total physical propagation time $\tau_{\text{tot},i}$ between writing a PCM frame into `polifonia_master` and the acoustic wavefront arriving at the listener's microphone (or ear) is the sum of four physical components:

$$\tau_{\text{tot},i} = D_i + \tau_{\text{hw},i} + \tau_{\text{dsp},i} + \tau_{\text{air},i}$$

where:
- $D_i \ge 0$: Programmable digital delay applied by Polifonia on speaker $i$'s `pw-loopback` branch (`--delay`).
- $\tau_{\text{hw},i}$: Hardware/transport latency reported by the PipeWire/ALSA/BlueZ subsystem (DMA ring-buffer latency for wired sinks, or codec packetization + **AVDTP v1.3 Delay Reporting** for Bluetooth A2DP sinks).
- $\tau_{\text{dsp},i}$: Unreported internal "black-box" DSP buffer inside the speaker firmware **not included** in protocol telemetry (e.g., proprietary post-decoder multi-room/EQ buffers inside smart speakers such as Amazon Echo Dot, which add $\sim 170\text{ ms}$ beyond the Bluetooth AVDTP transceiver report).
- $\tau_{\text{air},i} = \frac{d_i}{c}$: Acoustic time-of-flight in air from speaker $i$'s driver cone to the listening position ($c \approx 343\text{ m/s}$, i.e., $\approx 2.915\text{ ms/m}$).

The condition for **true physical acoustic unison** requires all $N$ wavefronts to arrive at the exact same instant $T_{\text{target}}$:

$$\tau_{\text{tot},1} = \tau_{\text{tot},2} = \dots = \tau_{\text{tot},N} = T_{\text{target}}$$

---

## 3. Level 1: PipeWire SPA Telemetry & Dynamic Differential Compensation

### 3.1 Extraction of `Latency` and `ProcessLatency`
In [`backend/pipewire_scanner.py`](../backend/pipewire_scanner.py), `PipeWireScanner.scan_sinks` inspects the live PipeWire graph via `pw-dump` and extracts both SPA parameter dictionaries (`Props` and `Params`) for each `Audio/Sink` node:
- `Latency`: The nanosecond transport range $[\text{minNs}, \text{maxNs}]$ negotiated by the ALSA/USB kernel driver or reported by the BlueZ stack via AVDTP Delay Reporting.
- `ProcessLatency`: Internal node processing latency in nanoseconds ($\text{ns}$).

The telemetric hardware latency $\tau_{\text{hw},i}$ (in milliseconds) is computed as:

$$\tau_{\text{hw},i} = \frac{\frac{1}{2}(\text{minNs}_i + \text{maxNs}_i) + \text{ProcessLatency}_{\text{ns},i}}{10^6}$$

If a Bluetooth A2DP sink is temporarily in an `IDLE`/`SUSPENDED` state and has not yet populated live `minNs`/`maxNs` counters, the scanner estimates baseline transport latency from the negotiated `api.bluez5.codec` property (`aptX LL` $40\text{ ms}$, `FastStream` $55\text{ ms}$, `LC3` $60\text{ ms}$, `aptX`/`aptX-HD` $130\text{ ms}$, `SBC-XQ` $150\text{ ms}$, `SBC` $160\text{ ms}$, `AAC` $180\text{ ms}$, `LDAC` $200\text{ ms}$), automatically replacing the estimate with live AVDTP telemetry as soon as the stream transitions to `RUNNING`.

### 3.2 Non-Negative Differential Compensation Matrix
In [`services/audio_engine.py`](../services/audio_engine.py) (`AudioEngineService.sync_active_branches`), given the hardware latency vector $[\tau_{\text{hw},1}, \dots, \tau_{\text{hw},N}]$ and the acoustic/user calibration vector $[\delta_1, \dots, \delta_N]$, the engine computes:

$$\tau_{\text{hw},\max} = \max_{j \in \{1 \dots N\}} \tau_{\text{hw},j}, \qquad \delta_{\min} = \min_{j \in \{1 \dots N\}} \delta_j$$

Because a causal digital delay line cannot advance audio into the past ($D_i \ge 0$), Polifonia normalizes both contributions and assigns to each `pw-loopback` branch $i$:

$$D_i = \underbrace{(\tau_{\text{hw},\max} - \tau_{\text{hw},i})}_{\text{Level 1 Telemetric Auto-Delay}} + \underbrace{(\delta_i - \delta_{\min})}_{\text{Normalized Relative Offset}} \ge 0$$

- **Property 1 (Zero-Config Standard Unison)**: For all standard wired and Bluetooth speakers whose protocol telemetry accurately reflects their physical pipeline ($\tau_{\text{dsp},i} = 0$) at equal listening distance ($\delta_i = 0$), $D_i = \tau_{\text{hw},\max} - \tau_{\text{hw},i}$, yielding an identical physical arrival time across all speakers:
  $$\tau_{\text{tot},i} = (\tau_{\text{hw},\max} - \tau_{\text{hw},i}) + \tau_{\text{hw},i} = \tau_{\text{hw},\max} = \text{const}$$
- **Property 2 (Unreported DSP Compensation)**: If speaker $k$ has an unreported internal DSP buffer $\tau_{\text{dsp},k} > 0$, setting $\delta_k = -\tau_{\text{dsp},k}$ yields $\delta_{\min} = -\tau_{\text{dsp},k}$, which keeps $D_k$ minimal while advancing all other $N-1$ faster speakers by $+\tau_{\text{dsp},k}$.

### 3.3 Real-Time Drift Watchdog
The background supervisor thread `_start_watchdog` in [`services/audio_engine.py`](../services/audio_engine.py) polls process health and live PipeWire SPA telemetry every $2\text{ s}$:
- Any crashed `pw-loopback` process is respawned immediately.
- If any active sink's reported hardware latency drifts by more than $25.0\text{ ms}$ (e.g., dynamic A2DP buffer resizing or transport re-negotiation), the engine recomputes $D_i$ and updates only the affected branches without interrupting unaffected speakers.

---

## 4. Level 2: Closed-Loop Acoustic System Identification (`Auto-Sync`)

When a speaker contains an unreported internal DSP buffer ($\tau_{\text{dsp},i} \neq 0$) or sits at a different physical distance from the listener ($\tau_{\text{air},i} \neq \tau_{\text{air,ref}}$), protocol telemetry alone cannot observe the hidden delay. `AudioEngineService.auto_align_all_speakers` performs **Closed-Loop Acoustic System Identification** using the system microphone.

### 4.1 Common-Clock Virtual Sink Injection (Eliminating Spawn Jitter)
Spawning `pw-play --target=<physical_sink>` directly against individual hardware endpoints introduces $20\text{–}180\text{ ms}$ of target-dependent process-fork and BlueZ/ALSA socket negotiation jitter between the Python `time.time()` call and actual PCM emission.

Polifonia eliminates this error by keeping the **live `polifonia_master` $\rightarrow$ `pw-loopback` graph running** throughout calibration:
1. External desktop application streams (`sink-inputs`) are temporarily muted (`pactl set-sink-input-mute <id> 1`) so background media cannot contaminate the microphone recording.
2. The virtual master sink `polifonia_master` remains **unmuted** (`mute = 0`) throughout all probe emissions.
3. For each speaker $i$, all other $N-1$ `pw-loopback` branches and physical sinks are muted, isolating speaker $i$'s branch.
4. The acoustic probe is played **exclusively into `polifonia_master`** (`pw-play --target=polifonia_master`). Because `polifonia_master` is a zero-latency RAM null-sink, the `pw-play` startup overhead $\tau_0$ is **constant across all speakers** and cancels out identically in differential calculations:
   $$(\tau_{\text{meas},i} + \tau_0) - (\tau_{\text{meas,ref}} + \tau_0) = \tau_{\text{meas},i} - \tau_{\text{meas,ref}}$$

### 4.2 `EOS` Truncation Guard & Full Buffer Drain Isolation (`drain_wait_s`)
Playing a bare $60\text{ ms}$ WAV file causes `pw-play` to send an `EOS` (End-Of-Stream) event after $60\text{ ms}$, which can prematurely close or truncate deep wireless/DSP buffers ($150\text{–}400\text{ ms}$). Furthermore, muting speaker $i$ immediately after `pw-play` exits would cut off the audio while it is still traversing the `pw-loopback --delay` buffer ($D_i$).

To guarantee complete physical emission:
1. **Pre/Post Silence Guard**: The probe WAV file is padded with $t_{\text{pre}} = 50\text{ ms}$ of leading silence, $60\text{ ms}$ of active chirp, and $t_{\text{post}} = 250\text{ ms}$ of trailing silence. The true emission timestamp relative to recording start is recorded as:
   $$t_{\text{emit,rel},i} = (t_{\text{emit},i} - t_{\text{start\_rec}}) + t_{\text{pre}}$$
2. **Full Buffer Drain Wait (`drain_wait_s`)**: Before muting speaker $i$ and advancing to speaker $i+1$, the engine waits for both the `pw-loopback` delay line $D_i$ and the physical/DSP buffer to drain completely:
   $$T_{\text{drain},i} = \frac{D_i}{1000} + \begin{cases} 1.10\text{ s} & \text{wireless / network} \\ 0.45\text{ s} & \text{wired} \end{cases}$$

### 4.3 Probe Signal Synthesis (Hann-Windowed Linear FM Chirp)
The probe signal $s(t)$ of duration $T_c = 60\text{ ms}$ at sample rate $f_s = 48000\text{ Hz}$ ($M = 2880$ samples) is a linear frequency sweep from $f_0 = 1200\text{ Hz}$ to $f_1 = 3500\text{ Hz}$ tapered by a Hann window $w(t)$:

$$s(t) = w(t) \cdot \sin\!\left(2\pi \left(f_0 t + \frac{f_1 - f_0}{2 T_c} t^2\right)\right), \qquad w(t) = \frac{1}{2}\left(1 - \cos\!\left(\frac{2\pi t}{T_c}\right)\right)$$

The $[1.2, 3.5]\text{ kHz}$ band maximizes signal-to-noise ratio across small monitor speakers, smart speakers, and laptop/webcam microphones while avoiding low-frequency room modes ($< 400\text{ Hz}$).

---

## 5. FFT Hilbert Analytic Envelope Cross-Correlation & Phase Invariance

### 5.1 Limitation of Standard Time-Domain Cross-Correlation
Standard zero-mean cross-correlation:

$$R_{ys}[m] = \sum_{n=0}^{M-1} \tilde{y}[n + m] \, \tilde{s}[n]$$

contains carrier oscillations at $\sim 2.35\text{ kHz}$. If a speaker's internal crossover filter, bass-reflex port, or acoustic reflection rotates the carrier phase by $\phi = 90^\circ$ (quadrature), $|R_{ys}[m]|$ exhibits a **local null (zero-crossing)** at the true group-delay peak $m_0$, causing peak-picking errors or sidelobe locking.

### 5.2 $O(L \log L)$ Frequency-Domain Hilbert Analytic Envelope
In [`AudioEngineService._compute_robust_correlation`](../services/audio_engine.py), Polifonia computes the **Hilbert Analytic Matched-Filter Envelope** in a single FFT/IFFT pass of size $N_{\text{fft}} = 2^{\lceil \log_2(N_{\text{rec}} + M - 1) \rceil}$:

1. Compute the forward FFTs of the zero-mean recorded signal $\tilde{y}$ and template $\tilde{s}$:
   $$Y[k] = \text{FFT}_{N_{\text{fft}}}(\tilde{y})[k], \qquad S[k] = \text{FFT}_{N_{\text{fft}}}(\tilde{s})[k]$$
2. Form the matched-filter cross-spectrum:
   $$X[k] = Y[k] \cdot S^*[k]$$
3. Apply the single-sided frequency-domain Hilbert analytic multiplier $H[k]$:
   $$H[k] = \begin{cases} 1 & k = 0 \text{ or } k = \frac{N_{\text{fft}}}{2} \\ 2 & 1 \le k < \frac{N_{\text{fft}}}{2} \\ 0 & k > \frac{N_{\text{fft}}}{2} \end{cases}$$
4. Compute the inverse FFT and take the complex magnitude of the analytic signal $z[m] = R_{ys}[m] + j\,\mathcal{H}\{R_{ys}\}[m]$:
   $$E[m] = \left|\text{IFFT}_{N_{\text{fft}}}\big(X[k] \cdot H[k]\big)[m]\right| = \sqrt{R_{ys}[m]^2 + \mathcal{H}\{R_{ys}\}[m]^2}$$

### 5.3 Proof of Phase Invariance
If the acoustic channel rotates the carrier phase by an arbitrary angle $\phi \in [0, 2\pi)$, the positive-frequency spectrum becomes $Y_\phi[k] = Y[k] e^{j\phi}$, which rotates the complex analytic correlation phasor to $z_\phi[m] = z[m] e^{j\phi}$. Since $|e^{j\phi}| = 1$:

$$E_\phi[m] = |z[m] \, e^{j\phi}| = |z[m]| = E[m] \qquad \forall \phi \in [0, 2\pi)$$

The envelope $E[m]$ is therefore **mathematically invariant to any carrier phase rotation or polarity inversion** and completely free of carrier ripples.

---

## 6. Direct-Wavefront Discrimination & Sub-Sample Parabolic Interpolation

### 6.1 Late Room-Reflection Rejection (Precedence / Haas Effect)
In indoor environments, a reflection off a desk or wall arriving $2\text{–}15\text{ ms}$ after the direct line-of-sight acoustic wave can exhibit a higher peak amplitude than the direct arrival due to constructive multipath interference.

[`AudioEngineService._detect_first_arrival_peak`](../services/audio_engine.py) first locates the maximum $m_{\max}$ within the bounded physical window $[w_{\text{start},i}, w_{\text{end},i}]$ (strictly upper-bounded by the next speaker's emission timestamp $t_{\text{emit,rel},i+1}$ to prevent inter-probe cross-contamination) and then inspects the preceding interval $[m_{\max} - 15\text{ ms}, m_{\max} - 2\text{ ms}]$:
- If an earlier peak $m_{\text{early}}$ satisfies:
  $$E[m_{\text{early}}] \ge \max\big(3.5 \, \mu_{\text{noise}}, \; 0.65 \, E[m_{\max}]\big)$$
- and is separated from $m_{\max}$ by a distinct valley:
  $$\min_{m \in [m_{\text{early}}, m_{\max}]} E[m] \le 0.55 \, E[m_{\text{early}}]$$
the detector locks onto $m_0 = m_{\text{early}}$ (the true direct-path wavefront) and rejects the late room reflection.

### 6.2 3-Point Parabolic Sub-Sample Refinement ($< 0.02\text{ ms}$)
Around the discrete peak index $m_0$, a parabola is fitted through $(m_0 - 1, E[m_0 - 1])$, $(m_0, E[m_0])$, and $(m_0 + 1, E[m_0 + 1])$. The continuous sub-sample vertex offset is:

$$m_{\text{exact}} = m_0 + \frac{1}{2} \frac{E[m_0 - 1] - E[m_0 + 1]}{E[m_0 - 1] - 2 E[m_0] + E[m_0 + 1]}$$

At $f_s = 48000\text{ Hz}$ ($1\text{ sample} = 20.83\,\mu\text{s}$), parabolic refinement achieves a temporal resolution better than $0.02\text{ ms}$.

### 6.3 Deduction of the Calibration Offset $\delta_i$
From $m_{\text{exact},i}$, the measured arrival latency relative to probe emission is:

$$\tau_{\text{meas},i} = \left(\frac{m_{\text{exact},i}}{f_s} - t_{\text{emit,rel},i}\right) \times 1000\text{ [ms]}$$

Subtracting the active `pw-loopback` delay $D_i$ yields the speaker's intrinsic total latency:

$$\tau_{\text{intr},i} = \max(0, \; \tau_{\text{meas},i} - D_i) = \tau_{\text{hw},i} + \tau_{\text{dsp},i} + \tau_{\text{air},i}$$

Finally, subtracting the Level 1 hardware telemetry delta already compensated relative to the reference speaker $\text{ref}$:

$$\Delta\tau_{\text{res},i} = (\tau_{\text{intr},i} - \tau_{\text{intr,ref}}) - (\tau_{\text{hw},i} - \tau_{\text{hw,ref}}) = (\tau_{\text{dsp},i} - \tau_{\text{dsp,ref}}) + (\tau_{\text{air},i} - \tau_{\text{air,ref}})$$

Polifonia assigns:

$$\delta_i = \begin{cases} 0.0\text{ ms} & \text{if } |\Delta\tau_{\text{res},i}| \le 0.3\text{ ms (below human binaural threshold } \approx 10\text{ cm)} \\ -\text{round}(\Delta\tau_{\text{res},i}, 1) & \text{otherwise} \end{cases}$$

---

## 7. Audiophile DSP Pipeline, LR4 Crossover & Phase Alignment

In [`backend/pipewire_config.py`](../backend/pipewire_config.py) and [`services/audio_engine.py`](../services/audio_engine.py):
1. **Audiophile Resampling (`resample.quality = 10`)**: All `pw-loopback` and `filter-chain` streams run at PipeWire's highest Kaiser-windowed sinc interpolation quality (64+ taps, $>100\text{ dB}$ stopband aliasing rejection).
2. **Headroom Normalization (`channelmix.normalize = true`, `channelmix.upmix = true`)**: Prevents $+6\text{ dB}$ digital clipping when summing stereo channels into mono `CENTER` and `SUBWOOFER` feeds.
3. **4th-Order Linkwitz-Riley ($24\text{ dB/oct}$, LR4) Active Crossover**:
   Constructed by cascading two identical 2nd-order Butterworth biquad filters (`bq_lowpass` $\times 2$ on the Subwoofer branch, `bq_highpass` $\times 2$ on satellite branches). At the crossover frequency $f_c$:
   - Both branches have a magnitude response of $-6.02\text{ dB}$ ($0.5$ linear gain).
   - Both branches are completely in phase ($0^\circ / 360^\circ$), guaranteeing a flat $0\text{ dB}$ acoustic sum ($0.5 + 0.5 = 1.0$) with zero phase cancellation around $f_c$.
4. **$180^\circ$ Polarity Inversion (`Ø`) & Multichannel Spatial Routing**: Each speaker supports polarity inversion (`phase_inverted`) and spatial role routing for `STEREO`, `LEFT` (`FL`), `RIGHT` (`FR`), `CENTER` (`FC`), `SUBWOOFER` (`LFE`), `SURROUND_LEFT` (`RL`), and `SURROUND_RIGHT` (`RR`).
