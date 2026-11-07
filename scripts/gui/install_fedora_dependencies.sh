#!/usr/bin/env bash
set -euo pipefail

if [[ "$(. /etc/os-release; printf '%s' "$ID")" != fedora ]]; then
    echo 'This installer supports mutable Fedora only.' >&2
    exit 2
fi
if command -v rpm-ostree >/dev/null 2>&1; then
    echo 'rpm-ostree is present; use the host established layering/container workflow.' >&2
    exit 2
fi

# The locked validation build succeeded with existing development libraries.
# WayVNC supplies the working pointer fallback; wtype handles Unicode keys.
packages=(sway swaybg wtype grim jq wayvnc)
missing=()
for package in "${packages[@]}"; do
    if rpm -q "$package" >/dev/null 2>&1; then
        rpm -q "$package"
    else
        missing+=("$package")
    fi
done

case "${1:-check}" in
    check)
        if (( ${#missing[@]} )); then
            printf 'Missing: %s\n' "${missing[*]}"
            echo 'Install only after reviewing the official Fedora package transaction:'
            printf '  sudo dnf install --disablerepo="*" --enablerepo=fedora --enablerepo=updates %s\n' "${missing[*]}"
        else
            echo 'Selected host packages are installed.'
        fi
        ;;
    install)
        if (( EUID != 0 )); then
            echo 'Administrator privileges are needed for host package installation.' >&2
            exit 3
        fi
        if (( ${#missing[@]} )); then
            before="$(mktemp)"
            after="$(mktemp)"
            trap 'unlink "$before"; unlink "$after"' EXIT
            rpm -qa --qf '%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}\n' | sort > "$before"
            dnf install --disablerepo='*' --enablerepo=fedora --enablerepo=updates "${missing[@]}"
            rpm -qa --qf '%{NAME}-%{VERSION}-%{RELEASE}.%{ARCH}\n' | sort > "$after"
            echo 'Newly installed RPMs:'
            comm -13 "$before" "$after"
        fi
        ;;
    *)
        echo 'Usage: install_fedora_dependencies.sh [check|install]' >&2
        exit 2
        ;;
esac
