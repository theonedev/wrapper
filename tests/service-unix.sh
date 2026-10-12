#!/bin/bash
# Run as root on a disposable Linux/systemd or Apple Silicon/launchd host.
set -euo pipefail
[ "$(id -u)" = 0 ] || { echo 'Run as root on a disposable service test host.' >&2; exit 1; }
PROJECT=$(cd "$(dirname "$0")/.." && pwd)
TEST_ROOT=$(mktemp -d '/tmp/OneDev service test.XXXXXX')
TEST_NAME=onedev-wrapper-test-$$
mkdir -p "$TEST_ROOT"/{bin,boot,conf,logs}
cp "$PROJECT"/dist/boot/* "$TEST_ROOT/boot/"
cp "$PROJECT/resources/App.sh.in" "$TEST_ROOT/bin/server.sh"
chmod +x "$TEST_ROOT/bin/server.sh"
javac --release 17 -cp "$TEST_ROOT/boot/wrapper.jar" -d "$TEST_ROOT/boot" "$PROJECT/tests/Fixture.java"
cat > "$TEST_ROOT/conf/wrapper.conf" <<EOF
#encoding=UTF-8
wrapper.name=$TEST_NAME
wrapper.java.command=$(command -v java)
wrapper.java.classpath.1=.
wrapper.app.parameter.1=Fixture
wrapper.java.additional.1=-Dfixture.root=$TEST_ROOT
wrapper.java.additional.2=-Dfixture.value=before
wrapper.shutdown.timeout=10
wrapper.restart.delay=0
EOF
cleanup() {
    if "$TEST_ROOT/bin/server.sh" remove; then
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
"$TEST_ROOT/bin/server.sh" install
"$TEST_ROOT/bin/server.sh" start
wait_event before
"$TEST_ROOT/bin/server.sh" status
sed 's/value=before/value=after/' "$TEST_ROOT/conf/wrapper.conf" > "$TEST_ROOT/conf/new.conf"
mv "$TEST_ROOT/conf/new.conf" "$TEST_ROOT/conf/wrapper.conf"
touch "$TEST_ROOT/restart"
wait_event after
"$TEST_ROOT/bin/server.sh" stop
[ "$(grep -c '^stop ' "$TEST_ROOT/events")" = 2 ]
"$TEST_ROOT/bin/server.sh" start
"$TEST_ROOT/bin/server.sh" stop
echo 'Unix service acceptance test passed'
