#!/usr/bin/env bash
# Builds the QDP gateway's RPM from a built qdp-gateway:
#
#   packaging/linux/build-gateway-rpm.sh target/release/qdp-gateway RELEASE OUTPUT.rpm
#
# RELEASE is the package's release number, such as the CI build's; the
# version is the workspace's. Needs rpmbuild: the rpm package on Debian and
# Ubuntu, rpm-build on Fedora.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $0 QDP_GATEWAY_BINARY RELEASE OUTPUT.rpm" >&2
  exit 2
fi
binary=$1
release=$2
output=$3

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
mkdir -p "$work/source"
cp "$binary" "$work/source/qdp-gateway"
cp "$here/qdp-gateway.service" "$here/qdp-gateway.sysusers" "$root/packaging/gateway.toml" \
  "$root/LICENSE" "$work/source/"

rpmbuild -bb "$here/qdp-gateway.spec" \
  --define "_topdir $work/rpmbuild" \
  --define "gateway_source $work/source" \
  --define "gateway_version $version" \
  --define "gateway_release $release"
cp "$work"/rpmbuild/RPMS/x86_64/qdp-gateway-*.rpm "$output"
echo "built $output"
