#!/usr/bin/env bash
# Builds QDP's RPM from a built qdp command:
#
#   packaging/linux/build-rpm.sh target/release/qdp RELEASE OUTPUT.rpm
#
# RELEASE is the package's release number, such as the CI build's; the
# version is the workspace's. Needs rpmbuild: the rpm package on Debian and
# Ubuntu, rpm-build on Fedora.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $0 QDP_BINARY RELEASE OUTPUT.rpm" >&2
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
cp "$binary" "$work/source/qdp"
cp "$here/60-qdp.rules" "$here/qdp-modules.conf" "$root/LICENSE" "$root/README.md" "$work/source/"

rpmbuild -bb "$here/qdp.spec" \
  --define "_topdir $work/rpmbuild" \
  --define "qdp_source $work/source" \
  --define "qdp_version $version" \
  --define "qdp_release $release"
cp "$work"/rpmbuild/RPMS/x86_64/qdp-*.rpm "$output"
echo "built $output"
