#!/usr/bin/env bash
# Builds the RPM of the QDP app for Linux desktops from a built qdp-gtk:
#
#   packaging/linux/build-gtk-rpm.sh target/release/qdp-gtk RELEASE OUTPUT.rpm
#
# RELEASE is the package's release number, such as the CI build's; the
# version is the workspace's. Needs rpmbuild: the rpm package on Debian and
# Ubuntu, rpm-build on Fedora.
set -euo pipefail

if [ "$#" -ne 3 ]; then
  echo "usage: $0 QDP_GTK_BINARY RELEASE OUTPUT.rpm" >&2
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
"$here/stage-gtk.sh" "$binary" "$work/stage"
cp "$work/stage/bin/qdp-gtk" "$work/source/"
cp -r "$work/stage/share" "$work/source/"

rpmbuild -bb "$here/qdp-gtk.spec" \
  --define "_topdir $work/rpmbuild" \
  --define "app_source $work/source" \
  --define "app_version $version" \
  --define "app_release $release"
cp "$work"/rpmbuild/RPMS/x86_64/qdp-gtk-*.rpm "$output"
echo "built $output"
