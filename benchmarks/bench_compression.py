"""Pyronova compression benchmark — TechEmpower / HTTP Arena shape.

Profile: "JSON Compressed" — returns a ~3-4 KB JSON payload representing a
realistic web response (32 fortunes-style records with enough variety for
non-trivial compression). Mirrors the TechEmpower Fortunes payload shape.

Usage (toggled by env var so sub-interpreter replays inherit the config):
    PYRONOVA_COMPRESSION=0 python benchmarks/bench_compression.py
    PYRONOVA_COMPRESSION=1 python benchmarks/bench_compression.py

Driver:
    bash benchmarks/run_compression_bench.sh

The server runs on 127.0.0.1:8001 to avoid colliding with bench_plaintext.py
on 8000.
"""

import os

from pyronova import Pyronova

# 32 records × realistic English text ≈ 3–4 KB JSON.
# Deliberately varied wording so compression sees real redundancy, not
# degenerate "aaaa..." repetition which would over-state the ratio.
FORTUNES = [
    {"id": 1, "message": "fortune: No such file or directory"},
    {"id": 2, "message": "A computer scientist is someone who fixes things that aren't broken."},
    {"id": 3, "message": "After enough decimal places, nobody gives a damn."},
    {"id": 4, "message": "A bad random number generator: 1, 1, 1, 1, 1, 4.33e+67, 1, 1, 1"},
    {"id": 5, "message": "A computer program does what you tell it to do, not what you want it to do."},
    {"id": 6, "message": "Emacs is a nice operating system, but I prefer UNIX. — Tom Christaensen"},
    {"id": 7, "message": "Any program that runs right is obsolete."},
    {"id": 8, "message": "A list is only as strong as its weakest link. — Donald Knuth"},
    {"id": 9, "message": "Feature: A bug with seniority."},
    {"id": 10, "message": "Computers make very fast, very accurate mistakes."},
    {"id": 11, "message": "<script>alert(\"This should not be displayed in a browser alert box.\");</script>"},
    {"id": 12, "message": "フレームワークのベンチマーク"},
    {"id": 13, "message": "Additional fortune added at request time."},
    {"id": 14, "message": "Good programmers have a solid grasp of their tools."},
    {"id": 15, "message": "The only constant is change."},
    {"id": 16, "message": "Premature optimization is the root of all evil. — Donald Knuth"},
    {"id": 17, "message": "There are only two hard things in Computer Science: cache invalidation and naming things."},
    {"id": 18, "message": "Testing shows the presence, not the absence of bugs. — Edsger Dijkstra"},
    {"id": 19, "message": "Simplicity is prerequisite for reliability. — Edsger Dijkstra"},
    {"id": 20, "message": "When in doubt, use brute force. — Ken Thompson"},
    {"id": 21, "message": "Controlling complexity is the essence of computer programming. — Brian Kernighan"},
    {"id": 22, "message": "The most important property of a program is whether it accomplishes the intention of its user."},
    {"id": 23, "message": "Measuring programming progress by lines of code is like measuring aircraft building progress by weight."},
    {"id": 24, "message": "The best performance improvement is the transition from the nonworking state to the working state."},
    {"id": 25, "message": "Deleted code is debugged code. — Jeff Sickel"},
    {"id": 26, "message": "First, solve the problem. Then, write the code. — John Johnson"},
    {"id": 27, "message": "Programs must be written for people to read, and only incidentally for machines to execute."},
    {"id": 28, "message": "Any sufficiently advanced bug is indistinguishable from a feature."},
    {"id": 29, "message": "There's no place like 127.0.0.1."},
    {"id": 30, "message": "It is practically impossible to teach good programming to students who have had a prior exposure to BASIC."},
    {"id": 31, "message": "Walking on water and developing software from a specification are easy if both are frozen."},
    {"id": 32, "message": "Debugging is twice as hard as writing the code in the first place."},
]

app = Pyronova()

if os.environ.get("PYRONOVA_COMPRESSION") == "1":
    # Sub-interpreter replay uses a mock Pyronova that doesn't expose this method.
    # Only the main interp actually serves responses, so the no-op in sub-interp
    # is fine — the global compression flag is set once from the main interp.
    enable = getattr(app, "enable_compression", None)
    if callable(enable):
        enable(min_size=256)


@app.get("/")
def index(req):
    # TFB-style plaintext probe route.
    return "Hello from Pyronova!"


@app.get("/json-fortunes")
def fortunes(req):
    return {"fortunes": FORTUNES}


if __name__ == "__main__":
    host = os.environ.get("PYRONOVA_HOST", "127.0.0.1")
    port = int(os.environ.get("PYRONOVA_PORT", "8001"))
    app.run(host=host, port=port)
