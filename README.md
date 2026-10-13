# Wrapper

Run an existing Java application as a service on **macOS, Windows, or Linux**.
Wrapper is an MIT-licensed native supervisor with a small Java bridge. It starts
Java, forwards service stops to shutdown hooks, captures and rotates stdout/stderr
logs, and supports application-requested restarts.

Your application needs Java 17+ and a `public static void main(String[] args)`.
Integration is manual: copy runtime files and edit `wrapper.conf`. No Wrapper API
dependency, build plugin, JNI library, or OneDev Server installation is required.
A service must keep a non-daemon thread alive; ordinary JVM exits stop Wrapper.
Automatic crash restart and JVM responsiveness monitoring are not implemented.

## Download and documentation

Download [wrapper-latest.tar.gz](https://code.onedev.io/onedev/wrapper/~sites/public/wrapper-latest.tar.gz)
([SHA-256](https://code.onedev.io/onedev/wrapper/~sites/public/wrapper-latest.tar.gz.sha256)).
The prebuilt package includes all supported platforms.

The [OneDev wiki](https://code.onedev.io/onedev/wrapper/~wiki) covers integration,
service administration, configuration, examples, and development.
Start with [Integrating an application](wiki/Integration.md), or browse the
[documentation home](wiki/Home.md). The same wiki pages are included in downloaded
packages for offline reading.

## Supported platforms

| Operating system | Architecture | Service manager |
| --- | --- | --- |
| macOS | Apple Silicon (arm64) | launchd |
| Windows | amd64, arm64 | Windows Service Control Manager |
| Linux | amd64, arm64 | systemd; SysV fallback |

Install Java 17+ for the target platform, or bundle it with your application.
Unix launchers require Bash and standard OS utilities. Linux executables are
statically linked with musl; Windows executables use the static C runtime.

Source and issue tracker: [code.onedev.io/onedev/wrapper](https://code.onedev.io/onedev/wrapper).
Build instructions are in the [development guide](wiki/Development.md);
contribution guidelines are in [Contributing](wiki/Contributing.md).

## License

Source, Java bridge, launchers, documentation, and examples use the [MIT license](LICENSE).
Keep `LICENSE` and the release's `THIRD-PARTY-LICENSES.txt` when redistributing
Wrapper. Your application can use its own license.
