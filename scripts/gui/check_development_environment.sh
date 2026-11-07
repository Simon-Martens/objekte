#!/usr/bin/env bash
set -u

project_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
echo '== Host =='
sed -n 's/^PRETTY_NAME=//p' /etc/os-release
uname -m
printf 'user=%s uid=%s\n' "$(id -un)" "$(id -u)"
printf 'memory_available_kib='; awk '/MemAvailable:/ { print $2 }' /proc/meminfo
df -h "$project_root" | tail -1
echo '== Toolchain =='
(cd "$project_root" && rustc --version && cargo --version && rustup component list --installed)
echo '== Desktop =='
if command -v sway >/dev/null 2>&1; then
    sway --version
elif [[ -x "$project_root/target/gui-validation/local-root/usr/bin/sway" ]]; then
    "$project_root/target/gui-validation/local-root/usr/bin/sway" --version
else
    echo 'sway=missing; run scripts/gui/install_user_dependencies.sh'
fi
printf 'wtype='; if command -v wtype >/dev/null 2>&1; then rpm -q wtype; elif [[ -x "$project_root/target/gui-validation/local-root/usr/bin/wtype" ]]; then echo user-local; else echo missing; fi
printf 'wayvnc='; if command -v wayvnc >/dev/null 2>&1; then wayvnc --version; else echo missing; fi
printf 'grim='; if command -v grim >/dev/null 2>&1; then rpm -q grim; else echo missing; fi
printf 'vncdo='; if [[ -x "$project_root/target/gui-validation/venv/bin/vncdo" ]]; then "$project_root/target/gui-validation/venv/bin/vncdo" --version; else echo missing; fi
echo '== Vulkan devices =='
vulkaninfo --summary 2>/dev/null | awk '/Devices:/{show=1} show && /deviceType|deviceName|driverName/ {print}'
echo '== Headless Sway session =='
python3 "$project_root/scripts/gui/headless_session.py" status
