#!/usr/bin/env python3
"""Build and verify a relocatable macOS app/DMG from a native release binary."""

import argparse
import gzip
import hashlib
import json
import os
from pathlib import Path
import plistlib
import re
import shutil
import subprocess
import tempfile
import tomllib

from package_media import serve_sample, verify_handoff

ROOT = Path(__file__).resolve().parent.parent
SYSTEM = ("/usr/lib/", "/System/Library/")
# These cover the application's local video, camera, mixer, recording and RTMP
# paths. Optional NDI still needs a separately installed NDI SDK runtime.
PLUGINS = """coreelements app playback typefindfunctions videoconvertscale videorate
audioconvert audioresample audiomixer volume inter audioparsers isomp4 matroska
avi ogg libav flac opus vorbis wavparse wavenc mpg123 jpeg png osxaudio applemedia curl
autodetect rtmp videoparsersbad flv x264 videotestsrc audiotestsrc rtp rtpmanager webrtc dtls srtp sctp nice""".split()


def run(*args, **kwargs):
    return subprocess.run([str(arg) for arg in args], check=True, text=True, **kwargs)


def output(*args):
    return run(*args, stdout=subprocess.PIPE).stdout.strip()


def commands(path, arch):
    """Read load commands, excluding LC_ID_DYLIB's self-reference."""
    loads, rpaths, minimum = [], [], (11, 0)
    dylib = False
    for block in re.split(r"Load command \d+\n", output("otool", "-arch", arch, "-l", path)):
        match = re.search(r"^\s*cmd (LC_\w+)$", block, re.M)
        if not match:
            continue
        kind = match[1]
        if kind in ("LC_LOAD_DYLIB", "LC_LOAD_WEAK_DYLIB", "LC_REEXPORT_DYLIB", "LC_LOAD_UPWARD_DYLIB"):
            loads.append(re.search(r"^\s*name (.+) \(offset \d+\)$", block, re.M)[1])
        elif kind == "LC_RPATH":
            rpaths.append(re.search(r"^\s*path (.+) \(offset \d+\)$", block, re.M)[1])
        elif kind == "LC_ID_DYLIB":
            dylib = True
        elif kind in ("LC_BUILD_VERSION", "LC_VERSION_MIN_MACOSX"):
            version = re.search(r"^\s*(?:minos|version) ([\d.]+)$", block, re.M)
            if version:
                minimum = max(minimum, tuple(int(part) for part in version[1].split(".")))
    return loads, rpaths, minimum, dylib


