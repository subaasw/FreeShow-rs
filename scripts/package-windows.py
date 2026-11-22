#!/usr/bin/env python3
"""Build an x64 Windows installer with an application-local media runtime."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
PLUGINS = """coreelements app playback typefindfunctions videoconvertscale
audioconvert audioresample audiomixer volume inter audioparsers isomp4 matroska
avi ogg libav flac opus vorbis wavparse wavenc mpg123 jpeg png
autodetect rtmp videoparsersbad flv x264 videotestsrc audiotestsrc wasapi2""".split()
OPTIONAL_PLUGINS = "mediafoundation directshow ksvideo directsound wasapi".split()
SAMPLE = ROOT / "crates/freeshow-data/tests/fixtures/lesson-clip.mp4"


def run(*args, **kwargs):
    return subprocess.run([str(arg) for arg in args], check=True, text=True, **kwargs)


def output(*args):
    return run(*args, stdout=subprocess.PIPE).stdout.strip()


def x64(path):
    with path.open("rb") as binary:
        if binary.read(2) != b"MZ":
            raise RuntimeError(f"not a Windows executable: {path}")
        binary.seek(0x3C)
        offset = struct.unpack("<I", binary.read(4))[0]
        binary.seek(offset)
        if binary.read(6) != b"PE\0\0\x64\x86":
            raise RuntimeError(f"not an x64 executable: {path}")


def crt_directory():
    vswhere = Path(os.environ["ProgramFiles(x86)"]) / "Microsoft Visual Studio/Installer/vswhere.exe"
    studio = Path(output(vswhere, "-latest", "-products", "*", "-property", "installationPath"))
    candidates = sorted((studio / "VC/Redist/MSVC").glob("*/x64/Microsoft.VC*.CRT"),
                        key=lambda path: tuple(map(int, path.parents[1].name.split("."))))
    if not candidates:
        raise RuntimeError("Visual Studio app-local x64 CRT files were not found")
    return candidates[-1]


class Bundle:
    def __init__(self, sdk, directory):
        self.directory = directory
        self.files = {}
        self.system = set()
        self.objdump = shutil.which("llvm-objdump")
        if not self.objdump:
            raise RuntimeError("llvm-objdump is required for PE dependency discovery")
        self.sdk_dlls = {path.name.lower(): path for path in (sdk / "bin").glob("*.dll")}
        # App-local CRT avoids relying on Visual Studio's globally installed redistributable.
        self.sdk_dlls.update({path.name.lower(): path for path in crt_directory().glob("*.dll")})

    def add(self, source, destination=None):
        destination = destination or self.directory / source.name
        key = destination.relative_to(self.directory).as_posix()
        if key in self.files:
            if self.files[key] != source:
                raise RuntimeError(f"duplicate bundled filename: {destination}")
            return
        x64(source)
        self.files[key] = source
        destination.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(source, destination)
        imports = output(self.objdump, "--private-headers", source)
        for name in re.findall(r"^\s*DLL Name:\s*(\S+)", imports, re.MULTILINE):
            self.require(name.lower(), source)

    def require(self, name, source):
        if name in self.sdk_dlls:
            self.add(self.sdk_dlls[name])
            return
        system = Path(os.environ["SystemRoot"]) / "System32" / name
        if name.startswith(("api-ms-win-", "ext-ms-win-")) or system.is_file():
            self.system.add(name)
            return
        raise RuntimeError(f"missing {name} required by {source}")


def bundle_payload(binary, sdk, payload):
    bundle = Bundle(sdk, payload)
    bundle.add(binary, payload / "freeshow.exe")
    scanner = next(iter(sdk.rglob("gst-plugin-scanner.exe")), None)
    if scanner is None:
        raise RuntimeError("the GStreamer SDK is missing gst-plugin-scanner.exe")
    # Keep the scanner next to the same DLLs as the presenter executable.
    bundle.add(scanner, payload / scanner.name)
    plugin_dir = sdk / "lib/gstreamer-1.0"
    for name in PLUGINS + OPTIONAL_PLUGINS:
        plugin = next((p for p in (plugin_dir / f"gst{name}.dll",
                                  plugin_dir / f"libgst{name}.dll") if p.is_file()), None)
        if plugin is not None:
            bundle.add(plugin, payload / "lib/gstreamer-1.0" / plugin.name)
        elif name in PLUGINS:
            raise RuntimeError(f"required GStreamer plugin missing from SDK: {name}")
    share = payload / "share"
    share.mkdir()
    for name in ("LICENSE", "README.md"):
        shutil.copyfile(ROOT / name, share / name)
    shutil.copyfile(ROOT / "assets/freeshow.ico", payload / "freeshow.ico")
    shutil.copyfile(ROOT / "crates/freeshow-output/assets/LICENSE-DejaVu", share / "LICENSE-DejaVu")
    for path in sdk.rglob("*"):
        if not path.is_file():
            continue
        relative = path.relative_to(sdk)
        if path.name.upper().startswith(("COPYING", "LICENSE", "NOTICE")) or any(
            part.lower() in ("license", "licenses") for part in relative.parts[:-1]
        ):
            target = share / "licenses" / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(path, target)
    (share / "THIRD_PARTY.json").write_text(json.dumps({
        "gstreamer_source": "https://gstreamer.freedesktop.org/src/",
        "gstreamer_build_recipes": "https://gitlab.freedesktop.org/gstreamer/cerbero",
        "icon_source": "https://github.com/ChurchApps/FreeShow/blob/ff784eb388c7baef60d67e6d4e7c504f4322bca3/public/icon.ico",
        "bundled_files": sorted(bundle.files),
        "windows_imports": sorted(bundle.system),
    }, indent=2) + "\n", encoding="utf-8")
    return share


def verify(installer, sdk):
    if os.environ.get("GITHUB_ACTIONS") != "true":
        raise RuntimeError("installer verification must run in an isolated Windows CI account")
    with tempfile.TemporaryDirectory(prefix="FreeShow relocation ") as temporary:
        temp = Path(temporary)
        installed = temp / "FreeShow प्रस्तुति"
        profile = temp / "Fresh Profile"
        profile.mkdir()
        sample = temp / "Example clip.mp4"
        shutil.copyfile(SAMPLE, sample)
        library = temp / "Library/Shows"
        library.mkdir(parents=True)
        sentinel = library / "keep.show"
        sentinel.write_text("keep this library file", encoding="utf-8")
        switches = ["/VERYSILENT", "/SUPPRESSMSGBOXES", "/NORESTART"]
        run(installer, "/SP-", *switches, f"/DIR={installed}", timeout=180)
        shortcut = Path(os.environ["APPDATA"]) / "Microsoft/Windows/Start Menu/Programs/FreeShow Rust.lnk"
        if not shortcut.is_file():
            raise RuntimeError("Start menu shortcut was not installed")
        hidden = sdk.with_name(sdk.name + "-disabled")
        sdk.rename(hidden)
        try:
            env = {key: value for key, value in os.environ.items()
                   if not key.upper().startswith(("GST_", "GSTREAMER_", "FREESHOW_"))}
            env.update(PATH=str(Path(os.environ["SystemRoot"]) / "System32"),
                       LOCALAPPDATA=str(profile), APPDATA=str(profile), USERPROFILE=str(profile))
            app = installed / "freeshow.exe"
            runtime = run(app, "--check-runtime", env=env, cwd=temp, stdout=subprocess.PIPE, timeout=90)
            decode = run(app, "--check-media", sample, env=env, cwd=temp, stdout=subprocess.PIPE, timeout=90)
            assert json.loads(runtime.stdout)["missing"] == [], runtime.stdout
            assert json.loads(decode.stdout.splitlines()[-1])["decoded_video"], decode.stdout
            assert list(profile.rglob("registry-*.bin")), "bundled runtime did not create a cold registry"
        finally:
            hidden.rename(sdk)
        run(installed / "unins000.exe", *switches, timeout=180)
        assert not app.exists() and not shortcut.exists()
        assert sentinel.read_text(encoding="utf-8") == "keep this library file"
    return {"installed_in_relocated_unicode_path": True, "sdk_hidden": True,
            "cold_registry": True, "h264_decoded": True, "uninstall_kept_library": True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--gst", type=Path, required=True)
    parser.add_argument("--binary", type=Path, default=ROOT / "target/release/freeshow.exe")
    parser.add_argument("--output", type=Path, default=ROOT / "dist/freeshow-windows-x86_64")
    parser.add_argument("--verify", action="store_true")
    args = parser.parse_args()
    if sys.platform != "win32":
        parser.error("Windows packaging requires a native Windows host")
    if output("git", "-C", ROOT, "status", "--porcelain"):
        raise RuntimeError("use a clean committed checkout so the source archive matches the build")
    sdk = args.gst.resolve()
    destination = args.output.resolve()
    destination.mkdir(parents=True, exist_ok=True)
    version = tomllib.loads((ROOT / "Cargo.toml").read_text(encoding="utf-8"))["workspace"]["package"]["version"]
    commit = output("git", "-C", ROOT, "rev-parse", "HEAD")
    iscc = shutil.which("ISCC.exe") or Path(os.environ["ProgramFiles(x86)"]) / "Inno Setup 6/ISCC.exe"
    with tempfile.TemporaryDirectory(prefix="freeshow-payload-") as temp:
        payload = Path(temp) / "payload"
        share = bundle_payload(args.binary.resolve(), sdk, payload)
        (share / "windows-build.json").write_text(json.dumps({
            "version": version,
            "architecture": "x86_64",
            "commit": commit,
            "source_tree": output("git", "-C", ROOT, "rev-parse", "HEAD^{tree}"),
            "rustc": output("rustc", "-vV"),
            "gstreamer": output(sdk / "bin/gst-inspect-1.0.exe", "--version"),
        }, indent=2) + "\n", encoding="utf-8")
        run("git", "-C", ROOT, "archive", "--format=tar.gz", "--output",
            share / "freeshow-rust-source.tar.gz", commit)
        run(iscc, f"/DAppVersion={version}", f"/DPayload={payload}", f"/O{destination}",
            ROOT / "scripts/windows/installer.iss", timeout=600)
    installer = destination / f"freeshow-rust-{version}-windows-x86_64-setup.exe"
    if args.verify:
        report = verify(installer, sdk)
        (destination / "windows-verification.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print("::notice::" + json.dumps(report), flush=True)
    with installer.open("rb") as handle:
        digest = hashlib.file_digest(handle, "sha256").hexdigest()
    installer.with_suffix(".exe.sha256").write_text(f"{digest}  {installer.name}\n", encoding="utf-8")
    print(installer)


if __name__ == "__main__":
    main()
