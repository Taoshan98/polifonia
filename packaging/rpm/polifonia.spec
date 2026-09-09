Name:           polifonia
Version:        1.0.5
Release:        1%{?dist}
Summary:        Multi-Speaker & Multi-Monitor Audio Unison Engine for Linux

License:        MIT
URL:            https://github.com/Taoshan98/polifonia
Source0:        %{name}-%{version}.tar.gz

BuildArch:      noarch
BuildRequires:  python3-devel
BuildRequires:  python3-setuptools
Requires:       python3-gobject
Requires:       gtk4
Requires:       libadwaita
Requires:       pipewire
Requires:       pipewire-pulseaudio
Requires:       pipewire-utils
Requires:       pulseaudio-utils

%description
Polifonia allows broadcasting synchronized audio to multiple
monitor displays, USB sound cards, and speakers simultaneously via PipeWire.

%prep
%autosetup

%build
%pyproject_wheel

%install
%pyproject_install
install -Dm644 io.github.taoshan98.Polifonia.desktop %{buildroot}%{_datadir}/applications/io.github.taoshan98.Polifonia.desktop
install -Dm644 assets/io.github.taoshan98.Polifonia.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/io.github.taoshan98.Polifonia.svg
install -Dm644 assets/io.github.taoshan98.Polifonia-symbolic.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/io.github.taoshan98.Polifonia-symbolic.svg
install -Dm644 io.github.taoshan98.Polifonia.metainfo.xml %{buildroot}%{_datadir}/metainfo/io.github.taoshan98.Polifonia.metainfo.xml

%files
%license LICENSE
%doc README.md
%{_bindir}/polifonia
%{python3_sitelib}/polifonia*
%{_datadir}/applications/io.github.taoshan98.Polifonia.desktop
%{_datadir}/icons/hicolor/scalable/apps/io.github.taoshan98.Polifonia*.svg
%{_datadir}/metainfo/io.github.taoshan98.Polifonia.metainfo.xml

%changelog
* Mon Aug 24 2026 Polifonia Contributors <info@polifonia.io> - 1.0.0-1
- Initial Open Source release
