#!/usr/bin/env python3
"""Generate/check README's capability table from the executable's canonical report."""
import argparse
import json
import pathlib
import subprocess

ROOT = pathlib.Path(__file__).resolve().parents[1]
BEGIN = '<!-- FAH_CAPABILITIES_BEGIN -->'
END = '<!-- FAH_CAPABILITIES_END -->'


def table(report):
    lines = [BEGIN, '| Operation | CLI operations | Availability | Evidence | Coverage limits |', '| --- | --- | --- | --- | --- |']
    for row in report['capabilities']:
        source = f'https://github.com/wilsonborba/fah/blob/{report["implementation_baseline"]}/{row["implementation"]}'
        commands = ', '.join(f'`{value}`' for value in row['commands']) or '—'
        levels = ', '.join(row['evidence'])
        limit = row['limits'].replace('|', '\\|') or row['effect'].replace('|', '\\|')
        lines.append(f'| [{row["operation"]}]({source}) | {commands} | {row["availability"]} | {levels} | {limit} |')
    return '\n'.join(lines + [END])


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--write', action='store_true')
    args = parser.parse_args()
    report = json.loads(subprocess.check_output(['cargo', 'run', '--quiet', '--locked', '--', 'capabilities'], cwd=ROOT))
    assert report['schema_version'] == 1 and not report['physical_validated']
    expected = table(report)
    readme = ROOT / 'README.md'
    text = readme.read_text()
    if BEGIN not in text:
        if not args.write:
            raise ValueError('README capability table is missing')
        text += '\n\n## Capability and evidence inventory\n\n' + expected + '\n'
    else:
        start, end = text.index(BEGIN), text.index(END) + len(END)
        actual = text[start:end]
        if not args.write:
            assert actual == expected, 'README capability table differs from CLI; regenerate with --write'
            print(f'capability matrix agrees with CLI ({len(report["capabilities"])} operations)')
            return
        text = text[:start] + expected + text[end:]
    readme.write_text(text)
    print(f'generated {len(report["capabilities"])} capability rows')


if __name__ == '__main__':
    main()
