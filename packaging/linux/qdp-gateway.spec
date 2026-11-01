# The QDP gateway package: qdp-gateway, which signs users in with an
# organization's directory and TOTP and tunnels their sessions to the hosts
# they may reach, as a system service running as its own user.
# build-gateway-rpm.sh builds it from a built qdp-gateway, defining
# gateway_source, gateway_version and gateway_release.

# The program is built already, by cargo.
%global debug_package %{nil}
%global __os_install_post %{nil}
%global _build_id_links none

Name:           qdp-gateway
Version:        %{gateway_version}
Release:        %{gateway_release}%{?dist}
Summary:        QDP gateway: reach an organization's QDP hosts with directory sign-in and TOTP
License:        Apache-2.0
URL:            https://github.com/ianmarmour/qdp
Packager:       Ian Armour <ianmarmour@gmail.com>
Vendor:         Ian Armour
BuildArch:      x86_64
Requires(pre):  shadow-utils

%description
The QDP gateway lets users outside an organization's network reach the QDP
hosts inside it through one public endpoint. It signs each user in with the
organization's directory, LDAP or Active Directory, then a TOTP code from
their authenticator app, and tunnels their QDP sessions, end to end, to the
hosts its policy lets them reach.

Its qdp-gateway service runs as the user qdp-gateway, and is off until
/etc/qdp-gateway/gateway.toml is configured and it is turned on with
systemctl enable --now qdp-gateway.

%pre
getent group qdp-gateway >/dev/null || groupadd -r qdp-gateway
getent passwd qdp-gateway >/dev/null || \
  useradd -r -g qdp-gateway -d /var/lib/qdp-gateway -s /sbin/nologin \
  -c "QDP gateway" qdp-gateway
exit 0

%install
install -Dm0755 %{gateway_source}/qdp-gateway %{buildroot}%{_bindir}/qdp-gateway
install -Dm0644 %{gateway_source}/qdp-gateway.service %{buildroot}%{_prefix}/lib/systemd/system/qdp-gateway.service
install -Dm0644 %{gateway_source}/qdp-gateway.sysusers %{buildroot}%{_prefix}/lib/sysusers.d/qdp-gateway.conf
install -Dm0640 %{gateway_source}/gateway.toml %{buildroot}%{_sysconfdir}/qdp-gateway/gateway.toml
install -Dm0644 %{gateway_source}/LICENSE %{buildroot}%{_datadir}/licenses/qdp-gateway/LICENSE

%post
# A new service is off, or as the system's presets say; the package does not
# turn it on, since its configuration is to be written first.
if [ "$1" -eq 1 ]; then
  systemctl preset qdp-gateway.service >/dev/null 2>&1 || :
fi

%preun
# Removing the package stops the service and turns it off.
if [ "$1" -eq 0 ]; then
  systemctl --no-reload disable --now qdp-gateway.service >/dev/null 2>&1 || :
fi

%postun
systemctl daemon-reload >/dev/null 2>&1 || :
# Upgrading restarts a running service with the new program.
if [ "$1" -ge 1 ]; then
  systemctl try-restart qdp-gateway.service >/dev/null 2>&1 || :
fi

%files
%{_bindir}/qdp-gateway
%{_prefix}/lib/systemd/system/qdp-gateway.service
%{_prefix}/lib/sysusers.d/qdp-gateway.conf
%dir %attr(0750,root,qdp-gateway) %{_sysconfdir}/qdp-gateway
%config(noreplace) %attr(0640,root,qdp-gateway) %{_sysconfdir}/qdp-gateway/gateway.toml
%license %{_datadir}/licenses/qdp-gateway/LICENSE
