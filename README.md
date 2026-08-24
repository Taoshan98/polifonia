# Taoshan Audio Studio 🎵

Applicazione moderna per Linux (GNOME/Wayland/PipeWire) per creare impianti audio multicanale/surround/2.1 combinando uscite audio indipendenti (es. altoparlanti integrati nei monitor HDMI/DP, uscite Aux, dock USB) escludendo quelle non desiderate.

## ✨ Caratteristiche

* **Routing Dinamico Multi-Sink**: Unione di più uscite audio in un unico **Virtual Sink** PipeWire ad alta fedeltà.
* **Crossover DSP Attivo (2.1)**:
  * Filtro passa-alto (High-pass) per i monitor/satelliti.
  * Filtro passa-basso (Low-pass) per monitor con cassa aux/subwoofer.
  * Frequenza di taglio regolabile in tempo reale (40Hz - 250Hz).
* **Allineamento Temporale (Delay)**: Correzione del ritardo in millisecondi per ciascun altoparlante (fino a 100ms) per compensare differenze di buffer video/audio HDMI o distanze fisiche.
* **Calibrazione Guadagno & Fase**: Volume per canale, mute rapido e inversione di fase (0°/180°).
* **Test Acustico Integrato**: Generatore di impulsi/click e rumore rosa per allineare fase, delay e volumi ad orecchio.
* **Interfaccia GTK4 / Libadwaita**: Design moderno in linea con GNOME, supporto nativo Dark Mode.

## 🚀 Avvio rapido

```bash
cd /home/ntm/Develop/Taoshan
python3 main.py
```

## 🧪 Esecuzione Test

```bash
PYTHONPATH=. python3 -m unittest discover tests
```
