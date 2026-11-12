#!/usr/bin/env bash
# Builds the QDP app for Linux desktops, with the qdp command it runs and
# the libraries both need, as an AppImage: one file that runs without
# installing on most Linux systems from 2024 on, the Steam Deck's included.
#
#   packaging/linux/build-appimage.sh target/release/qdp-gtk target/release/qdp APPDIR [RUNTIME OUTPUT.AppImage]
#
# APPDIR gets the AppImage's contents, which its AppRun runs as they are.
# Given the AppImage runtime, type 2, the script packs them after it into
# OUTPUT. The libraries are this system's, so the AppImage needs a C library
# at least as new as the one it was built with. The system's graphics
# drivers, display server, fonts, sound and C library are left out, as
# AppImages leave them, so that the app uses the computer's own. Needs ldd,
# patchelf, gdk-pixbuf-query-loaders, glib-compile-schemas and mksquashfs.
set -euo pipefail

if [ "$#" -ne 3 ] && [ "$#" -ne 5 ]; then
  echo "usage: $0 QDP_GTK_BINARY QDP_BINARY APPDIR [RUNTIME OUTPUT.AppImage]" >&2
  exit 2
fi
app=$1
qdp=$2
appdir=$3
here=$(cd "$(dirname "$0")" && pwd)
id=io.github.ianmarmour.qdp
multiarch=$(gcc -dumpmachine 2>/dev/null || echo x86_64-linux-gnu)
system_lib=/usr/lib/$multiarch
pixbuf=gdk-pixbuf-2.0/2.10.0

rm -rf "$appdir"
"$here/stage-gtk.sh" "$app" "$appdir/usr"
install -Dm0755 "$qdp" "$appdir/usr/bin/qdp"
lib=$appdir/usr/lib
mkdir -p "$lib/$pixbuf" "$lib/gio/modules"

# Libraries that come from the system the AppImage runs on: the C library,
# graphics and display, fonts, sound and devices, whose own copies match
# the computer's drivers and services.
excluded='^(ld-linux.*|libc|libm|libdl|libpthread|librt|libresolv|libutil|libanl|libgcc_s|libstdc\+\+|libGL|libGLX|libGLdispatch|libOpenGL|libEGL|libGLESv2|libgbm|libdrm|libvulkan|libva|libva-drm|libva-x11|libva-wayland|libX11|libX11-xcb|libxcb|libxcb-.*|libxshmfence|libfontconfig|libfreetype|libharfbuzz|libexpat|libz|libasound|libpipewire-0\.3|libudev|libsystemd|libdbus-1)\.so'

# Copies the libraries that the ELF files given need, and theirs, that the
# system does not provide, into $lib.
bundle() {
  local pending=("$@") file dependency name
  while [ "${#pending[@]}" -gt 0 ]; do
    file=${pending[0]}
    pending=("${pending[@]:1}")
    while read -r dependency; do
      name=$(basename "$dependency")
      if [[ "$name" =~ $excluded ]] || [ -e "$lib/$name" ]; then
        continue
      fi
      install -m0644 "$(readlink -f "$dependency")" "$lib/$name"
      pending+=("$lib/$name")
    done < <(ldd "$file" | awk '/=> \// { print $3 }')
  done
}

# GTK's SVG image loader, for its icons, beside the other libraries, where
# GLib finds the name the loaders' cache gives it; and GTK's settings
# schemas.
install -m0644 "$system_lib/$pixbuf/loaders/libpixbufloader-svg.so" "$lib/"
install -Dm0644 -t "$appdir/usr/share/glib-2.0/schemas" /usr/share/glib-2.0/schemas/org.gtk.gtk4.Settings.*.gschema.xml
glib-compile-schemas "$appdir/usr/share/glib-2.0/schemas"
# The symbolic icons GTK and libadwaita draw, without the cursors, which
# come from the system.
mkdir -p "$appdir/usr/share/icons/Adwaita"
cp -r /usr/share/icons/Adwaita/{index.theme,symbolic,scalable,16x16} "$appdir/usr/share/icons/Adwaita/"

bundle "$appdir/usr/bin/qdp-gtk" "$appdir/usr/bin/qdp" "$lib/libpixbufloader-svg.so"

# Each binary looks for its libraries in the AppImage first.
patchelf --set-rpath '$ORIGIN/../lib' "$appdir/usr/bin/qdp-gtk" "$appdir/usr/bin/qdp"
for library in "$lib"/*.so*; do
  patchelf --set-rpath '$ORIGIN' "$library"
done
# The loaders' cache names the loader without its directory, as the
# AppImage is mounted somewhere new each time.
query=$(command -v gdk-pixbuf-query-loaders || echo "$system_lib/gdk-pixbuf-2.0/gdk-pixbuf-query-loaders")
"$query" "$lib/libpixbufloader-svg.so" | sed "s|$lib/||" > "$lib/$pixbuf/loaders.cache"

install -m0755 "$here/AppRun" "$appdir/AppRun"
install -m0644 "$appdir/usr/share/applications/$id.desktop" "$appdir/$id.desktop"
install -m0644 "$appdir/usr/share/icons/hicolor/256x256/apps/$id.png" "$appdir/$id.png"
ln -sf "$id.png" "$appdir/.DirIcon"

if [ "$#" -eq 5 ]; then
  runtime=$4
  output=$5
  image=$(mktemp)
  trap 'rm -f "$image"' EXIT
  mksquashfs "$appdir" "$image" -root-owned -noappend -comp zstd -quiet -no-progress
  cat "$runtime" "$image" > "$output"
  chmod 0755 "$output"
fi
