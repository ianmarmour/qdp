#!/usr/bin/env bash
# Lays out the QDP app for Linux desktops as it installs, under a prefix:
#
#   packaging/linux/stage-gtk.sh target/release/qdp-gtk DIRECTORY
#
# DIRECTORY gets bin/qdp-gtk and share/, with the desktop entry, icons,
# AppStream metadata and license, for the RPM and the tarball; extracting the
# tarball into ~/.local installs the app for one user.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 QDP_GTK_BINARY DIRECTORY" >&2
  exit 2
fi
binary=$1
stage=$2

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
data="$root/crates/qdp-gtk/data"
id=io.github.ianmarmour.qdp

install -Dm0755 "$binary" "$stage/bin/qdp-gtk"
install -Dm0644 "$data/$id.desktop" "$stage/share/applications/$id.desktop"
install -Dm0644 "$data/$id.metainfo.xml" "$stage/share/metainfo/$id.metainfo.xml"
(cd "$data/icons" && find hicolor -type f) | while read -r icon; do
  install -Dm0644 "$data/icons/$icon" "$stage/share/icons/$icon"
done
install -Dm0644 "$root/LICENSE" "$stage/share/licenses/qdp-gtk/LICENSE"
