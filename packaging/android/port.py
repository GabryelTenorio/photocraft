#!/usr/bin/env python3
"""Export the Android delta and merge only missing catalog keys on a future base.

No source files are patched automatically. Review/apply the three patches separately.
Only tracked Git objects are exported: signing keys and APKs cannot enter the bundle.
"""
import argparse
import json
from pathlib import Path
import subprocess


def entries(text):
    result = {}
    for line in text.splitlines():
        if not line or line.startswith('#'):
            continue
        fields = line.split('\t', 2)
        if len(fields) != 3:
            raise ValueError('Malformed translation row')
        key = tuple(fields[:2])
        if key in result:
            raise ValueError(f'Duplicate translation key: {key}')
        result[key] = fields[2]
    return result


def merge_missing(text, rows):
    """Keep every upstream translation, adding only absent context/source pairs."""
    current = entries(text)
    added = []
    for context, source, translated in rows:
        key = (context, source)
        if key not in current:
            added.append('\t'.join((context, source, translated)))
            current[key] = translated
    if not added:
        return text
    return text.rstrip('\n') + '\n\n# Android touch controls\n' + '\n'.join(added) + '\n'


def git(root, *args):
    return subprocess.check_output(['git', '-c', f'safe.directory={root.as_posix()}', *args], cwd=root)


def export(root, base, out):
    base = git(root, 'rev-parse', '--verify', base + '^{commit}').decode().strip()
    head = git(root, 'rev-parse', 'HEAD').decode().strip()
    out.mkdir(parents=True, exist_ok=True)
    groups = {
        '01-platform.patch': ['Cargo.toml', 'Cargo.lock', 'apps/photocraft-android', 'crates/text', 'xtask/src/layers.rs'],
        '02-touch.patch': ['crates/ui-egui', ':!crates/ui-egui/src/i18n/*.tsv'],
        '03-docs-tools.patch': ['docs/android-touch-controls.md', 'docs/android-porting.md', 'docs/images/android-touch', 'docs/hardware-parity.md', 'ATTRIBUTION.md', 'packaging/android'],
    }
    for filename, paths in groups.items():
        (out / filename).write_bytes(git(root, 'diff', '--binary', base, head, '--', *paths))
    catalogs = {}
    names = git(root, 'ls-tree', '-r', '--name-only', head, '--', 'crates/ui-egui/src/i18n').decode().splitlines()
    for name in names:
        if not name.endswith('.tsv'):
            continue
        now = entries(git(root, 'show', head + ':' + name).decode('utf-8'))
        try:
            old = entries(git(root, 'show', base + ':' + name).decode('utf-8'))
        except subprocess.CalledProcessError:
            old = {}
        rows = [[*key, value] for key, value in now.items() if key not in old]
        if rows:
            catalogs[Path(name).name] = rows
    (out / 'catalogs.json').write_text(json.dumps(catalogs, ensure_ascii=False, indent=2), encoding='utf-8')
    (out / 'manifest.json').write_text(json.dumps({'base': base, 'source': head, 'patches': list(groups), 'catalogs': list(catalogs)}, indent=2), encoding='utf-8')
    print(f'Exported three patches and {len(catalogs)} catalog deltas to {out}')


def catalogs(bundle, target):
    data = json.loads((bundle / 'catalogs.json').read_text(encoding='utf-8'))
    pending = []
    for filename, rows in data.items():
        if Path(filename).name != filename or not filename.endswith('.tsv'):
            raise ValueError('Invalid catalog filename')
        path = target / 'crates/ui-egui/src/i18n' / filename
        # A future branch may drop a language. Do not recreate it silently.
        if not path.is_file():
            raise ValueError(f'Catalog missing on target: {filename}')
        before = path.read_text(encoding='utf-8')
        after = merge_missing(before, rows)
        if before != after:
            pending.append((path, after))
    # Validate the whole bundle before writing any catalog.
    for path, after in pending:
        path.write_text(after, encoding='utf-8')
    print(f'Updated {len(pending)} catalogs; existing translations retained')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest='action', required=True)
    cmd = sub.add_parser('export')
    cmd.add_argument('--base', required=True)
    cmd.add_argument('--out', type=Path, required=True)
    cmd = sub.add_parser('catalogs')
    cmd.add_argument('--bundle', type=Path, required=True)
    cmd.add_argument('--target', type=Path, required=True)
    args = parser.parse_args()
    root = Path(__file__).resolve().parents[2]
    if args.action == 'export':
        export(root, args.base, args.out.resolve())
    else:
        catalogs(args.bundle.resolve(), args.target.resolve())


if __name__ == '__main__':
    main()
