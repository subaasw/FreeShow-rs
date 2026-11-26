"""Media checks shared by the package verifiers."""

from contextlib import contextmanager
from functools import partial
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
import json
import math
import subprocess
from threading import Thread
from urllib.parse import quote


def verify_handoff(*launcher, env, cwd=None):
    """Return the offline service handoff report from the packaged launcher."""
    result = subprocess.run(
        [str(part) for part in launcher] + ["--check-handoff"],
        env=env, cwd=cwd, capture_output=True, text=True, encoding="utf-8", timeout=90,
    )
    if result.returncode:
        raise RuntimeError(f"packaged handoff check failed ({result.returncode})\n{result.stdout}{result.stderr}")
    report = json.loads(result.stdout)
    required = (
        "project_stage_note_preserved", "distinct_same_named_pictures", "video_relinked_and_decoded",
        "audience_cleared", "template_edited_and_reopened_without_private_notes",
    )
    peaks = [report.get(key) for key in ("video_audio_peak", "relinked_video_audio_peak")]
    if not all(report.get(key) is True for key in required) or not all(
        type(peak) in (float, int) and math.isfinite(peak) and peak > 0.01 for peak in peaks
    ):
        raise RuntimeError(f"packaged handoff check returned incomplete evidence: {report}")
    return report


@contextmanager
def serve_sample(sample):
    handler = partial(SimpleHTTPRequestHandler, directory=sample.parent)
    with ThreadingHTTPServer(("127.0.0.1", 0), handler) as server:
        worker = Thread(target=server.serve_forever, daemon=True)
        worker.start()
        try:
            yield f"http://127.0.0.1:{server.server_port}/{quote(sample.name)}"
        finally:
            server.shutdown()
            worker.join()
