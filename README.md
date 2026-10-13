# A Minimized Java Service Wrapper

Run Java applications as native services on **macOS, Windows, and Linux**.

Originally created for [OneDev](https://onedev.io/), a development platform with
Git hosting, issue tracking, and CI/CD, Service Wrapper is now a standalone
project that other Java applications can use too.

Inspired by [Java Service Wrapper (JSW)](https://wrapper.tanukisoftware.com/doc/english/introduction.html),
this project borrows some JSW configuration conventions. Its native supervisor
is a clean room rewrite in Rust, with a small Java bridge.

The focus is minimal service integration: installation, startup, graceful shutdown,
log rotation, and application-requested restarts. Advanced JSW features such as
automatic crash recovery, JVM monitoring, and event or scheduled actions are
outside its scope.

Requires Java 17+. Supports macOS arm64 and Windows/Linux amd64 and arm64.

[Download](https://code.onedev.io/onedev/wrapper/~sites/public/wrapper-latest.tar.gz) ·
[Wiki](https://code.onedev.io/onedev/wrapper/~wiki) ·
[MIT license](LICENSE)
