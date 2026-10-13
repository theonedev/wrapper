#!/bin/bash
# Run with JDK 17+ in a disposable Linux container. The fixture is compiled locally.
set -euo pipefail
PROJECT=$(cd "$(dirname "$0")/.." && pwd)
TEST_ROOT=$(mktemp -d '/tmp/Wrapper Linux smoke.XXXXXX')
mkdir -p "$TEST_ROOT"/{bin,boot,conf,logs}
cp "$PROJECT"/dist/boot/* "$TEST_ROOT/boot/"
cp "$PROJECT/resources/App.sh.in" "$TEST_ROOT/bin/service.sh"
chmod +x "$TEST_ROOT/bin/service.sh"
javac --release 17 -cp "$TEST_ROOT/boot/wrapper.jar" -d "$TEST_ROOT/boot" "$PROJECT/tests/Fixture.java"
cat > "$TEST_ROOT/conf/wrapper.conf" <<EOF
wrapper.name=wrapper-linux-test
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

    # A detached root start must not pass a privileged log descriptor to nobody.
    ROOT_ONLY="$TEST_ROOT/root-only"
    mkdir -m 755 "$ROOT_ONLY"
    printf 'root-only sentinel\n' > "$ROOT_ONLY/existing.log"
    chmod 600 "$ROOT_ONLY/existing.log"
    cp "$ROOT_ONLY/existing.log" "$TEST_ROOT/expected-root-log"
    ln -s "$ROOT_ONLY/existing.log" "$TEST_ROOT/logs/existing-link"
    ln -s "$ROOT_ONLY/missing.log" "$TEST_ROOT/logs/missing-link"
    ln -s "$ROOT_ONLY" "$TEST_ROOT/logs/directory-link"
    cat >> "$TEST_ROOT/conf/wrapper.conf" <<EOF
wrapper.java.command=$TEST_ROOT/missing-java
wrapper.startup.wait.timeout=10
EOF
    expect_start_failure() {
        if "$TEST_ROOT/bin/service.sh" start > "$TEST_ROOT/start-output" 2>&1; then
            echo 'Expected detached startup to fail' >&2
            exit 1
        fi
    }
    for LOG_PATH in "$TEST_ROOT/logs/existing-link" "$TEST_ROOT/logs/missing-link" \
            "$TEST_ROOT/logs/directory-link/missing.log" "$ROOT_ONLY/existing.log" \
            "$ROOT_ONLY/new-directory/console.log"; do
        echo "wrapper.logfile=$LOG_PATH" >> "$TEST_ROOT/conf/wrapper.conf"
        expect_start_failure
        cmp "$TEST_ROOT/expected-root-log" "$ROOT_ONLY/existing.log"
        [ ! -e "$ROOT_ONLY/missing.log" ]
        [ ! -e "$ROOT_ONLY/new-directory" ]
    done

    # Resolve the startup log using the service account's environment, and create
    # both its directory and file as that account while retaining diagnostics.
    cat >> "$TEST_ROOT/conf/wrapper.conf" <<'EOF'
set.startup_log_dir=../logs/%USER%
wrapper.logfile=%startup_log_dir%/console.log
EOF
    expect_start_failure
    [ "$(stat -c %U "$TEST_ROOT/logs/nobody")" = nobody ]
    [ "$(stat -c %U "$TEST_ROOT/logs/nobody/console.log")" = nobody ]
    grep -q 'missing-java' "$TEST_ROOT/logs/nobody/console.log"
    [ ! -e "$TEST_ROOT/logs/root" ]
    echo "wrapper.java.command=$(command -v java)" >> "$TEST_ROOT/conf/wrapper.conf"
    "$TEST_ROOT/bin/service.sh" start
    "$TEST_ROOT/bin/service.sh" stop
    # Controllers run as root, but must locate the service account's PID/control files.
    echo 'wrapper.pidfile=../bin/%USER%.pid' >> "$TEST_ROOT/conf/wrapper.conf"
    "$TEST_ROOT/bin/service.sh" start
    "$TEST_ROOT/bin/service.sh" status
    OLD_PID=$(cat "$TEST_ROOT/bin/nobody.pid")
    "$TEST_ROOT/bin/service.sh" restart
    [ "$(cat "$TEST_ROOT/bin/nobody.pid")" != "$OLD_PID" ]
    "$TEST_ROOT/bin/service.sh" stop
    [ ! -s "$TEST_ROOT/bin/nobody.pid" ]
fi
if [ "$(id -u)" = 0 ]; then
    bash "$PROJECT/tests/install-failure.sh"
fi
echo "Linux $(uname -m) lifecycle test passed"
