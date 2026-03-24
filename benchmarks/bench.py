"""
Quick benchmark script using wrk (must be installed).
Start the server first:  python examples/hello.py
Then run:                 python benchmarks/bench.py
"""

import subprocess
import shutil
import sys


def run_wrk(url: str, threads: int = 4, connections: int = 256, duration: str = "10s"):
    wrk = shutil.which("wrk")
    if not wrk:
        print("ERROR: 'wrk' not found. Install it first:")
        print("  macOS:  brew install wrk")
        print("  Linux:  apt install wrk")
        sys.exit(1)

    cmd = [wrk, f"-t{threads}", f"-c{connections}", f"-d{duration}", url]
    print(f"\n>>> {' '.join(cmd)}\n")
    subprocess.run(cmd)


if __name__ == "__main__":
    base = "http://127.0.0.1:8000"

    print("=" * 60)
    print("  SkyTrade Engine Benchmark")
    print("=" * 60)

    print("\n--- GET / (plain text) ---")
    run_wrk(f"{base}/")

    print("\n--- GET /hello/bench (JSON-like) ---")
    run_wrk(f"{base}/hello/bench")
