# Hello service

A plain Java main method with stdout logging, arguments, and a shutdown hook.
It stays alive until stopped and has no Wrapper API or framework dependency.
The source is covered by Wrapper's [MIT license](../../LICENSE).

[Download the prebuilt Wrapper package](../../docs/integration.md#download-wrapper)
and start in its **freshly extracted directory**, with JDK 17+ tools (`javac`, `jar`,
and `java`) on `PATH`. The package includes this example and the runtime layout.

## 1. Compile the application

On macOS/Linux:

```sh
mkdir -p build/hello-service
javac --release 17 -d build/hello-service examples/hello-service/HelloService.java
jar --create --file lib/hello-service.jar --main-class example.HelloService -C build/hello-service .
```

On Windows, in Command Prompt:

```bat
mkdir build\hello-service
javac --release 17 -d build\hello-service examples\hello-service\HelloService.java
jar --create --file lib\hello-service.jar --main-class example.HelloService -C build\hello-service .
```

## 2. Configure Wrapper

Replace the corresponding entries in `conf/wrapper.conf`:

```properties
wrapper.name=hello-service
wrapper.displayname=Hello Java Service
wrapper.description=Example Java service
wrapper.java.mainclass=example.HelloService
wrapper.java.classpath.1=../lib/*.jar
```

## 3. Run and stop

```sh
bin/service.sh console
```

On Windows, use `.\bin\service.bat console`. Expect `Hello service started` and
`Greeting: Hello from Java`. Press Ctrl+C; the hook prints
`Hello service stopped: shutdown hook completed`. Output is also captured in
`logs/console.log`. A service stop invokes the same hook.

For a check that exits by itself:

```sh
bin/service.sh console --once "an argument with spaces"
```

On Windows, use `.\bin\service.bat console --once "an argument with spaces"`.
To change the greeting, add this JVM option to `conf/wrapper.conf`:

```properties
wrapper.java.additional.1=-Dhello.greeting=Hello from configuration
```

Compiling and running in console mode does not register an OS service. To try
registration, select an absolute Java path and follow the
[service guide](../../docs/services.md). To package your own application, follow
the [integration guide](../../docs/integration.md).
