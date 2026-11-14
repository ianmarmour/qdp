#!/usr/bin/env bash
# Tries QDP's Arch packages in Arch's own image: they install with their
# dependencies from Arch's repositories, match their file lists, leave their
# services off, give the gateway's configuration to its group alone, and
# remove.
#
#   packaging/linux/try-arch.sh QDP.pkg.tar.zst QDP_GATEWAY.pkg.tar.zst
#
# Needs Docker.
set -euo pipefail

if [ "$#" -ne 2 ]; then
  echo "usage: $0 QDP.pkg.tar.zst QDP_GATEWAY.pkg.tar.zst" >&2
  exit 2
fi
qdp=$(realpath "$1")
gateway=$(realpath "$2")

docker run --rm \
  -v "$qdp:/packages/qdp.pkg.tar.zst:ro" \
  -v "$gateway:/packages/qdp-gateway.pkg.tar.zst:ro" \
  archlinux:latest bash -euo pipefail -c '
    pacman -Syu --noconfirm >/dev/null
    # The image leaves documentation out, which the file lists include.
    sed -i "s|^NoExtract|#NoExtract|" /etc/pacman.conf
    pacman -U --noconfirm /packages/qdp.pkg.tar.zst /packages/qdp-gateway.pkg.tar.zst
    qdp help >/dev/null
    qdp-gateway help >/dev/null
    pacman -Qkk qdp
    test "$(systemctl --root=/ is-enabled qdp.service)" = disabled
    test "$(systemctl --root=/ is-enabled qdp-gateway.service)" = disabled
    test "$(stat -c "%U:%G %a" /etc/qdp-gateway/gateway.toml)" = "root:qdp-gateway 640"
    pacman -R --noconfirm qdp qdp-gateway
    test ! -e /usr/bin/qdp && test ! -e /usr/bin/qdp-gateway
    echo "the Arch packages install, run and remove"
  '
