#!/usr/bin/env python3
"""Builds each benchmark in Ruach and in C, checks that the outputs agree,
and prints the best wall-clock time of several runs."""
import os, subprocess, sys, tempfile, time

HERE = os.path.dirname(os.path.abspath(__file__))
RUACH = os.environ.get("RUACH", os.path.join(HERE, "..", "target", "release", "ruach"))
CC = os.environ.get("CC", "cc")
NAMES = sys.argv[1:] or ["fib", "sieve", "nbody", "trees", "sort"]
RUNS = int(os.environ.get("RUNS", "3"))

def best(exe):
    times, out = [], None
    for _ in range(RUNS):
        t = time.perf_counter()
        out = subprocess.run([exe], capture_output=True, check=True).stdout
        times.append(time.perf_counter() - t)
    return min(times), out

with tempfile.TemporaryDirectory() as tmp:
    print(f"{'benchmark':10} {'Ruach':>8} {'C -O2':>8} {'ratio':>6}")
    for name in NAMES:
        rh, c = os.path.join(tmp, name + "-rh"), os.path.join(tmp, name + "-c")
        subprocess.run([RUACH, "build", os.path.join(HERE, name + ".rh"), "-o", rh], check=True)
        subprocess.run([CC, "-O2", os.path.join(HERE, name + ".c"), "-o", c, "-lm"], check=True)
        tr, outr = best(rh)
        tc, outc = best(c)
        if outr != outc:
            sys.exit(f"{name}: the outputs differ:\n{outr.decode()}\n{outc.decode()}")
        print(f"{name:10} {tr:7.2f}s {tc:7.2f}s {tr / tc:6.2f}")
