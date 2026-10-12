#!/bin/bash
# Run with JDK 17+ in a disposable Linux container. The fixture is compiled locally.
set -euo pipefail
PROJECT=$(cd "$(dirname "$0")/.." && pwd)
TEST_ROOT=$(mktemp -d '/tmp/ServiceWrapper Linux smoke.XXXXXX')
mkdir -p "$TEST_ROOT"/{bin,boot,conf,logs}
cp "$PROJECT"/dist/boot/* "$TEST_ROOT/boot/"
cp "$PROJECT/resources/App.sh.in" "$TEST_ROOT/bin/service.sh"
chmod +x "$TEST_ROOT/bin/service.sh"
javac --release 17 -cp "$TEST_ROOT/boot/wrapper.jar" -d "$TEST_ROOT/boot" "$PROJECT/tests/Fixture.java"
cat > "$TEST_ROOT/conf/wrapper.conf" <<EOF
wrapper.name=servicewrapper-linux-test
wrapper.java.command=$(command -v java)
wrapper.java.classpath.1=.
wrapper.app.parameter.1=Fixture
wrapper.java.additional.1=-Dfixture.root=$TEST_ROOT
wrapper.java.additional.2=-Dfixture.value=before
wrapper.shutdown.timeout=10
wrapper.restart.delay=0
EOF
cleanup() { "$TEST_ROOT/bin/service.sh" stop; rm -rf "$TEST_ROOT"; }
trap cleanup EXIT
"$TEST_ROOT/bin/service.sh" start
"$TEST_ROOT/bin/service.sh" status
sed 's/value=before/value=after/' "$TEST_ROOT/conf/wrapper.conf" > "$TEST_ROOT/conf/new.conf"
mv "$TEST_ROOT/conf/new.conf" "$TEST_ROOT/conf/wrapper.conf"
touch "$TEST_ROOT/restart"
for ((attempt=0; attempt<100; attempt++)); do
    if [ -f "$TEST_ROOT/events" ] && grep -q after "$TEST_ROOT/events"; then break; fi
    sleep 0.2
done
grep -q after "$TEST_ROOT/events"
"$TEST_ROOT/bin/service.sh" stop
[ "$(grep -c '^start ' "$TEST_ROOT/events")" = 2 ]
[ "$(grep -c '^stop ' "$TEST_ROOT/events")" = 2 ]
if [ "$(id -u)" = 0 ] && id nobody >/dev/null 2>&1; then
    echo 'wrapper.daemon.user=nobody' >> "$TEST_ROOT/conf/wrapper.conf"
    chown -R nobody "$TEST_ROOT"
    "$TEST_ROOT/bin/service.sh" start
    "$TEST_ROOT/bin/service.sh" stop
    grep -q "home=$(getent passwd nobody | cut -d: -f6)" "$TEST_ROOT/logs/console.log"
fi
echo "Linux $(uname -m) lifecycle test passed"
