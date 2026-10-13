# Contributing to Wrapper

[Home](Home.md) · [Development](Development.md)

Clone [the standalone repository](https://code.onedev.io/onedev/wrapper) and use
the [build guide](Development.md) to install its pinned toolchain. No OneDev
application checkout or private Maven repository is needed. Report reproducible
problems and propose changes through the project's issue tracker and pull requests.
Include the Wrapper version, OS/architecture, Java version, and relevant redacted
configuration and logs when reporting a bug.

## Project layout

| Path | Purpose |
| --- | --- |
| `src/` | Rust supervisor, configuration, logging, process control, OS service registration |
| `java/` | Java main-method bridge and optional lifecycle API, compiled for Java 17 |
| `resources/` | Generic launchers and default configuration |
| `package.py`, `licenses/` | Runtime release assembly, checksums, dependency notices |
| `examples/` | Runnable Java example source |
| `wiki/` | OneDev wiki pages, also included in release packages |
| `tests/` | JVM integration and native service acceptance |
| `ci/release.sh` | Full release validation and archive creation |

The native supervisor always starts the Java bridge on the classpath. The bridge
connects to an authenticated loopback control endpoint, invokes the application's
main method, and translates a service stop into JVM shutdown. The application
does not need to import wrapper classes. The native process owns service-manager
integration and JVM descendant cleanup. Keep application-specific packaging out
of this repository.

## Checks

After building the host runtime, run:

```sh
cargo fmt --all -- --check
cargo test --locked
cargo clippy --locked --all-targets -- -D warnings
python3 tests/integration.py
```

Use `python` on Windows. The JVM integration suite uses temporary fixtures and
does not register machine services. Follow the [example](Example.md)
to check the documented manual assembly steps with a released runtime.
Native service acceptance scripts require disposable hosts and elevated access;
see the build guide. Verify affected target builds as well as host tests when
changing native APIs. Add focused regression coverage for behavior changes.

Keep user-visible configuration and launcher behavior compatible where possible.
Document migration steps when compatibility must change. Keep `Cargo.lock` pinned,
and ensure release packaging can collect the license text of every dependency.

## Documentation

OneDev reads the project wiki from `wiki/`. `Home.md` is its landing page and
`_Sidebar.md` defines navigation. Keep the root `README.md` focused on the project
introduction; put integration, administration, and development instructions in the
wiki. Example source stays in `examples/hello-service/`.

Use relative Markdown links between pages so they work in OneDev and offline.
Update `Home.md` and `_Sidebar.md` when adding or renaming a page. Release packaging
includes the same `wiki/` directory; check links in both the checkout and extracted
package.

Contributions to Wrapper, including examples and documentation, are provided
under the project's [MIT license](../LICENSE).
