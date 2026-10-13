#!/bin/bash
# Run as root on a disposable Linux host. Service-manager commands are mocked.
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo 'Run as root on a disposable test host.' >&2; exit 1; }
PROJECT=$(cd "$(dirname "$0")/.." && pwd)
TEST_ROOT=$(mktemp -d)
TEST_NAME=wrapper-install-failure-$$
case "$(uname -m)" in
    x86_64) BINARY=wrapper-linux-x86-64 ;;
    aarch64) BINARY=wrapper-linux-arm-64 ;;
    *) echo 'Unsupported test host' >&2; exit 1 ;;
esac
mkdir -p "$TEST_ROOT"/{boot,bin,conf,mocks}
cp "$PROJECT/dist/boot/$BINARY" "$TEST_ROOT/boot/wrapper"
cp "$PROJECT/resources/App.sh.in" "$TEST_ROOT/bin/service.sh"
printf 'wrapper.name=%s\n' "$TEST_NAME" > "$TEST_ROOT/conf/wrapper.conf"
if [ -d /run/systemd/system ]; then
    REGISTRATION="/etc/systemd/system/$TEST_NAME.service"
    FAILURES=(daemon-reload enable)
else
    REGISTRATION="/etc/init.d/$TEST_NAME"
    FAILURES=(defaults)
fi
[ ! -e "$REGISTRATION" ]
cleanup() { rm -f "$REGISTRATION"; rm -rf "$TEST_ROOT"; }
trap cleanup EXIT
cat > "$TEST_ROOT/mocks/manager" <<'EOF'
#!/bin/bash
set -eu
[ "$1" != --help ] || exit 0
printf '%s\n' "$*" >> "$TEST_ROOT/calls"
case "$*" in
    enable\ *|*\ defaults) touch "$TEST_ROOT/enabled" ;;
    disable\ *|-f\ *\ remove) rm -f "$TEST_ROOT/enabled" ;;
esac
if [[ " $* " == *" $FAIL_REGISTRATION "* ]]; then exit 7; fi
EOF
chmod +x "$TEST_ROOT/mocks/manager"
ln -s manager "$TEST_ROOT/mocks/systemctl"
ln -s manager "$TEST_ROOT/mocks/update-rc.d"
export TEST_ROOT
export PATH="$TEST_ROOT/mocks:$PATH"
invoke() { "$TEST_ROOT/boot/wrapper" install "$TEST_ROOT/conf/wrapper.conf"; }
for failure in "${FAILURES[@]}"; do
    export FAIL_REGISTRATION="$failure"
    if invoke > "$TEST_ROOT/error" 2>&1; then
        echo 'Installation must report the manager failure' >&2; exit 1
    fi
    grep -q 'exit status: 7' "$TEST_ROOT/error"
    [ ! -e "$REGISTRATION" ]
    [ ! -e "$TEST_ROOT/enabled" ]
done
export FAIL_REGISTRATION=none
invoke
[ -f "$REGISTRATION" ]
[ -f "$TEST_ROOT/enabled" ]
echo 'Failed installation cleanup and retry passed'
