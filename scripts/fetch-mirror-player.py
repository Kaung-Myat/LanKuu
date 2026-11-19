#!/usr/bin/env python3
"""Stage a verified, native ffplay runtime for the desktop installer."""

import hashlib
import io
import os
import platform
import shutil
import stat
import subprocess
import sys
import tarfile
import urllib.request
import zipfile
from pathlib import Path


DESTINATION = Path(
    os.environ.get(
        "LANKUU_MIRROR_DESTINATION", "desktop/src-tauri/resources/mirror-player"
    )
)
BTBN_RELEASE = "autobuild-2026-11-13-12-43"
BTBN_BASE = f"https://github.com/BtbN/FFmpeg-Builds/releases/download/{BTBN_RELEASE}"
WINDOWS_ASSET = "ffmpeg-n8.1.3-35-gf1acb0a266-win64-lgpl-8.1.zip"
WINDOWS_SHA256 = "ea5e260eb3d669aff29e7bfc100e0ed9ea633e37f02fee358f6d164cb946e421"
LINUX_ASSET = "ffmpeg-n8.1.3-35-gf1acb0a266-linux64-lgpl-8.1.tar.xz"
LINUX_SHA256 = "c807f1054031e614e60f11482a8729bc8b442e9bddad3464282c164c88565747"


def download_verified(asset: str, expected_sha256: str) -> bytes:
    url = f"{BTBN_BASE}/{asset}"
    print(f"downloading pinned runtime {url}", flush=True)
    request = urllib.request.Request(url, headers={"User-Agent": "LanKuu release builder"})
    with urllib.request.urlopen(request) as response:
        payload = response.read()
    actual = hashlib.sha256(payload).hexdigest()
    if actual != expected_sha256:
        sys.exit(f"checksum mismatch for {asset}: expected {expected_sha256}, got {actual}")
    print(f"verified sha256 {actual}", flush=True)
    return payload


def reset_destination() -> None:
    DESTINATION.mkdir(parents=True, exist_ok=True)
    for child in DESTINATION.iterdir():
        if child.name == ".gitkeep":
            continue
        if child.is_dir():
            shutil.rmtree(child)
        else:
            child.unlink()


def executable(path: Path) -> None:
    path.chmod(path.stat().st_mode | stat.S_IEXEC | stat.S_IXGRP | stat.S_IXOTH)


def copy_archive_licenses(names, reader) -> None:
    copied = set()
    for member in names:
        name = member.replace("\\", "/").rsplit("/", 1)[-1]
        lowered = name.lower()
        if not lowered.startswith(("license", "copying")) or name in copied:
            continue
        (DESTINATION / name).write_bytes(reader(member))
        copied.add(name)


def write_windows(payload: bytes) -> Path:
    with zipfile.ZipFile(io.BytesIO(payload)) as package:
        members = [name for name in package.namelist() if not name.endswith("/")]
        candidates = [name for name in members if name.replace("\\", "/").endswith("/bin/ffplay.exe")]
        if not candidates:
            sys.exit("ffplay.exe was not in the pinned Windows archive")
        target = DESTINATION / "ffplay.exe"
        target.write_bytes(package.read(candidates[0]))
        copy_archive_licenses(members, package.read)
    return target


def write_linux(payload: bytes) -> Path:
    with tarfile.open(fileobj=io.BytesIO(payload), mode="r:xz") as package:
        members = [member for member in package.getmembers() if member.isfile()]
        candidates = [member for member in members if member.name.endswith("/bin/ffplay")]
        if not candidates:
            sys.exit("ffplay was not in the pinned Linux archive")
        source = package.extractfile(candidates[0])
        if source is None:
            sys.exit("could not read ffplay from the pinned Linux archive")
        target = DESTINATION / "ffplay"
        target.write_bytes(source.read())
        executable(target)
        for member in members:
            name = member.name.rsplit("/", 1)[-1]
            if not name.lower().startswith(("license", "copying")) or (DESTINATION / name).exists():
                continue
            license_source = package.extractfile(member)
            if license_source is not None:
                (DESTINATION / name).write_bytes(license_source.read())
    return target


def dylib_dependencies(path: Path) -> list[Path]:
    output = subprocess.check_output(["otool", "-L", str(path)], text=True)
    dependencies = []
    for line in output.splitlines()[1:]:
        value = line.strip().split(" (compatibility", 1)[0]
        if value.startswith(("/System/Library/", "/usr/lib/", "@")):
            continue
        candidate = Path(value)
        if candidate.is_file():
            dependencies.append(candidate.resolve())
    return dependencies


def write_macos() -> Path:
    prefix = Path(subprocess.check_output(["brew", "--prefix", "ffmpeg@8"], text=True).strip())
    source = prefix / "bin" / "ffplay"
    if not source.is_file():
        sys.exit("Homebrew ffmpeg@8 is required before staging the macOS runtime")

    target = DESTINATION / "ffplay"
    shutil.copy2(source, target)
    executable(target)

    queue = [target]
    copied_by_source = {}
    while queue:
        binary = queue.pop(0)
        for dependency in dylib_dependencies(binary):
            bundled = copied_by_source.get(dependency)
            if bundled is None:
                bundled = DESTINATION / dependency.name
                if bundled.exists():
                    sys.exit(f"duplicate macOS library name while bundling: {dependency.name}")
                shutil.copy2(dependency, bundled)
                copied_by_source[dependency] = bundled
                queue.append(bundled)
            subprocess.run(
                ["install_name_tool", "-change", str(dependency), f"@loader_path/{bundled.name}", str(binary)],
                check=True,
            )
    for library in copied_by_source.values():
        subprocess.run(["install_name_tool", "-id", f"@loader_path/{library.name}", str(library)], check=True)

    expected_arch = "arm64" if platform.machine() == "arm64" else "x86_64"
    info = subprocess.check_output(["lipo", "-info", str(target)], text=True)
    if expected_arch not in info:
        sys.exit(f"staged ffplay is not native {expected_arch}: {info.strip()}")
    (DESTINATION / "RUNTIME-NOTICE.txt").write_text(
        "ffplay and its runtime libraries were bundled from Homebrew ffmpeg@8.\n"
        "See https://ffmpeg.org/legal.html and the installer license notices.\n",
        encoding="utf-8",
    )
    return target


def smoke_test(player: Path) -> None:
    result = subprocess.run(
        [str(player.resolve()), "-version"],
        cwd=DESTINATION,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        timeout=20,
    )
    if result.returncode != 0 or "ffplay version" not in result.stdout.lower():
        sys.exit(f"bundled mirror player smoke test failed:\n{result.stdout}")
    print(result.stdout.splitlines()[0], flush=True)


def main() -> None:
    runner = os.environ.get("RUNNER_OS") or platform.system()
    reset_destination()
    if runner == "Windows":
        player = write_windows(download_verified(WINDOWS_ASSET, WINDOWS_SHA256))
    elif runner == "Linux":
        player = write_linux(download_verified(LINUX_ASSET, LINUX_SHA256))
    elif runner in ("macOS", "Darwin"):
        player = write_macos()
    else:
        sys.exit(f"unsupported release platform: {runner}")
    smoke_test(player)


if __name__ == "__main__":
    main()
