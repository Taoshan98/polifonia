Name:           polifonia
Version:        2.0.0
Release:        1%{?dist}
Summary:        Multi-Speaker & Multi-Monitor Audio Unison Engine for Linux

License:        MIT
URL:            https://github.com/Taoshan98/polifonia
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  pkgconfig(dbus-1)
Requires:       dbus-libs
Requires:       pipewire
Requires:       pipewire-pulseaudio
Requires:       pipewire-utils
Requires:       wireplumber
Requires:       pulseaudio-utils

%description
Polifonia Audio Studio is a native Rust multi-speaker audio compositing,
synchronization, and acoustic DSP studio for PipeWire that broadcasts
phase-aligned audio simultaneously across monitor speakers, USB DACs,
S/PDIF receivers, Bluetooth A2DP, and network endpoints.

%prep
%autosetup

%build
cargo build --release --locked

%install
install -Dm755 target/release/polifonia %{buildroot}%{_bindir}/polifonia
install -Dm644 io.github.taoshan98.Polifonia.desktop %{buildroot}%{_datadir}/applications/io.github.taoshan98.Polifonia.desktop
install -Dm644 assets/io.github.taoshan98.Polifonia.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/io.github.taoshan98.Polifonia.svg
install -Dm644 assets/io.github.taoshan98.Polifonia-symbolic.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/io.github.taoshan98.Polifonia-symbolic.svg
install -Dm644 assets/io.github.taoshan98.Polifonia.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/polifonia-studio-color.svg
install -Dm644 io.github.taoshan98.Polifonia.metainfo.xml %{buildroot}%{_datadir}/metainfo/io.github.taoshan98.Polifonia.metainfo.xml

%files
%license LICENSE
%doc README.md
%{_bindir}/polifonia
%{_datadir}/applications/io.github.taoshan98.Polifonia.desktop
%{_datadir}/icons/hicolor/scalable/apps/*.svg
%{_datadir}/metainfo/io.github.taoshan98.Polifonia.metainfo.xml

%changelog
* Sun Nov 15 2026 Polifonia Contributors <info@polifonia.io> - 2.0.0-1
- Rewrote Polifonia v2.0 in native Rust (eframe/egui) with Matte Carbon & Studio Copper DAW Console UI and split-process daemon IPC
* Mon Aug 24 2026 Polifonia Contributors <info@polifonia.io> - 1.0.0-1
- Initial Open Source release
