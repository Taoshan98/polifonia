Name:           polifonia
Version:        1.0.0
Release:        1%{?dist}
Summary:        Multi-Speaker & Multi-Monitor Audio Unison Engine for Linux

License:        MIT
URL:            https://github.com/taoshan/polifonia
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
Polifonia Audio Studio allows broadcasting synchronized audio to multiple
monitor displays, USB sound cards, and speakers simultaneously via PipeWire.

%prep
%autosetup

%build
%pyproject_wheel

%install
%pyproject_install
install -Dm644 io.polifonia.AudioStudio.desktop %{buildroot}%{_datadir}/applications/io.polifonia.AudioStudio.desktop
install -Dm644 assets/io.polifonia.AudioStudio.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/io.polifonia.AudioStudio.svg

%files
%license LICENSE
%doc README.md
%{_bindir}/polifonia
%{python3_sitelib}/polifonia*
%{_datadir}/applications/io.polifonia.AudioStudio.desktop
%{_datadir}/icons/hicolor/scalable/apps/io.polifonia.AudioStudio.svg

%changelog
* Mon Aug 24 2026 Polifonia Contributors <info@polifonia.io> - 1.0.0-1
- Initial Open Source release
