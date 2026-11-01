# The QDP package: the qdp command, which serves this desktop and connects to
# others, with what lets it play a client's input. build-rpm.sh builds it from
# a built qdp command, defining qdp_source, qdp_version and qdp_release.

# The command is built already, by cargo.
%global debug_package %{nil}
%global __os_install_post %{nil}
%global _build_id_links none

Name:           qdp
Version:        %{qdp_version}
Release:        %{qdp_release}%{?dist}
Summary:        Quick Desktop Protocol: stream desktops over the network
License:        Apache-2.0
URL:            https://github.com/ianmarmour/qdp
Packager:       Ian Armour <ianmarmour@gmail.com>
Vendor:         Ian Armour
BuildArch:      x86_64
# Hardware encoding and decoding go through VA-API, which qdp loads when the
# system has it.
Recommends:     libva.so.2()(64bit)
Recommends:     libva-drm.so.2()(64bit)

%description
QDP streams a computer's desktop to other devices, and shows other computers'
desktops, over QUIC with AV1 video. The qdp command is both the host, qdp
serve, and a client, qdp connect.

The package lets the user at the machine create the virtual input devices the
host plays a client's keyboard, mouse, touch, pens and passed-through devices
through, and loads the kernel modules for them at boot.

%install
install -Dm0755 %{qdp_source}/qdp %{buildroot}%{_bindir}/qdp
install -Dm0644 %{qdp_source}/60-qdp.rules %{buildroot}%{_prefix}/lib/udev/rules.d/60-qdp.rules
install -Dm0644 %{qdp_source}/qdp-modules.conf %{buildroot}%{_prefix}/lib/modules-load.d/qdp.conf
install -Dm0644 %{qdp_source}/LICENSE %{buildroot}%{_datadir}/licenses/qdp/LICENSE
install -Dm0644 %{qdp_source}/README.md %{buildroot}%{_docdir}/qdp/README.md

%post
# The modules load and the rule applies now, so that input works before the
# next boot.
modprobe -a uinput uhid >/dev/null 2>&1 || :
udevadm control --reload-rules >/dev/null 2>&1 || :
udevadm trigger --subsystem-match=misc >/dev/null 2>&1 || :

%files
%{_bindir}/qdp
%{_prefix}/lib/udev/rules.d/60-qdp.rules
%{_prefix}/lib/modules-load.d/qdp.conf
%license %{_datadir}/licenses/qdp/LICENSE
%doc %{_docdir}/qdp/README.md
