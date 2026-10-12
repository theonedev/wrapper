#!/usr/bin/env python3
"""Verify actual Maven packaging outputs, including incremental JSW cleanup."""
import hashlib
import io
from pathlib import Path
import sys
import zipfile

inputs = Path(sys.argv[2]) if len(sys.argv) > 2 else Path(__file__).resolve().parents[3] / "server/server-product/wrapper"
archive = Path(sys.argv[1])
agent = archive.parent / "agent"
with zipfile.ZipFile(archive) as release:
    names = release.namelist()
    prefix = names[0].split("/")[0] + "/"
    expected = sorted((inputs / "boot").iterdir())
    for source in expected:
        path = prefix + "boot/" + source.name
        assert release.read(path) == source.read_bytes(), path
        assert (agent / "boot" / source.name).read_bytes() == source.read_bytes(), source.name
        if source.name != "wrapper.jar":
            assert (release.getinfo(path).external_attr >> 16) & 0o111, path
    for path in names:
        assert "libwrapper-" not in path and "wrapper-license.conf" not in path, path
    for name in ("server", "upgrade", "backup-db", "restore-db", "reset-admin-password"):
        mode = b"--script" if name == "server" else b"--command"
        assert mode in release.read(prefix + "bin/" + name + ".sh")
        assert mode in release.read(prefix + "bin/" + name + ".bat")
    for path in (prefix + "conf/wrapper.conf", prefix + "agent/conf/wrapper.conf"):
        config = release.read(path)
        assert b"@bootstrap.class@" not in config and b"@app.name@" not in config, path
    assert "agentVersion" not in (agent / "conf/wrapper.conf").read_text()
    assert not (agent / "conf/wrapper-license.conf").exists()
    assert not list((agent / "boot").glob("libwrapper-*"))
    # A previous incremental product jar accidentally embedded the populated sandbox,
    # including JSW and previous product jars. The development payload must be lean.
    product = next(n for n in names if n.startswith(prefix + "lib/io.onedev.server-product-") and n.endswith(".jar"))
    with zipfile.ZipFile(io.BytesIO(release.read(product))) as product_jar:
        for name in product_jar.namelist():
            assert not name.startswith(("sandbox/lib/", "sandbox/boot/wrapper", "sandbox/boot/libwrapper", "sandbox/bin/")), name
        assert "sandbox/release.properties" in product_jar.namelist()
for line in (inputs / "SHA256SUMS").read_text().splitlines():
    digest, name = line.split("  ", 1)
    assert hashlib.sha256((inputs / name).read_bytes()).hexdigest() == digest, name
print("Server archive, agent distribution, embedded product jar, permissions, and checksums verified")
