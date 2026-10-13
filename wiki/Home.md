# Wrapper documentation

Wrapper runs an existing Java application as a service on macOS, Windows, and Linux.
Start with the prebuilt
[latest package](https://code.onedev.io/onedev/wrapper/~sites/public/wrapper-latest.tar.gz)
([SHA-256](https://code.onedev.io/onedev/wrapper/~sites/public/wrapper-latest.tar.gz.sha256))
and follow the [integration guide](Integration.md). You need Java 17+ and your
application's JARs; integration consists of copying files and editing configuration.

## Use Wrapper

| Guide | What it covers |
| --- | --- |
| [Integrate an application](Integration.md) | Runtime layout, existing Java commands, executable JARs, working directories, and bundled Java |
| [Run the example](Example.md) | Compile and run a small Java service with a shutdown hook |
| [Manage services](Services.md) | Installation, accounts, logging, troubleshooting, and upgrades |
| [Commands and configuration](Configuration.md) | Supported settings, configuration syntax, lifecycle behavior, and compatibility |

## Develop Wrapper

Report bugs or request features through [issues](https://code.onedev.io/onedev/wrapper/~issues),
and submit changes through [pull requests](https://code.onedev.io/onedev/wrapper/~pulls).
Both are managed on **code.onedev.io**.

- [Development guide](Development.md): install build tools, build, test, and publish releases.
- [Contributing](Contributing.md): project layout, contribution checks, and wiki maintenance.

The [project introduction](../README.md) lists supported platforms and licensing.
These pages live in `wiki/` in the source repository and are also included in
release packages for offline reading.
