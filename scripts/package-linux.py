#!/usr/bin/env python3
"""Bundle a native Linux release binary, its media runtime and desktop files."""

import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib

ROOT = Path(__file__).resolve().parent.parent
SYSTEM = re.compile(r"^(?:linux-vdso|ld-linux|lib(?:c|m|pthread|dl|rt|resolv|util|anl|mvec|nss_[\w]+))\.so(?:\.|$)")
PLUGINS = """coreelements app playback typefindfunctions videoconvertscale videorate
audioconvert audioresample audiomixer volume inter audioparsers isomp4 matroska
avi ogg libav flac opus vorbis wavparse wavenc mpg123 jpeg png autodetect rtmp
videoparsersbad flv x264 videotestsrc audiotestsrc shout2 alsa pulseaudio video4linux2 rtp rtpmanager webrtc dtls srtp sctp nice""".split()
# Winit/wgpu can load these at runtime rather than through DT_NEEDED.
DISPLAY_LIBS = """libxkbcommon.so.0 libX11.so.6 libXcursor.so.1 libXrandr.so.2
libXi.so.6 libXext.so.6 libwayland-client.so.0 libwayland-cursor.so.0 libwayland-egl.so.1""".split()


def run(*args, **kwargs):
    try:
        return subprocess.run([str(arg) for arg in args], check=True, text=True, **kwargs)
    except subprocess.CalledProcessError as error:
        # Captured launcher diagnostics must reach CI logs when verification fails.
        if error.stdout:
            print(error.stdout, flush=True)
        if error.stderr:
            print(error.stderr, flush=True)
        raise


def output(*args):
    return run(*args, stdout=subprocess.PIPE).stdout.strip()


def dependencies(path):
    found = {}
    for line in output("ldd", path).splitlines():
        if "=> not found" in line:
            raise RuntimeError(f"unresolved library in {path}: {line}")
        match = re.match(r"\s*(\S+)\s+=>\s+(.+?)\s+\(", line)
        if match and not SYSTEM.match(match[1]):
            found[match[1]] = Path(match[2]).resolve()
    for name in output("patchelf", "--print-needed", path).splitlines():
        if name.startswith("/") and not SYSTEM.match(Path(name).name):
            found[name] = Path(name).resolve()
    return found


class Bundle:
    def __init__(self, directory):
        self.directory = directory
        self.files = {}
        self.names = {}
        self.minimum_glibc = (2, 0)
        for folder in ("bin", "lib", "lib/gstreamer-1.0", "libexec", "share/applications", "share/licenses"):
            (directory / folder).mkdir(parents=True, exist_ok=True)

    def add(self, source, destination=None):
        source = source.resolve()
        if source in self.files:
            return self.files[source]
        if source.name.lower().startswith("libndi"):
            raise RuntimeError("install the optional NDI SDK separately; it is not bundled automatically")
        if destination is None:
            soname = output("patchelf", "--print-soname", source)
            destination = self.directory / "lib" / (soname or source.name)
        if destination in self.names and self.names[destination] != source:
            raise RuntimeError(f"library filename collision: {destination}")
        self.names[destination] = source
        self.files[source] = destination
        shutil.copyfile(source, destination)
        destination.chmod(0o755)
        for version in re.findall(r"\bGLIBC_(\d+(?:\.\d+)+)", output("readelf", "--version-info", source)):
            self.minimum_glibc = max(self.minimum_glibc, tuple(map(int, version.split("."))))
        for name, dependency in dependencies(source).items():
            target = self.add(dependency)
            if name.startswith("/"):
                run("patchelf", "--replace-needed", name, target.name, destination)
        relative = os.path.relpath(self.directory / "lib", destination.parent)
        run("patchelf", "--set-rpath", f"$ORIGIN/{relative}", destination)
        return destination

    def audit(self):
        for destination in self.files.values():
            for name, dependency in dependencies(destination).items():
                if not dependency.is_relative_to(self.directory.resolve()):
                    raise RuntimeError(f"external dependency remains in {destination}: {name} => {dependency}")


