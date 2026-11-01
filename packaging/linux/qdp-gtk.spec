# The QDP app for Linux desktops: qdp-gtk, in GTK 4 and libadwaita, with its
# desktop entry, icons and AppStream metadata, so that it shows in GNOME's,
# KDE Plasma's and other desktops' menus and software centers. It streams
# with the qdp command, from the qdp package. build-gtk-rpm.sh builds it from
# a built qdp-gtk, defining app_source, app_version and app_release.

# The program is built already, by cargo.
%global debug_package %{nil}
%global __os_install_post %{nil}
%global _build_id_links none

Name:           qdp-gtk
Version:        %{app_version}
Release:        %{app_release}%{?dist}
Summary:        QDP for Linux desktops: stream the desktops of your computers
License:        Apache-2.0
URL:            https://github.com/ianmarmour/qdp
Packager:       Ian Armour <ianmarmour@gmail.com>
Vendor:         Ian Armour
BuildArch:      x86_64
# The app runs qdp connect for each stream.
Requires:       qdp = %{version}
Requires:       gtk4 >= 4.14
Requires:       libadwaita >= 1.5

%description
QDP streams the desktops of your computers to this one over the network, with
low latency. The app lists the computers on your network and those you paired
with: choose a new one to pair with the code it shows, and a paired one to
stream from at once, in a window of its own.

%install
install -Dm0755 %{app_source}/qdp-gtk %{buildroot}%{_bindir}/qdp-gtk
cp -r %{app_source}/share %{buildroot}%{_prefix}/

%files
%{_bindir}/qdp-gtk
%{_datadir}/applications/io.github.ianmarmour.qdp.desktop
%{_datadir}/metainfo/io.github.ianmarmour.qdp.metainfo.xml
%{_datadir}/icons/hicolor/*/apps/io.github.ianmarmour.qdp.*
%license %{_datadir}/licenses/qdp-gtk/LICENSE
