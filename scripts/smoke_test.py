#!/usr/bin/env python3
# Copyright (C) 2026 whispr.dev
# SPDX-License-Identifier: GPL-3.0-only
"""Exercise every standalone CLI command with the offline example's generated inputs.

Usage: python scripts/smoke_test.py --binary target/release/skeletrace --demo output/demo
Generate the demo first with cargo run --locked --example standalone_demo -- output/demo.
"""
from __future__ import annotations

import argparse
import json
from pathlib import Path
import subprocess
import sys
import tempfile


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--binary', type=Path, required=True)
    parser.add_argument('--demo', type=Path, required=True)
    args = parser.parse_args()
    binary = args.binary.resolve(strict=True)
    demo = args.demo.resolve(strict=True)
    profile = demo / 'profile.json'
    source_id = json.loads(profile.read_text(encoding='utf-8'))['sources'][0]['definition']['id']
    results = []
    with tempfile.TemporaryDirectory(prefix='skeletrace-cli-smoke-') as temporary:
        export_dir = Path(temporary) / 'exports'
        cases = [
            ('profile-validate', [profile], 'Topology'),
            ('tick', [profile], 'Tick'),
            ('poll-source', [profile, source_id], 'Tick'),
            ('materialize-topology', [profile, demo / 'topology-view.json'], 'Topology'),
            ('materialize-sparse-geo', [profile, demo / 'geo-view.json'], 'SparseGeo'),
            ('analyze-topology', [profile, demo / 'topology-view.json'], 'TopologyAnalysis'),
            ('export-snapshot', [profile, demo / 'snapshot-job.json', export_dir,
                                 Path(temporary) / 'catalog.sqlite'], 'Export'),
            ('query-latest', [profile, demo / 'filter.json'], 'Query'),
            ('query-advanced', [profile, demo / 'saved-query.json'], 'QueryEnvelope'),
            ('evaluate-watchlist', [profile, demo / 'watchlist.json'], 'Watchlist'),
            ('health', [profile], 'HealthReport'),
        ]
        for command, values, expected in cases:
            completed = subprocess.run([str(binary), command, *map(str, values)],
                                       text=True, encoding='utf-8', capture_output=True,
                                       timeout=30, check=False)
            if completed.returncode != 0:
                raise RuntimeError(f'{command} failed ({completed.returncode}): {completed.stderr}')
            response = json.loads(completed.stdout)
            if expected not in response:
                raise RuntimeError(f'{command}: expected {expected}, got {list(response)}')
            if command == 'poll-source' and response['Tick']['samples_seen'] != 3:
                raise RuntimeError('poll-source did not ingest three observations')
            if command == 'analyze-topology':
                analysis = response['TopologyAnalysis']
                if len(analysis['chokepoints']) != 1 or len(analysis['bridges']) != 2:
                    raise RuntimeError('analyze-topology returned the wrong graph result')
            if command == 'export-snapshot':
                snapshot = Path(response['Export']['output_path'])
                document = json.loads(snapshot.read_text(encoding='utf-8'))
                if document.get('type') != 'FeatureCollection':
                    raise RuntimeError('snapshot is not a GeoJSON FeatureCollection')
            results.append({'command': command, 'status': 'passed'})
        # The demo is a line of three nodes joined by two links at confidence 1, so the chain runs A, B, C.
        completed = subprocess.run([str(binary), 'strongest-chain', str(profile),
                                    str(demo / 'topology-view.json'),
                                    'Synthetic station A', 'Synthetic station C'],
                                   text=True, encoding='utf-8', capture_output=True,
                                   timeout=30, check=False)
        if completed.returncode != 0:
            raise RuntimeError(f'strongest-chain failed ({completed.returncode}): {completed.stderr}')
        chain = json.loads(completed.stdout)['StrongestChain']
        if not chain['found'] or len(chain['nodes']) != 3 or abs(chain['confidence'] - 1.0) > 1e-9:
            raise RuntimeError('strongest-chain did not find the A-B-C chain')
        results.append({'command': 'strongest-chain', 'status': 'passed'})
        # The demo's nodes sit on one line of longitude at 52.00, 52.05 and 52.10 N. A fence round the first two:
        fence = Path(temporary) / 'fence.geojson'
        fence.write_text(json.dumps({'type': 'Feature', 'properties': {'name': 'south pair'},
                                     'geometry': {'type': 'Polygon', 'coordinates': [[
                                         [-3.1, 51.9], [-2.9, 51.9], [-2.9, 52.07], [-3.1, 52.07],
                                         [-3.1, 51.9]]]}}), encoding='utf-8')
        completed = subprocess.run([str(binary), 'fence-check', str(profile),
                                    str(demo / 'topology-view.json'), str(fence)],
                                   text=True, encoding='utf-8', capture_output=True,
                                   timeout=30, check=False)
        if completed.returncode != 0:
            raise RuntimeError(f'fence-check failed ({completed.returncode}): {completed.stderr}')
        check = json.loads(completed.stdout)['FenceCheck']
        if len(check['fences'][0]['inside']) != 2 or len(check['outside_all']) != 1:
            raise RuntimeError('fence-check did not place the demo nodes correctly')
        results.append({'command': 'fence-check', 'status': 'passed'})
        completed = subprocess.run([str(binary), 'cover-area', str(profile),
                                    str(demo / 'topology-view.json')],
                                   text=True, encoding='utf-8', capture_output=True,
                                   timeout=30, check=False)
        if completed.returncode != 0:
            raise RuntimeError(f'cover-area failed ({completed.returncode}): {completed.stderr}')
        cover = json.loads(completed.stdout)['CoverArea']
        # Three nodes in a line enclose no area: the outline is the line's two ends.
        if cover['positioned'] != 3 or len(cover['outline']) != 2 or cover['area_km2'] != 0:
            raise RuntimeError('cover-area did not outline the demo nodes correctly')
        results.append({'command': 'cover-area', 'status': 'passed'})
        # sweep-text needs no profile: look for the demo's source id, and one term that isn't there.
        terms = Path(temporary) / 'terms.txt'
        terms.write_text(f'# smoke test\n{source_id}\n@nobody_here\n', encoding='utf-8')
        completed = subprocess.run([str(binary), 'sweep-text', str(terms), str(profile)],
                                   text=True, encoding='utf-8', capture_output=True,
                                   timeout=30, check=False)
        if completed.returncode != 0:
            raise RuntimeError(f'sweep-text failed ({completed.returncode}): {completed.stderr}')
        sweep = json.loads(completed.stdout)['Sweep']
        if sweep['hits_total'] < 1 or sweep['missing'] != ['@nobody_here']:
            raise RuntimeError('sweep-text did not find the source id in the profile')
        results.append({'command': 'sweep-text', 'status': 'passed'})
        # sweep-text --fuzzy: the source id with its last character changed is one typo from the real one.
        near_id = source_id[:-1] + ('1' if source_id[-1] == '0' else '0')
        terms.write_text(f'# smoke test\n{source_id}\n{near_id}\n', encoding='utf-8')
        completed = subprocess.run([str(binary), 'sweep-text', '--fuzzy', '1', str(terms), str(profile)],
                                   text=True, encoding='utf-8', capture_output=True,
                                   timeout=30, check=False)
        if completed.returncode != 0:
            raise RuntimeError(f'sweep-text --fuzzy failed ({completed.returncode}): {completed.stderr}')
        near = json.loads(completed.stdout)['Sweep']
        first = near['near_hits'][0] if near['near_hits'] else {}
        if (near['hits_total'] < 1 or near['near_only'] != [near_id]
                or first.get('edits') != 1 or first.get('matched') != source_id):
            raise RuntimeError('sweep-text --fuzzy did not find the source id one typo away')
        results.append({'command': 'sweep-text --fuzzy', 'status': 'passed'})
    print(json.dumps({'ok': True, 'commands_checked': len(results), 'results': results}, indent=2))
    return 0


if __name__ == '__main__':
    try:
        sys.exit(main())
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.SubprocessError) as error:
        print(f'SkeleTrace smoke test failed: {error}', file=sys.stderr)
        sys.exit(1)
