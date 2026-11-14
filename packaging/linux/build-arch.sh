#!/usr/bin/env bash
# Builds the Arch Linux package of one of QDP's servers, for Arch, CachyOS
# and the other distributions built on it, from its built program, with the
# same files as its RPM and Debian package:
#
#   packaging/linux/build-arch.sh qdp target/release/qdp RELEASE OUTPUT.pkg.tar.zst
#   packaging/linux/build-arch.sh qdp-gateway target/release/qdp-gateway RELEASE OUTPUT.pkg.tar.zst
#
# RELEASE is the package's release number, such as the CI build's; the
# version is the workspace's. The package depends on the packages that have
# the libraries the program links. It is put together as makepkg puts one
# together, the program stripped, its install script being
# arch/PACKAGE.install beside this script. Needs bsdtar, objdump and strip.
set -euo pipefail

if [ "$#" -ne 4 ]; then
  echo "usage: $0 qdp|qdp-gateway BINARY RELEASE OUTPUT.pkg.tar.zst" >&2
  exit 2
fi
package=$1
binary=$2
release=$3
output=$(realpath -m "$4")

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
if [ ! -f "$here/arch/$package.install" ]; then
  echo "$0: no package $package in $here/arch" >&2
  exit 2
fi

# Prints the packages the program needs, one a line, failing on a library it
# does not know the package of.
depends() {
  echo glibc
  local library
  for library in $(objdump -p "$1" | awk '$1 == "NEEDED" { print $2 }'); do
    case "$library" in
      libc.so.6 | libm.so.6 | libdl.so.2 | libpthread.so.0 | librt.so.1 | ld-linux-x86-64.so.2) ;;
      libgcc_s.so.1) echo libgcc ;;
      libudev.so.1) echo systemd-libs ;;
      libpipewire-0.3.so.0) echo libpipewire ;;
      *)
        echo "$0: $1 links $library: add the Arch package that has it" >&2
        return 1
        ;;
    esac
  done
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
stage=$work/$package
backup=()
optional=()
case "$package" in
  qdp)
    description="Quick Desktop Protocol: stream desktops over the network"
    install -Dm0755 "$binary" "$stage/usr/bin/qdp"
    strip --strip-all "$stage/usr/bin/qdp"
    install -Dm0644 "$here/60-qdp.rules" "$stage/usr/lib/udev/rules.d/60-qdp.rules"
    install -Dm0644 "$here/qdp-modules.conf" "$stage/usr/lib/modules-load.d/qdp.conf"
    install -Dm0644 "$here/qdp.service" "$stage/usr/lib/systemd/system/qdp.service"
    install -Dm0644 "$root/README.md" "$stage/usr/share/doc/qdp/README.md"
    optional=("libva: video encoding and decoding on the GPU")
    ;;
  qdp-gateway)
    description="QDP gateway: reach an organization's QDP hosts with directory sign-in and TOTP"
    install -Dm0755 "$binary" "$stage/usr/bin/qdp-gateway"
    strip --strip-all "$stage/usr/bin/qdp-gateway"
    install -Dm0644 "$here/qdp-gateway.service" "$stage/usr/lib/systemd/system/qdp-gateway.service"
    install -Dm0644 "$here/qdp-gateway.sysusers" "$stage/usr/lib/sysusers.d/qdp-gateway.conf"
    # Readable by the gateway's group once installed, which the install
    # script gives it. pacman keeps the user's changes through upgrades.
    install -Dm0640 "$root/packaging/gateway.toml" "$stage/etc/qdp-gateway/gateway.toml"
    backup=(etc/qdp-gateway/gateway.toml)
    ;;
esac
install -Dm0644 "$root/LICENSE" "$stage/usr/share/licenses/$package/LICENSE"
install -m0644 "$here/arch/$package.install" "$stage/.INSTALL"

{
  echo "pkgname = $package"
  echo "pkgbase = $package"
  echo "xdata = pkgtype=pkg"
  echo "pkgver = $version-$release"
  echo "pkgdesc = $description"
  echo "url = https://github.com/ianmarmour/qdp"
  echo "builddate = ${SOURCE_DATE_EPOCH:-$(date +%s)}"
  echo "packager = Ian Armour <ianmarmour@gmail.com>"
  echo "size = $(du -sb --exclude=.INSTALL "$stage" | cut -f1)"
  echo "arch = x86_64"
  echo "license = Apache-2.0"
  for file in "${backup[@]}"; do
    echo "backup = $file"
  done
  depends "$binary" | sort -u | sed 's/^/depend = /'
  for dependency in "${optional[@]}"; do
    echo "optdepend = $dependency"
  done
} > "$stage/.PKGINFO"

# The package's files, its metadata first, owned by root.
cd "$stage"
files() {
  printf '%s\0' .PKGINFO .INSTALL
  find . -mindepth 1 ! -name .PKGINFO ! -name .INSTALL ! -name .MTREE -printf '%P\0' | LC_ALL=C sort -z
}
as_root=(--uid 0 --gid 0 --uname root --gname root)
files | LANG=C bsdtar "${as_root[@]}" -cnf - --format=mtree \
  --options='!all,use-set,type,uid,gid,mode,time,size,md5,sha256,link' --null --files-from - \
  | gzip -c -f -n > .MTREE
{ printf '%s\0' .MTREE; files; } \
  | LANG=C bsdtar "${as_root[@]}" --no-fflags --zstd --options zstd:compression-level=19 \
    -cnf "$output" --null --files-from -
echo "built $output"
