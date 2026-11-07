#!/usr/bin/env bash
set -euo pipefail

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
cd "$project_root"
mkdir -p target/gui-validation/rpms target/gui-validation/local-root

# Download signed Fedora RPMs, then unpack only the executables needed by this
# user's validation session. This does not change the host RPM database.
dnf --disablerepo='*' --enablerepo=fedora --enablerepo=updates \
    download --resolve --destdir=target/gui-validation/rpms sway swaybg wtype
for package in sway swaybg wtype; do
    rpmfile="$(find target/gui-validation/rpms -maxdepth 1 -type f -name "$package-*.rpm" -print -quit)"
    if [[ -z "$rpmfile" ]]; then
        echo "Fedora RPM missing after download: $package" >&2
        exit 2
    fi
    (cd target/gui-validation/local-root && rpm2cpio "$project_root/$rpmfile" | cpio -id --quiet)
done

for executable in sway swaymsg swaybg wtype; do
    test -x "target/gui-validation/local-root/usr/bin/$executable"
done
if ldd target/gui-validation/local-root/usr/bin/sway target/gui-validation/local-root/usr/bin/swaybg target/gui-validation/local-root/usr/bin/wtype \
    | rg 'not found'; then
    echo 'A required shared library is missing; use host DNF installation.' >&2
    exit 2
fi
target/gui-validation/local-root/usr/bin/sway --version
echo "User-local tools ready in $project_root/target/gui-validation/local-root/usr/bin"