class Bundle:
    def __init__(self, app, binary):
        self.contents = app / "Contents"
        self.binary = binary.resolve()
        arches = output("lipo", "-archs", self.binary).split()
        if len(arches) != 1:
            raise RuntimeError("build one native architecture at a time")
        self.arch = arches[0]
        self.files = {}
        self.names = {}
        self.minimum = (11, 0)
        self.libdir = Path(output("pkg-config", "--variable=libdir", "gstreamer-1.0"))
        self.main_rpaths = commands(self.binary, self.arch)[1]
        for folder in ("MacOS", "Frameworks", "PlugIns", "Helpers", "Resources"):
            (self.contents / folder).mkdir(parents=True, exist_ok=True)

    def resolve(self, name, source, rpaths):
        def expand(value):
            return Path(value.replace("@loader_path", str(source.parent)).replace("@executable_path", str(self.binary.parent)))

        if name.startswith("@rpath/"):
            tail = name.removeprefix("@rpath/")
            candidates = [expand(path) / tail for path in rpaths + self.main_rpaths]
            candidates.append(self.libdir / tail)
        else:
            candidates = [expand(name)]
        for path in candidates:
            if path.is_file():
                return path.resolve()
        raise RuntimeError(f"unresolved dependency {name} in {source}")

    def add(self, source, destination):
        source = source.resolve()
        if source in self.files:
            return self.files[source][0]
        if source.name.lower().startswith("libndi"):
            raise RuntimeError("the optional NDI SDK must be installed separately, not copied into this bundle")
        arches = output("lipo", "-archs", source).split()
        if self.arch not in arches:
            raise RuntimeError(f"{source} has no {self.arch} slice")
        if destination in self.names and self.names[destination] != source:
            raise RuntimeError(f"dependency filename collision: {destination}")
        self.names[destination] = source
        loads, rpaths, minimum, dylib = commands(source, self.arch)
        self.minimum = max(self.minimum, minimum)
        destination.parent.mkdir(parents=True, exist_ok=True)
        if len(arches) > 1:
            run("lipo", source, "-thin", self.arch, "-output", destination)
        else:
            shutil.copyfile(source, destination)
        destination.chmod(0o755)
        subprocess.run(["codesign", "--remove-signature", str(destination)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        self.files[source] = (destination, loads, rpaths, dylib)
        for name in loads:
            if name.startswith(SYSTEM):
                continue
            dependency = self.resolve(name, source, rpaths)
            self.add(dependency, self.contents / "Frameworks" / dependency.name)
        return destination

    def relocate(self):
        for source, (destination, loads, rpaths, dylib) in self.files.items():
            args = ["install_name_tool"]
            for name in loads:
                if name.startswith(SYSTEM):
                    continue
                dependency = self.resolve(name, source, rpaths)
                target = self.files[dependency][0]
                relative = os.path.relpath(target, destination.parent)
                args += ["-change", name, f"@loader_path/{relative}"]
            for path in dict.fromkeys(rpaths):
                args += ["-delete_rpath", path]
            if dylib:
                args += ["-id", f"@rpath/{destination.name}"]
            if len(args) > 1:
                run(*args, destination)
        self.audit()

    def audit(self):
        for destination, _, _, _ in self.files.values():
            loads, rpaths, _, _ = commands(destination, self.arch)
            if rpaths:
                raise RuntimeError(f"unexpected runtime search paths in {destination}")
            for name in loads:
                if name.startswith(SYSTEM):
                    continue
                if not name.startswith("@loader_path/"):
                    raise RuntimeError(f"external library dependency remains: {name}")
                target = (destination.parent / name.removeprefix("@loader_path/")).resolve()
                if not target.is_relative_to(self.contents.resolve()) or not target.is_file():
                    raise RuntimeError(f"library escapes or is missing from the app: {name}")


def source_archive(resources):
    if subprocess.run(["git", "diff", "--quiet"], cwd=ROOT).returncode:
        raise RuntimeError("stage source changes before packaging so the source archive matches the build")
    if output("git", "-C", ROOT, "ls-files", "--others", "--exclude-standard"):
        raise RuntimeError("track or ignore untracked files before packaging")
    tree = output("git", "-C", ROOT, "write-tree")
    archive = subprocess.run(["git", "-C", str(ROOT), "archive", "--format=tar", tree], check=True, stdout=subprocess.PIPE).stdout
    with open(resources / "freeshow-rust-source.tar.gz", "wb") as handle:
        with gzip.GzipFile(fileobj=handle, mode="wb", mtime=int(os.environ.get("SOURCE_DATE_EPOCH", "0"))) as compressed:
            compressed.write(archive)
    return tree


def notices(bundle):
    resources = bundle.contents / "Resources"
    shutil.copy2(ROOT / "LICENSE", resources)
    shutil.copy2(ROOT / "README.md", resources)
    shutil.copy2(ROOT / "crates/freeshow-output/assets/LICENSE-DejaVu", resources)
    shutil.copy2(ROOT / "assets/freeshow.icns", resources)
    (resources / "ICON-SOURCE.txt").write_text("FreeShow icon, GPL-3.0-only.\nSource: https://github.com/ChurchApps/FreeShow/blob/ff784eb388c7baef60d67e6d4e7c504f4322bca3/public/icon.icns\n")
    formulas = {}
    files = []
    for source, (destination, _, _, _) in bundle.files.items():
        files.append({"file":str(destination.relative_to(bundle.contents)), "source":str(source)})
        if "Cellar" not in source.parts:
            continue
        index = source.parts.index("Cellar")
        formula, version = source.parts[index + 1:index + 3]
        prefix = Path(*source.parts[:index + 3])
        formulas[formula] = version
        target = resources / "ThirdParty" / formula
        if target.exists():
            continue
        target.mkdir(parents=True)
        for recipe in (prefix / ".brew").glob("*.rb"):
            shutil.copy2(recipe, target / recipe.name)
        for pattern in ("LICENSE*", "COPYING*", "NOTICE*", "share/licenses"):
            for path in prefix.glob(pattern):
                if path.is_dir():
                    shutil.copytree(path, target / path.name, dirs_exist_ok=True)
                elif path.is_file():
                    shutil.copy2(path, target / path.name)
    (resources / "THIRD_PARTY.json").write_text(json.dumps({"homebrew_formulas":formulas, "files":files}, indent=2) + "\n")


def verify(app):
    with tempfile.TemporaryDirectory(prefix="freeshow relocated ") as temp:
        temp = Path(temp)
        relocated = temp / app.name
        shutil.copytree(app, relocated)
        home = temp / "home"
        home.mkdir()
        sample = temp / "Sample clip.mp4"
        shutil.copy2(ROOT / "crates/freeshow-data/tests/fixtures/lesson-clip.mp4", sample)
        environment = {"HOME":str(home), "PATH":"/usr/bin:/bin", "TMPDIR":str(temp), "LANG":"en_US.UTF-8"}
        prefix = []
        if Path("/usr/bin/sandbox-exec").is_file():
            blocked = ["/opt/homebrew", "/usr/local", str(Path.home() / ".rustup"), str(Path.home() / ".cargo")]
            profile = "(version 1)(allow default)(deny file-read* " + " ".join(f"(subpath {json.dumps(path)})" for path in blocked) + ")"
            prefix = ["/usr/bin/sandbox-exec", "-p", profile]
        executable = relocated / "Contents/MacOS/freeshow"
        for args in (["--check-runtime"], ["--check-media", str(sample)], ["--check-perf"]):
            run(*prefix, executable, *args, env=environment, timeout=90)
        with serve_sample(sample) as url:
            run(*prefix, executable, "--check-media", url, env=environment, timeout=90)
        handoff = verify_handoff(*prefix, executable, env=environment, cwd=temp)
        run("codesign", "--verify", "--deep", "--strict", relocated)
        return {"relocated_path_with_spaces":True, "empty_home_and_registry":True, "homebrew_and_rust_reads_blocked":bool(prefix), "h264_decoded":True, "http_video_decoded":True, "portable_handoff":handoff}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--identity", default="-", help="codesign identity; default is ad-hoc development signing")
    parser.add_argument("--dmg", action="store_true")
    args = parser.parse_args()
    if os.uname().sysname != "Darwin":
        parser.error("macOS packaging requires a native macOS build host")
    args.output.mkdir(parents=True, exist_ok=True)
    app = args.output.resolve() / "FreeShow Rust.app"
    if app.exists():
        shutil.rmtree(app)
    bundle = Bundle(app, args.binary)
    bundle.add(args.binary, bundle.contents / "MacOS/freeshow")
    plugins = Path(output("pkg-config", "--variable=pluginsdir", "gstreamer-1.0"))
    optional = [name for name in ("ndi", "shout2") if (plugins / f"libgst{name}.dylib").is_file()]
    for plugin in PLUGINS + optional:
        source = plugins / f"libgst{plugin}.dylib"
        if not source.is_file():
            raise RuntimeError(f"required GStreamer plugin is missing: {source}")
        bundle.add(source, bundle.contents / "PlugIns" / source.name)
    scanner = Path(output("pkg-config", "--variable=libexecdir", "gstreamer-1.0")) / "gstreamer-1.0/gst-plugin-scanner"
    bundle.add(scanner, bundle.contents / "Helpers/gst-plugin-scanner")
    bundle.relocate()
    notices(bundle)
    tree = source_archive(bundle.contents / "Resources")
    version = tomllib.loads((ROOT / "Cargo.toml").read_text())["workspace"]["package"]["version"]
    minimum = ".".join(str(part) for part in bundle.minimum)
    with open(bundle.contents / "Info.plist", "wb") as handle:
        plistlib.dump({"CFBundleExecutable":"freeshow", "CFBundleIdentifier":"org.freeshow.rust", "CFBundleName":"FreeShow Rust",
            "CFBundleDisplayName":"FreeShow Rust", "CFBundleIconFile":"freeshow.icns", "CFBundlePackageType":"APPL", "CFBundleShortVersionString":version,
            "CFBundleVersion":version, "LSMinimumSystemVersion":minimum, "NSHighResolutionCapable":True,
            "NSCameraUsageDescription":"FreeShow uses the selected camera as a presentation background.",
            "NSMicrophoneUsageDescription":"FreeShow uses the selected audio input for linear timecode.",
            "NSLocalNetworkUsageDescription":"FreeShow provides remote controls, stage views, and presentation media on your local network."}, handle)
    build = {"version":version, "architecture":bundle.arch, "minimum_macos":minimum, "source_tree":tree,
        "commit":output("git", "-C", ROOT, "rev-parse", "HEAD"), "signing_identity":args.identity,
        "rustc":output("rustc", "-vV"), "gstreamer":output("pkg-config", "--modversion", "gstreamer-1.0")}
    (bundle.contents / "Resources/BUILD.json").write_text(json.dumps(build, indent=2) + "\n")
    signing = ["codesign", "--force", "--sign", args.identity]
    signing += ["--timestamp=none"] if args.identity == "-" else ["--options", "runtime", "--timestamp"]
    for destination, _, _, _ in bundle.files.values():
        if destination != bundle.contents / "MacOS/freeshow":
            run(*signing, destination, stdout=subprocess.DEVNULL)
    run(*signing, app)
    result = verify(app)
    if args.dmg:
        with tempfile.TemporaryDirectory(prefix="freeshow-dmg-") as temp:
            staging = Path(temp) / "image"
            staging.mkdir()
            shutil.copytree(app, staging / app.name)
            (staging / "Applications").symlink_to("/Applications")
            shutil.copy2(ROOT / "README.md", staging / "README.md")
            dmg = args.output / f"freeshow-rust-{version}-{bundle.arch}.dmg"
            run("hdiutil", "create", "-ov", "-format", "UDZO", "-volname", "FreeShow Rust", "-srcfolder", staging, dmg)
            run("hdiutil", "verify", dmg)
            mount = Path(temp) / "mounted"
            mount.mkdir()
            run("hdiutil", "attach", "-readonly", "-nobrowse", "-mountpoint", mount, dmg)
            try:
                verify(mount / app.name)
            finally:
                run("hdiutil", "detach", mount)
            result["mounted_dmg_verified"] = True
            with open(dmg, "rb") as handle:
                digest = hashlib.file_digest(handle, "sha256").hexdigest()
            dmg.with_suffix(".dmg.sha256").write_text(f"{digest}  {dmg.name}\n")
    (args.output / "macos-verification.json").write_text(json.dumps(result, indent=2) + "\n")
    print(app)


if __name__ == "__main__":
    main()
