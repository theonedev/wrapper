#!/usr/bin/env python3
"""Assemble a generic Java service distribution."""
import argparse
import hashlib
import json
from pathlib import Path
import platform
import shutil
import struct
import subprocess
import tarfile
import tomllib
import zipfile

ROOT = Path(__file__).resolve().parent
TARGETS = {
    "wrapper-linux-x86-64": ("elf", 62),
    "wrapper-linux-arm-64": ("elf", 183),
    "wrapper-macosx-arm-64": ("macho", 0x100000C),
    "wrapper-windows-x86-64.exe": ("pe", 0x8664),
    "wrapper-windows-arm-64.exe": ("pe", 0xAA64),
}
HOSTS = {
    ("Linux", "x86_64"): "wrapper-linux-x86-64",
    ("Linux", "aarch64"): "wrapper-linux-arm-64",
    ("Linux", "arm64"): "wrapper-linux-arm-64",
    ("Darwin", "arm64"): "wrapper-macosx-arm-64",
    ("Windows", "amd64"): "wrapper-windows-x86-64.exe",
    ("Windows", "arm64"): "wrapper-windows-arm-64.exe",
}
RUST_TARGETS = {
    "wrapper-linux-x86-64": "x86_64-unknown-linux-musl",
    "wrapper-linux-arm-64": "aarch64-unknown-linux-musl",
    "wrapper-macosx-arm-64": "aarch64-apple-darwin",
    "wrapper-windows-x86-64.exe": "x86_64-pc-windows-msvc",
    "wrapper-windows-arm-64.exe": "aarch64-pc-windows-msvc",
}


def verify(path, kind, machine):
    data = path.read_bytes()
    if kind == "elf":
        assert data[:6] == b"\x7fELF\x02\x01" and struct.unpack_from("<H", data, 18)[0] == machine, path
    elif kind == "macho":
        assert data[:4] == b"\xcf\xfa\xed\xfe" and struct.unpack_from("<I", data, 4)[0] == machine, path
    else:
        offset = struct.unpack_from("<I", data, 0x3C)[0]
        assert data[offset:offset + 4] == b"PE\x00\x00" and struct.unpack_from("<H", data, offset + 4)[0] == machine, path


def notices(targets):
    packages = {}
    for target in (RUST_TARGETS[name] for name in targets):
        metadata = json.loads(subprocess.check_output(["cargo", "metadata", "--locked", "--offline", "--format-version", "1", "--filter-platform", target], cwd=ROOT))
        packages.update({p["id"]: p for p in metadata["packages"]})
    parts = [(ROOT / "LICENSE").read_text()]
    for package in sorted(packages.values(), key=lambda p: (p["name"], p["version"])):
        if package["name"] == "wrapper":
            continue
        directory = Path(package["manifest_path"]).parent
        texts = []
        for pattern in ("LICENSE*", "LICENCE*", "COPYING*", "license*", "licence*"):
            for file in sorted(directory.glob(pattern)):
                if file.is_file():
                    content = file.read_text(errors="replace")
                    if content not in texts:
                        texts.append(content)
        if not texts:
            vendored = ROOT / "licenses" / f"{package['name']}-{package['version']}.txt"
            if not vendored.is_file():
                raise RuntimeError(f"Missing license text for {package['name']}; vendor its upstream license in {vendored}")
            texts.append(vendored.read_text())
        parts.append(f"\n{'=' * 72}\n{package['name']} {package['version']} ({package['license']})\n" + "\n".join(texts))
    return "\n".join(parts)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--destination", type=Path)
    parser.add_argument("--archive", type=Path, help="Also write a versioned tar.gz and its SHA-256 here")
    parser.add_argument("--host-only", action="store_true", help="Package only this host's executable for local integration testing")
    args = parser.parse_args()
    source = ROOT / "dist/boot"
    destination = (args.destination or ROOT / "dist/release/wrapper").resolve()
    # A fresh tree also prevents stale platforms and example outputs in releases.
    if destination.exists():
        parser.error(f"Destination already exists: {destination}; remove the previous generated release or use --destination")
    targets = TARGETS
    if args.host_only:
        host = HOSTS.get((platform.system(), platform.machine().lower()))
        if host is None:
            parser.error(f"Unsupported host: {platform.system()} {platform.machine()}")
        targets = {host: TARGETS[host]}
    for name, (kind, machine) in targets.items():
        verify(source / name, kind, machine)
    with zipfile.ZipFile(source / "wrapper.jar") as jar:
        assert "io/onedev/wrapper/WrapperManager.class" in jar.namelist()
    license_text = notices(targets)
    boot = destination / "boot"
    boot.mkdir(parents=True, exist_ok=True)
    for name in [*targets, "wrapper.jar"]:
        shutil.copy2(source / name, boot / name)
        (boot / name).chmod(0o644 if name.endswith(".jar") else 0o755)
    # Keep dependency license notices with the bridge in application distributions.
    with zipfile.ZipFile(boot / "wrapper.jar", "a", compression=zipfile.ZIP_DEFLATED) as jar:
        info = zipfile.ZipInfo("META-INF/THIRD-PARTY-LICENSES.txt", date_time=(2026, 1, 1, 0, 0, 0))
        info.compress_type = zipfile.ZIP_DEFLATED
        jar.writestr(info, license_text)
    for name in ("bin", "conf", "lib", "logs"):
        (destination / name).mkdir(exist_ok=True)
    shutil.copy2(ROOT / "resources/App.sh.in", destination / "bin/service.sh")
    (destination / "bin/service.sh").chmod(0o755)
    shutil.copy2(ROOT / "resources/AppCommand.bat.in", destination / "bin/service.bat")
    shutil.copy2(ROOT / "resources/wrapper.conf", destination / "conf/wrapper.conf")
    for name in ("LICENSE", "README.md", "CONTRIBUTING.md"):
        shutil.copy2(ROOT / name, destination / name)
    for name in ("docs", "examples"):
        shutil.copytree(ROOT / name, destination / name,
                        ignore=shutil.ignore_patterns("build", "__pycache__"))
    (destination / "THIRD-PARTY-LICENSES.txt").write_text(license_text, encoding="utf-8")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["package"]["version"]
    (destination / "VERSION").write_text(version + "\n")
    (destination / "SHA256SUMS").write_text("".join(
        f"{hashlib.sha256(file.read_bytes()).hexdigest()}  {file.relative_to(destination).as_posix()}\n"
        for file in sorted(destination.rglob("*")) if file.is_file() and file.name != "SHA256SUMS"))
    if args.archive:
        args.archive.mkdir(parents=True, exist_ok=True)
        name = f"wrapper-{version}"
        if args.host_only:
            name += "-" + RUST_TARGETS[host]
        archive = args.archive / (name + ".tar.gz")
        with tarfile.open(archive, "w:gz") as output:
            output.add(destination, arcname=name)
        archive.with_suffix(archive.suffix + ".sha256").write_text(
            f"{hashlib.sha256(archive.read_bytes()).hexdigest()}  {archive.name}\n")
    print(f"Packaged Wrapper {version} for {len(targets)} target(s) in {destination}")


if __name__ == "__main__":
    main()
