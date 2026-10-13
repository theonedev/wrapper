# Wrapper

Run an existing Java application as a service on **macOS, Windows, or Linux**.
Wrapper is an MIT-licensed native supervisor that starts Java, forwards service
stops to shutdown hooks, and captures and rotates stdout/stderr logs.

Your application needs Java 17+ and a `public static void main(String[] args)`.
Integration is manual: copy files and edit `wrapper.conf`. Your application needs
no Wrapper API dependency, build plugin, or JNI library.

## Quick start

1. Download [wrapper-latest.tar.gz](https://code.onedev.io/onedev/wrapper/~sites/public/wrapper-latest.tar.gz)
   ([SHA-256](https://code.onedev.io/onedev/wrapper/~sites/public/wrapper-latest.tar.gz.sha256))
   and extract it. This prebuilt package includes all supported platforms.
   Use the extracted directory as your application distribution, or follow the
   [integration guide](docs/integration.md) to copy Wrapper into an existing layout.
2. Put your application JAR and its runtime dependencies in `lib/`.
3. Edit the existing entries in `conf/wrapper.conf`:

   ```properties
   wrapper.name=my-app
   wrapper.displayname=My Application
   wrapper.description=My Java application
   wrapper.java.mainclass=com.example.Main
   wrapper.java.classpath.1=../lib/*.jar
   ```

   For a JAR normally run with `java -jar`, use its manifest's `Main-Class` value.
   For Spring Boot, this is the Boot launcher class; see the
   [executable JAR instructions](docs/integration.md#executable-jars-and-spring-boot).
4. From the application directory, run:

   ```sh
   bin/service.sh console
   ```

   On Windows, run `.\bin\service.bat console`. Check `logs/console.log` and press
   Ctrl+C to stop. A service must keep at least one non-daemon thread alive.
5. Move the distribution to its permanent location and set `wrapper.java.command`
   to the target machine's Java executable, preferably an absolute path. Then install
   and start it on macOS/Linux:

   ```sh
   sudo bin/service.sh install
   sudo bin/service.sh start
   ```

   On Windows, run `.\bin\service.bat install` and `.\bin\service.bat start`;
   the launcher requests administrator permission when needed.

Paths in the default configuration are relative to `boot/`. For applications
that expect to run from their top-level directory, use the
[working directory instructions](docs/integration.md#working-directory).
Ordinary JVM exits stop Wrapper; automatic crash restart is not implemented.

## Supported platforms

| Operating system | Architecture | Service manager |
| --- | --- | --- |
| macOS | Apple Silicon (arm64) | launchd |
| Windows | amd64, arm64 | Windows Service Control Manager |
| Linux | amd64, arm64 | systemd; SysV fallback |

Install Java 17+ for the target platform, or bundle it with your application.
Unix launchers require Bash and standard OS utilities. Linux executables are
statically linked with musl; Windows executables use the static C runtime.

## Documentation

- [Integrate an existing application](docs/integration.md): copying, configuration, and JVM options.
- [Run the Java example](examples/hello-service/README.md): compile and try a small service.
- [Manage services](docs/services.md): accounts, commands, logs, and upgrades.
- [Configuration reference](docs/reference.md): syntax, lifecycle, and compatibility.
- [Development guide](docs/development.md): install build tools, build Wrapper, and publish releases.
- [Contributing](CONTRIBUTING.md): project layout and contribution checks.

Source and issue tracker: [code.onedev.io/onedev/wrapper](https://code.onedev.io/onedev/wrapper).

## License

Source, Java bridge, launchers, documentation, and examples use the [MIT license](LICENSE).
Keep `LICENSE` and the release's `THIRD-PARTY-LICENSES.txt` when redistributing
Wrapper. Your application can use its own license.
