#!/usr/bin/env bash
# Builds the Debian package of one of QDP's servers, for Debian and Ubuntu,
# from its built program, with the same files as its RPM:
#
#   packaging/linux/build-deb.sh qdp target/release/qdp RELEASE OUTPUT.deb
#   packaging/linux/build-deb.sh qdp-gateway target/release/qdp-gateway RELEASE OUTPUT.deb
#
# RELEASE is the package's release number, such as the CI build's; the
# version is the workspace's. The package depends on the packages that have
# the libraries the program links, as rpmbuild finds an RPM's, and on the
# oldest C library its symbols allow. Its control file and maintainer scripts
# are in deb/PACKAGE beside this script. Needs dpkg-deb and objdump, as
# Debian and Ubuntu have them.
set -euo pipefail

if [ "$#" -ne 4 ]; then
  echo "usage: $0 qdp|qdp-gateway BINARY RELEASE OUTPUT.deb" >&2
  exit 2
fi
package=$1
binary=$2
release=$3
output=$4

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
version=$(sed -n 's/^version = "\(.*\)"/\1/p' "$root/Cargo.toml" | head -n 1)
control=$here/deb/$package
if [ ! -f "$control/control" ]; then
  echo "$0: no package $package in $here/deb" >&2
  exit 2
fi

# Prints the packages the program needs, comma-separated, failing on a
# library it does not know the package of.
depends() {
  # Weak references, which Rust's standard library makes to newer C library
  # functions it uses only where they exist, do not count.
  local glibc
  glibc=$(objdump -T "$1" | awk '$2 != "w" && /\*UND\*/' | grep -o 'GLIBC_[0-9.]*' \
    | sed 's/^GLIBC_//' | sort -Vu | tail -n 1)
  local needs=("libc6${glibc:+ (>= $glibc)}")
  local library
  for library in $(objdump -p "$1" | awk '$1 == "NEEDED" { print $2 }'); do
    case "$library" in
      libc.so.6 | libm.so.6 | libdl.so.2 | libpthread.so.0 | librt.so.1 | ld-linux-x86-64.so.2) ;;
      libgcc_s.so.1) needs+=("libgcc-s1") ;;
      libudev.so.1) needs+=("libudev1") ;;
      # Debian 13 and Ubuntu 24.04's libpipewire-0.3-0t64 provides it too.
      libpipewire-0.3.so.0) needs+=("libpipewire-0.3-0") ;;
      *)
        echo "$0: $1 links $library: add the Debian package that has it" >&2
        return 1
        ;;
    esac
  done
  local IFS=,
  echo "${needs[*]}" | sed 's/,/, /g'
}

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
stage=$work/$package
case "$package" in
  qdp)
    install -Dm0755 "$binary" "$stage/usr/bin/qdp"
    install -Dm0644 "$here/60-qdp.rules" "$stage/usr/lib/udev/rules.d/60-qdp.rules"
    install -Dm0644 "$here/qdp-modules.conf" "$stage/usr/lib/modules-load.d/qdp.conf"
    install -Dm0644 "$here/qdp.service" "$stage/usr/lib/systemd/system/qdp.service"
    install -Dm0644 "$root/README.md" "$stage/usr/share/doc/qdp/README.md"
    ;;
  qdp-gateway)
    install -Dm0755 "$binary" "$stage/usr/bin/qdp-gateway"
    install -Dm0644 "$here/qdp-gateway.service" "$stage/usr/lib/systemd/system/qdp-gateway.service"
    install -Dm0644 "$here/qdp-gateway.sysusers" "$stage/usr/lib/sysusers.d/qdp-gateway.conf"
    # Readable by the gateway's group once the package is configured.
    install -Dm0640 "$root/packaging/gateway.toml" "$stage/etc/qdp-gateway/gateway.toml"
    chmod 0750 "$stage/etc/qdp-gateway"
    ;;
esac
# Debian keeps a package's license as its copyright file.
install -Dm0644 "$root/LICENSE" "$stage/usr/share/doc/$package/copyright"

mkdir -p "$stage/DEBIAN"
(cd "$stage" && find . -type f ! -path './DEBIAN/*' -printf '%P\0' | sort -z | xargs -0 md5sum) \
  > "$stage/DEBIAN/md5sums"
size=$(du -sk --exclude=DEBIAN "$stage" | cut -f1)
needs=$(depends "$binary")
sed -e "s/@VERSION@/$version-$release/" -e "s/@INSTALLED_SIZE@/$size/" -e "s/@DEPENDS@/$needs/" \
  "$control/control" > "$stage/DEBIAN/control"
for script in preinst postinst prerm postrm; do
  if [ -f "$control/$script" ]; then
    install -m0755 "$control/$script" "$stage/DEBIAN/$script"
  fi
done
if [ -f "$control/conffiles" ]; then
  install -m0644 "$control/conffiles" "$stage/DEBIAN/conffiles"
fi

# xz, which every dpkg reads, rather than Ubuntu's zstd, which older
# Debian's does not.
dpkg-deb --root-owner-group -Zxz --build "$stage" "$output" >/dev/null
echo "built $output"
