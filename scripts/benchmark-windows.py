#!/usr/bin/env python3
"""Measure a cold and dependency-cached app build; keep machine-readable results."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import platform
import subprocess
import time

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--cross', action='store_true')
args = parser.parse_args()
target = 'x86_64-pc-windows-msvc'
command = ['cargo', *(['xwin'] if args.cross else []), 'build', '--locked', '--release', '-p', 'app', '--target', target]
results = {'host': platform.platform(), 'cpus': os.cpu_count(), 'command': command, 'commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip(), 'rustc': subprocess.check_output(['rustc', '-Vv'], text=True), 'seconds': {}}
results['source_tree'] = hashlib.sha256(subprocess.check_output(['git', 'ls-tree', 'HEAD', 'Cargo.toml', 'Cargo.lock', 'crates', 'vendor'])).hexdigest()
if args.cross:
    results['clang'] = subprocess.check_output(['clang-cl', '--version'], text=True)
for phase in ['cold', 'dependencies-cached']:
    if phase != 'cold':
        subprocess.run(['cargo', 'clean', '--release', '-p', 'app', '--target', target], check=True)
    start = time.monotonic()
    completed = subprocess.run(command)
    results['seconds'][phase] = round(time.monotonic() - start, 3)
    results['success'] = completed.returncode == 0
    Path('benchmark.json').write_text(json.dumps(results, indent=2) + '\n')
    if completed.returncode:
        raise SystemExit(completed.returncode)
    print(f'{phase}: {results["seconds"][phase]} seconds', flush=True)
if summary := os.environ.get('GITHUB_STEP_SUMMARY'):
    with open(summary, 'a') as output:
        output.write(f'Host: {results["host"]}; CPUs: {results["cpus"]}\n\n| Build | Seconds |\n|---|---:|\n')
        for phase, elapsed in results['seconds'].items():
            output.write(f'| {phase} | {elapsed} |\n')