def notices(bundle):
    packages = {}
    for source in bundle.files:
        result = subprocess.run(["dpkg-query", "-S", str(source)], text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
        if result.returncode:
            continue
        package = result.stdout.split(": ", 1)[0].split(", ")[0]
        packages[package] = output("dpkg-query", "-W", "-f=${Version}", package)
        copyright = Path("/usr/share/doc") / package.split(":")[0] / "copyright"
        if copyright.is_file():
            shutil.copyfile(copyright, bundle.directory / "share/licenses" / f"{package.replace(':', '_')}.copyright")
    (bundle.directory / "share/THIRD_PARTY.json").write_text(json.dumps({"debian_packages":packages,
        "files":[{"file":str(dest.relative_to(bundle.directory)), "source":str(source)} for source,dest in bundle.files.items()]},indent=2)+"\n")
    for name in ("LICENSE", "README.md"):
        shutil.copyfile(ROOT / name, bundle.directory / "share" / name)
    shutil.copyfile(ROOT / "crates/freeshow-output/assets/LICENSE-DejaVu", bundle.directory / "share/licenses/LICENSE-DejaVu")
    (bundle.directory / "share/ICON-SOURCE.txt").write_text("FreeShow icon, GPL-3.0-only.\nSource: https://github.com/ChurchApps/FreeShow/blob/ff784eb388c7baef60d67e6d4e7c504f4322bca3/public/icon.png\n")


def verify(directory):
    with tempfile.TemporaryDirectory(prefix="freeshow-linux-check-") as temporary:
        temporary = Path(temporary)
        relocated = temporary / 'FreeShow Linux "$`% प्रस्तुति'
        shutil.copytree(directory, relocated)
        home = temporary / "home"
        home.mkdir()
        environment = {"HOME":str(home), "PATH":"/usr/bin:/bin", "TMPDIR":str(temporary), "LANG":"C.UTF-8"}
        launcher = relocated / "AppRun"
        run(launcher, "--check-runtime", env=environment, timeout=60)
        run(launcher, "--check-media", ROOT / "crates/freeshow-data/tests/fixtures/lesson-clip.mp4", env=environment, timeout=60)
        run(launcher, "--check-perf", env=environment, timeout=180)
        run(relocated / "install.sh", env=environment, timeout=30)
        desktop = home / ".local/share/applications/org.freeshow.rust.desktop"
        run("desktop-file-validate", desktop)
        run(home / ".local/bin/freeshow-rust", "--check-runtime", env=environment, timeout=60)
        # Exercise the actual freedesktop parser while keeping the app headless.
        probe = temporary / "probe.desktop"
        probe.write_text(desktop.read_text().replace(" --show %f", " --check-runtime"))
        launched = run("gio", "launch", probe, env=environment, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=60)
        if '"missing":[]' not in launched.stdout:
            raise RuntimeError(f"desktop launcher did not run its headless check: {launched.stdout} {launched.stderr}")
        library = home / "Documents/FreeShow/Shows"
        library.mkdir(parents=True)
        sentinel = library / "keep.show"
        sentinel.write_text("user library must survive desktop removal")
        run(relocated / "install.sh", "--uninstall", env=environment, timeout=30)
        if desktop.exists() or (home / ".local/bin/freeshow-rust").is_symlink() or not sentinel.is_file():
            raise RuntimeError("desktop uninstall removed user data or left a launcher behind")
    return {"relocated_special_character_path":True,"empty_home_and_registry":True,"h264_decoded":True,"check_perf":True,
        "desktop_entry_validated_and_launched":True,"uninstall_keeps_library":True}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if platform.system() != "Linux":
        parser.error("Linux packaging requires a native Linux host")
    if output("git", "-C", ROOT, "status", "--porcelain"):
        raise RuntimeError("use a clean committed checkout so the source archive matches the build")
    architecture = platform.machine()
    machine = output("readelf", "-h", args.binary)
    expected = {"x86_64":"Advanced Micro Devices X86-64", "aarch64":"AArch64"}.get(architecture)
    if expected is None or expected not in machine:
        raise RuntimeError("build and package on the same supported native Linux architecture")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    directory = args.output.resolve() / f"freeshow-rust-{version}-linux-{architecture}"
    if directory.exists():
        shutil.rmtree(directory)
    bundle = Bundle(directory)
    bundle.add(args.binary, directory / "bin/freeshow")
    plugins = Path(output("pkg-config", "--variable=pluginsdir", "gstreamer-1.0"))
    for name in PLUGINS + (["ndi"] if (plugins / "libgstndi.so").is_file() else []):
        bundle.add(plugins / f"libgst{name}.so", directory / "lib/gstreamer-1.0" / f"libgst{name}.so")
    scanner = Path(output("pkg-config", "--variable=libexecdir", "gstreamer-1.0")) / "gstreamer-1.0/gst-plugin-scanner"
    bundle.add(scanner, directory / "libexec/gst-plugin-scanner")
    libraries = output("ldconfig", "-p")
    for name in DISPLAY_LIBS:
        matches = re.findall(rf"^\s*{re.escape(name)} .* => (.+)$", libraries, re.M)
        if not matches:
            raise RuntimeError(f"required desktop library is missing: {name}")
        bundle.add(Path(matches[0]))
    bundle.audit()
    notices(bundle)
    for name in ("AppRun", "install.sh"):
        shutil.copyfile(ROOT / "scripts/linux" / name, directory / name)
        (directory / name).chmod(0o755)
    shutil.copyfile(ROOT / "assets/org.freeshow.rust.desktop", directory / "share/applications/org.freeshow.rust.desktop")
    icon = directory / "share/icons/hicolor/512x512/apps/freeshow-rust.png"
    icon.parent.mkdir(parents=True)
    shutil.copyfile(ROOT / "assets/freeshow.png", icon)
    run("git", "-C", ROOT, "archive", "--format=tar.gz", "--output", directory / "share/freeshow-rust-source.tar.gz", "HEAD")
    build = {"version":version,"architecture":architecture,"minimum_glibc":".".join(map(str,bundle.minimum_glibc)),
        "commit":output("git","-C",ROOT,"rev-parse","HEAD"),"source_tree":output("git","-C",ROOT,"rev-parse","HEAD^{tree}"),
        "rustc":output("rustc","-vV"),"gstreamer":output("pkg-config","--modversion","gstreamer-1.0")}
    (directory / "share/BUILD.json").write_text(json.dumps(build,indent=2)+"\n")
    result = verify(directory)
    (args.output / "linux-verification.json").write_text(json.dumps(result,indent=2)+"\n")
    archive = directory.parent / f"{directory.name}.tar.gz"
    with tarfile.open(archive,"w:gz") as handle:
        handle.add(directory,arcname=directory.name)
    with archive.open("rb") as handle:
        digest = hashlib.file_digest(handle,"sha256").hexdigest()
    archive.with_suffix(".gz.sha256").write_text(f"{digest}  {archive.name}\n")
    print(archive)


if __name__ == "__main__":
    main()
