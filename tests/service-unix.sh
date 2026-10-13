#!/bin/bash
# Run as root on a disposable Linux/systemd or Apple Silicon/launchd host.
# For a systemd container, mask systemd-binfmt.service in its image before boot;
# otherwise shutdown can clear the host's shared binary-emulation registrations.
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo 'Run as root on a disposable service test host.' >&2; exit 1; }
PROJECT=$(cd "$(dirname "$0")/.." && pwd)
TEST_ROOT=$(mktemp -d '/tmp/Wrapper service test.XXXXXX')
TEST_NAME=wrapper-test-$$
mkdir -p "$TEST_ROOT"/{bin,boot,conf,logs}
cp "$PROJECT"/dist/boot/* "$TEST_ROOT/boot/"
cp "$PROJECT/resources/App.sh.in" "$TEST_ROOT/bin/service.sh"
chmod +x "$TEST_ROOT/bin/service.sh"
case "$(uname -s):$(uname -m)" in
    Darwin:arm64) WRAPPER="$TEST_ROOT/boot/wrapper-macosx-arm-64" ;;
    Linux:x86_64) WRAPPER="$TEST_ROOT/boot/wrapper-linux-x86-64" ;;
    Linux:aarch64) WRAPPER="$TEST_ROOT/boot/wrapper-linux-arm-64" ;;
    *) echo 'Unsupported test host' >&2; exit 1 ;;
esac
CONFIG="$TEST_ROOT/conf/custom config.conf"
echo 'WRAPPER_CONF="../conf/custom config.conf"' > "$TEST_ROOT/bin/service.shconf"
javac --release 17 -cp "$TEST_ROOT/boot/wrapper.jar" -d "$TEST_ROOT/boot" "$PROJECT/tests/Fixture.java"
cat > "$CONFIG" <<EOF
#encoding=UTF-8
wrapper.name=$TEST_NAME
wrapper.java.command=$(command -v java)
wrapper.java.classpath.1=.
wrapper.app.parameter.1=Fixture
wrapper.java.additional.1=-Dfixture.root=$TEST_ROOT
wrapper.java.additional.2=-Dfixture.value=before
wrapper.shutdown.timeout=10
wrapper.restart.delay=0
wrapper.pidfile=../bin/config.pid
EOF
if [ "$(uname -s)" = Linux ] && id nobody >/dev/null 2>&1; then
    echo 'wrapper.daemon.user=nobody' >> "$CONFIG"
    chown -R nobody "$TEST_ROOT"
fi
invoke() {
    "$WRAPPER" "$1" "$CONFIG"
}
start_installed() {
    if [ -d /run/systemd/system ]; then systemctl reset-failed "$TEST_NAME"; fi
    if [ -x "/etc/init.d/$TEST_NAME" ] && [ ! -d /run/systemd/system ]; then
        "/etc/init.d/$TEST_NAME" start
    else
        invoke start
    fi
}
cleanup() {
    if [ -f "$TEST_ROOT/init-backup" ]; then
        cp "$TEST_ROOT/init-backup" "/etc/init.d/$TEST_NAME"
    fi
    if invoke remove; then
        rm -rf "$TEST_ROOT"
    else
        echo "Cleanup failed: inspect $TEST_ROOT and service $TEST_NAME" >&2
    fi
}
trap cleanup EXIT
wait_event() {
    for ((attempt=0; attempt<100; attempt++)); do
        if [ -f "$TEST_ROOT/events" ] && grep -q "$1" "$TEST_ROOT/events"; then return; fi
        sleep 0.2
    done
    echo "Timed out waiting for $1" >&2; exit 1
}
wait_arguments() {
    for ((attempt=0; attempt<100; attempt++)); do
        if [ "$(grep -c '^arg=persisted argument$' "$TEST_ROOT/logs/console.log")" -ge "$1" ]; then return; fi
        sleep 0.2
    done
    echo 'Timed out waiting for saved application arguments' >&2; exit 1
}
"$WRAPPER" install "$CONFIG" 'wrapper.pidfile=../bin/installed-%USER%.pid' 'wrapper.app.parameter.2=persisted parameter' -- 'persisted argument'
start_installed
wait_event before
invoke status
grep -q 'arg=persisted parameter' "$TEST_ROOT/logs/console.log"
grep -q 'arg=persisted argument' "$TEST_ROOT/logs/console.log"
sed 's/value=before/value=after/' "$CONFIG" > "$TEST_ROOT/conf/new.conf"
mv "$TEST_ROOT/conf/new.conf" "$CONFIG"
touch "$TEST_ROOT/restart"
wait_event after
invoke stop
[ "$(grep -c '^stop ' "$TEST_ROOT/events")" = 2 ]
# Manual startup must use the same saved arguments as boot-time startup.
"$TEST_ROOT/bin/service.sh" start
wait_arguments 3
invoke status
invoke restart
wait_arguments 4
invoke condrestart
wait_arguments 5
invoke status
if [ -e "/etc/init.d/$TEST_NAME" ] && [ ! -d /run/systemd/system ]; then
    cp "/etc/init.d/$TEST_NAME" "$TEST_ROOT/init-backup"
    cat > "/etc/init.d/$TEST_NAME" <<EOF
#!/bin/bash
if [ "\$1" = stop ]; then
    echo 'Simulated service stop failure' >&2
    exit 7
fi
exec "$TEST_ROOT/init-backup" "\$@"
EOF
    if invoke remove > "$TEST_ROOT/remove-output" 2>&1; then
        echo 'Removal must fail when the service cannot be stopped' >&2; exit 1
    fi
    grep -q 'exit code 7' "$TEST_ROOT/remove-output"
    [ -f "/etc/init.d/$TEST_NAME" ]
    invoke status
    cp "$TEST_ROOT/init-backup" "/etc/init.d/$TEST_NAME"
    rm "$TEST_ROOT/init-backup"
fi
invoke stop
invoke condrestart
if invoke status; then
    echo 'Conditional restart must leave a stopped service stopped' >&2; exit 1
else
    [ "$?" = 3 ]
fi
[ "$(grep -c '^start ' "$TEST_ROOT/events")" = 5 ]
[ "$(grep -c '^stop ' "$TEST_ROOT/events")" = 5 ]
[ "$(grep -c 'arg=persisted parameter' "$TEST_ROOT/logs/console.log")" = 5 ]
[ "$(grep -c 'arg=persisted argument' "$TEST_ROOT/logs/console.log")" = 5 ]
if [ -e "/etc/init.d/$TEST_NAME" ] && [ ! -d /run/systemd/system ]; then
    invoke remove
    # Upgraded JSW deployments may still use an init symlink to the launcher.
    ln -s "$TEST_ROOT/bin/service.sh" "/etc/init.d/$TEST_NAME"
    "$TEST_ROOT/bin/service.sh" start
    invoke status
    invoke stop
    if invoke status; then
        echo 'Expected stopped SysV status to return 3' >&2; exit 1
    else
        [ "$?" = 3 ]
    fi
fi
# Java discovery failures must reach both the log and the operator, even when
# native installation saved a different PID path. A later healthy start must not
# reuse the previous failure.
cp "$CONFIG" "$TEST_ROOT/working.conf"
echo "wrapper.java.command=$TEST_ROOT/missing-java" >> "$CONFIG"
if start_installed > "$TEST_ROOT/start-out" 2> "$TEST_ROOT/start-err"; then
    echo 'Missing Java must fail the start command' >&2; exit 1
fi
grep -q 'Launching .*missing-java' "$TEST_ROOT/start-err"
grep -q 'Launching .*missing-java' "$TEST_ROOT/logs/console.log"
cp "$TEST_ROOT/working.conf" "$CONFIG"
start_installed
invoke status
invoke stop
if [ "$(uname -s)" = Linux ] && [ -d /run/systemd/system ]; then
    # Exercise plain systemctl, which must wait for JVM startup and return a
    # failing job even when the launcher never executes or Java cannot start.
    [ "$(systemctl show "$TEST_NAME" -p Type --value)" = notify ]
    expect_systemctl_failure() {
        systemctl reset-failed "$TEST_NAME"
        if systemctl start "$TEST_NAME" > "$TEST_ROOT/systemctl-output" 2>&1; then
            echo 'systemctl must report a failed start' >&2; exit 1
        fi
        grep -qi failed "$TEST_ROOT/systemctl-output"
    }
    chmod 644 "$TEST_ROOT/bin/service.sh"
    expect_systemctl_failure
    [ "$(systemctl show "$TEST_NAME" -p ExecMainStatus --value)" = 203 ]
    chmod 755 "$TEST_ROOT/bin/service.sh"
    for FAILURE in "wrapper.java.command=$TEST_ROOT/missing-java" \
            'wrapper.java.additional.10=-Xmxinvalid-fixture-size'; do
        cp "$TEST_ROOT/working.conf" "$CONFIG"
        echo "$FAILURE" >> "$CONFIG"
        expect_systemctl_failure
    done
    journalctl -u "$TEST_NAME" --no-pager > "$TEST_ROOT/journal"
    grep -q 'missing-java' "$TEST_ROOT/journal"
    grep -q 'invalid-fixture-size' "$TEST_ROOT/journal"

    cp "$TEST_ROOT/working.conf" "$CONFIG"
    cat > "$TEST_ROOT/boot/slow-java" <<EOF
#!/bin/sh
sleep 1
exec "$(command -v java)" "\$@"
EOF
    chmod 755 "$TEST_ROOT/boot/slow-java"
    echo "wrapper.java.command=$TEST_ROOT/boot/slow-java" >> "$CONFIG"
    systemctl reset-failed "$TEST_NAME"
    START_TIME=$(date +%s)
    systemctl start "$TEST_NAME"
    [ "$(( $(date +%s) - START_TIME ))" -ge 2 ]
    invoke status
    invoke stop

    # The configured wait limit becomes systemd's activation timeout.
    invoke remove
    echo 'wrapper.startup.wait.timeout=1' >> "$CONFIG"
    invoke install
    expect_systemctl_failure
    [ "$(systemctl show "$TEST_NAME" -p Result --value)" = timeout ]
    invoke remove
    echo 'wrapper.startup.wait.timeout=0' >> "$CONFIG"
    invoke install
    [ "$(systemctl show "$TEST_NAME" -p TimeoutStartUSec --value)" = infinity ]
fi
echo 'Unix service acceptance test passed'
