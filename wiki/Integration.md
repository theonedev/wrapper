# Integrate an existing Java application

[Home](Home.md) · [Services](Services.md) · [Reference](Configuration.md)

Keep your application's main method and shutdown hooks. You need a Java 17+
runtime, your application's JARs, and the prebuilt Wrapper package. Build your
application and collect dependencies with your usual build process.

## Download Wrapper

Download [wrapper-latest.tar.gz](https://code.onedev.io/onedev/wrapper/~sites/public/wrapper-latest.tar.gz)
([SHA-256](https://code.onedev.io/onedev/wrapper/~sites/public/wrapper-latest.tar.gz.sha256))
from code.onedev.io and extract it. The archive contains a `wrapper-VERSION/`
directory with binaries for every supported platform. The download URL stays the
same across releases; the included `VERSION` file identifies the release.

Use this extracted directory in the steps below. Building Wrapper from source
is covered separately in the [development guide](Development.md).

## 1. Copy the runtime files

Use the extracted release as a starting directory, or copy its runtime files into
your own application distribution. The recommended layout is:

```text
my-app/
  bin/service.sh             # Unix launcher
  bin/service.bat            # Windows launcher
  boot/wrapper-*             # Native executables for your target platforms
  boot/wrapper.jar           # Java bridge; always required
  conf/wrapper.conf          # Application and service settings
  lib/                      # Your application and dependency JARs
  logs/                     # Created as needed
  licenses/wrapper/          # LICENSE, THIRD-PARTY-LICENSES.txt, VERSION
```

For an existing distribution, preserve your application's directories and files;
merge the following files into a staging copy. Set the three directory paths at
the start of the block for your platform. The JAR source directory should contain
your application's JARs and runtime dependency JARs.

On macOS/Linux, using Bash:

```sh
wrapper_release='/path/to/extracted-wrapper'
application_dir='/path/to/my-app'
application_jars_dir='/path/to/application-jars'

mkdir -p "$application_dir/bin" "$application_dir/boot" "$application_dir/conf" \
  "$application_dir/lib" "$application_dir/licenses/wrapper"
cp -R "$wrapper_release/boot/." "$application_dir/boot/"
cp "$wrapper_release/bin/service.sh" "$wrapper_release/bin/service.bat" "$application_dir/bin/"
cp "$wrapper_release/conf/wrapper.conf" "$application_dir/conf/"
cp "$wrapper_release/LICENSE" "$wrapper_release/THIRD-PARTY-LICENSES.txt" \
  "$wrapper_release/VERSION" "$application_dir/licenses/wrapper/"
cp "$application_jars_dir"/*.jar "$application_dir/lib/"
chmod +x "$application_dir/bin/service.sh" "$application_dir"/boot/wrapper-*
```

On Windows, using PowerShell:

```powershell
$wrapperRelease = 'C:\downloads\wrapper'
$applicationDir = 'C:\services\my-app'
$applicationJarsDir = 'C:\build\application-jars'

New-Item -ItemType Directory -Force -Path "$applicationDir\bin", "$applicationDir\boot", "$applicationDir\conf", "$applicationDir\lib", "$applicationDir\licenses\wrapper" | Out-Null
Copy-Item "$wrapperRelease\boot\*" "$applicationDir\boot\"
Copy-Item "$wrapperRelease\bin\service.sh", "$wrapperRelease\bin\service.bat" "$applicationDir\bin\"
Copy-Item "$wrapperRelease\conf\wrapper.conf" "$applicationDir\conf\"
Copy-Item "$wrapperRelease\LICENSE", "$wrapperRelease\THIRD-PARTY-LICENSES.txt", "$wrapperRelease\VERSION" "$applicationDir\licenses\wrapper\"
Copy-Item "$applicationJarsDir\*.jar" "$applicationDir\lib\"
```

These commands copy all JARs directly inside the source directory. Copy nested
dependencies and other resources separately, preserving any layout your app
requires. Keep `boot/`, `bin/`, and `conf/` as siblings; your application JARs may
live elsewhere if you adjust the classpath. Keep only the native executables for
your target platforms, or retain all of them. Always use `wrapper.jar` and native
executables from the same release, and preserve Unix executable permissions.

## 2. Translate your Java command

For an application normally started with:

```sh
java -Xmx512m -cp 'lib/*' com.example.Main --port 8080
```

edit `conf/wrapper.conf` so it contains these settings. Replace existing values;
uncomment or add optional entries as needed:

```properties
wrapper.name=my-app
wrapper.displayname=My Application
wrapper.description=My Java application
wrapper.java.command=java
wrapper.java.mainclass=com.example.Main
wrapper.java.classpath.1=../lib/*.jar
wrapper.java.additional.1=-Xmx512m
wrapper.app.parameter.1=--port
wrapper.app.parameter.2=8080
```

Each numbered property is one value, including spaces. Use separate entries for
separate JVM flags or application arguments; shell quoting is unnecessary inside
the configuration. For example, `wrapper.java.additional.2=-Dmessage=Hello world`
passes one JVM option. The supplied `wrapper.conf` comments describe every
supported property, its default, and platform restrictions.

`../lib/*.jar` includes JARs directly in `lib/`. Add entries such as
`wrapper.java.classpath.2=../conf` for loose resources or
`wrapper.java.classpath.3=../classes` for compiled classes. Wildcards do not
search nested directories. Wrapper adds its own bridge JAR automatically.

### Executable JARs and Spring Boot

For `java -jar my-app.jar`, inspect `META-INF/MANIFEST.MF` inside the JAR and use
its `Main-Class` value for `wrapper.java.mainclass`. Join manifest continuation
lines if necessary. For Spring Boot, use the launcher named by `Main-Class`;
keep the entire JAR intact, including its nested dependencies. The application's
`Start-Class` is invoked by that launcher. See the
[Spring Boot launch specification](https://docs.spring.io/spring-boot/specification/executable-jar/launching.html).

Preserve paths referenced by the manifest's `Class-Path`. For example,
`Class-Path: dependencies/helper.jar` inside `lib/my-app.jar` requires
`lib/dependencies/helper.jar`.

Wrapper invokes the main class on the classpath. It does not run `java -jar` or
`java -m`. Applications that rely on named modules or launcher-only manifest
attributes (such as `Launcher-Agent-Class` or `Add-Opens`) need equivalent JVM
options or an adapted classpath entry point. See the
[JAR specification](https://docs.oracle.com/en/java/javase/17/docs/specs/jar/jar.html).

### Working directory

By default, the JVM runs from `boot/`, regardless of where you invoke the launcher.
Classpath, native library, log, PID, and application-relative file paths use that
working directory. For example, use `../conf/application.properties` to refer to
an application configuration file in the recommended layout.

If your application expects to run from `my-app/`, set all of these together:

```properties
wrapper.working.dir=..
wrapper.java.classpath.1=lib/*.jar
wrapper.logfile=logs/console.log
wrapper.pidfile=bin/my-app.pid
```

Then application arguments can use paths such as `conf/application.properties`.
Update any other relative classpath, native library, or Java executable paths too.
`wrapper.working.dir` itself, configuration includes, and `wrapper.script` remain
relative to `boot/`.

## 3. Test in the foreground

From the assembled application directory:

```sh
bin/service.sh console
```

On Windows, use `.\bin\service.bat console`. Check startup and
`logs/console.log`, then press Ctrl+C and verify that shutdown hooks finish.
A long-running service must keep a non-daemon thread alive. An ordinary JVM exit
stops Wrapper. Arguments after `console` are appended to configured application
arguments; for example, `bin/service.sh console --port 8081`.

The [Hello service example](Example.md) provides a small
application to try before integrating your own.

## 4. Install and distribute

Move the application to its permanent location, select Java as described below,
and follow the [service guide](Services.md) to install and start it. Each instance
needs a unique `wrapper.name` and separate log/PID paths; separate application
directories are the simplest arrangement. You can rename the launchers to
`my-app.sh` and `my-app.bat` without changing the service name.

Distribute the directory using your existing release process. Keep Wrapper's MIT
license and dependency notices alongside your application's own notices. The
source checkout, documentation, examples, Rust, Python, and a JDK are not needed
to run a packaged application; a compatible Java runtime is required.

### Select or bundle Java

Before service installation, set `wrapper.java.command` to the target Java
executable, for example `/opt/java/bin/java` or
`C:\Program Files\Java\jdk-21\bin\java.exe`. Services do not load shell profiles.
The default is `java` on `PATH`; Wrapper does not automatically use `JAVA_HOME`.

Alternatively, put a runtime in `runtime/` alongside `boot/` and set:

```properties
# With the default boot/ working directory, on macOS/Linux:
wrapper.java.command=../runtime/bin/java
# On Windows, use ../runtime/bin/java.exe instead.
```

With `wrapper.working.dir=..`, use `runtime/bin/java` (or `runtime/bin/java.exe`).
Bundle a runtime for each target OS/architecture and include its own notices.
Applications with native dependencies may also need separate platform archives.

## Optional lifecycle API

Shutdown hooks and `System.exit()` work without a Wrapper API dependency.
For application-requested restarts or stops, compile against `boot/wrapper.jar`:

```java
import io.onedev.wrapper.WrapperManager;

WrapperManager.restartAndReturn(); // Run shutdown hooks, then start a new JVM.
WrapperManager.stopAndReturn(0);   // Run shutdown hooks and stop Wrapper.
```

Call these only when launched by Wrapper. Treat the bridge as a provided runtime
dependency; do not bundle a second copy into your application's JAR.
