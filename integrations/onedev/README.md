# OneDev packaging integration

The wrapper source now lives in the separate
[servicewrapper project](https://code.onedev.io/onedev/servicewrapper).
This directory contains OneDev's JVM defaults and release verification script.
The general wrapper does not require OneDev server or agent jars.

From a sibling checkout at `onedev/servicewrapper`, build the five targets and run:

```sh
python3 package.py --profile onedev --destination ../server/server-product/wrapper
```

Alternatively download `servicewrapper-onedev-VERSION.tar.gz` and its `.sha256`
from a successful **Release** build, verify the checksum, and extract the archive's
contents into `server/server-product/wrapper`. All native binaries, `wrapper.jar`,
launcher templates, `wrapper.conf`, `VERSION`, and `SHA256SUMS` are included.
Maven does not need Rust or a bridge compiler when packaging the server.

The companion maven-plugin 2.9.1 changes select `Command.sh.in` / `Command.bat.in`
for database/upgrade utilities and ordinary templates for server and agent. They
must be published before the server change. Validate the resulting release with:

```sh
python3 integrations/onedev/check_package.py ../server/server-product/target/onedev-VERSION.zip ../server/server-product/wrapper
```

Existing `server.sh` / `agent.sh` commands and `conf/wrapper.conf` remain supported.
The JSW-compatible Java bridge preserves the server/agent's existing reflective
restart and stop calls. Stop deployments before replacing native wrapper files.
Server upgrades replace boot files and scripts while retaining `wrapper.conf`.
Existing agents update only application jars: migrating their wrapper requires
stopping them and replacing `boot` and `bin` from a new agent distribution while
retaining configuration. Newly downloaded agents use ServiceWrapper.

Review the standalone project's configuration compatibility list before migrating
custom JSW features. Native SCM, launchd, and systemd acceptance testing is still
required before the initial production release.
