#!/usr/bin/env python3
"""Reject orphan server source files, following all cfg branches without building.

This is a source-inventory guard, not a Rust compiler: resolve ordinary/inline
modules, literal #[path] attributes and literal include! calls. Dynamic includes
and cfg_attr paths must get explicit support here rather than silently passing.
"""
import argparse
from pathlib import Path
import re
import tomllib


ROOT = Path(__file__).resolve().parents[1]
TOKEN = re.compile(r'''//[^\n]*|/\*|(?:br|cr|r)(\#*)"|(?:b|c)?"(?:\\.|[^"\\])*"|(?:b)?'(?:\\.|[^'\\\n])'|[A-Za-z_][A-Za-z_0-9]*|[^\s]''')


def tokens(text):
    result = []
    pos = 0
    while match := TOKEN.search(text, pos):
        value = match.group()
        pos = match.end()
        if value.startswith('//'):
            continue
        if value == '/*':
            depth = 1
            while depth:
                delimiter = re.search(r'/\*|\*/', text[pos:])
                if delimiter is None:
                    raise ValueError('unterminated block comment')
                depth += 1 if delimiter.group() == '/*' else -1
                pos += delimiter.end()
            continue
        if match.group(1) is not None:
            end = '"' + match.group(1)
            stop = text.index(end, pos)
            value = '"' + text[pos:stop] + '"'
            pos = stop + len(end)
        result.append(value)
    return result


def literal(token):
    if not token.startswith('"') or '\\' in token:
        raise ValueError('module paths must be plain string literals')
    return token[1:-1]


def source_roots(crate):
    manifest = tomllib.loads((crate / 'Cargo.toml').read_text())
    roots = set()
    package = manifest['package']
    lib = crate / manifest.get('lib', {}).get('path', 'src/lib.rs')
    if lib.is_file() and ('lib' in manifest or package.get('autolib', True)):
        roots.add(lib)
    bins = {path.stem: path for path in (crate / 'src/bin').glob('*.rs')}
    bins.update({path.parent.name: path for path in (crate / 'src/bin').glob('*/main.rs')})
    if (crate / 'src/main.rs').is_file():
        bins[package['name']] = crate / 'src/main.rs'
    for entry in manifest.get('bin', []):
        inferred = bins.pop(entry['name'], None)
        path = crate / entry['path'] if 'path' in entry else inferred
        if path is None:
            raise ValueError(f"cannot resolve binary {entry['name']}")
        roots.add(path)
    if package.get('autobins', True):
        roots.update(bins.values())
    return roots


def reachable_sources(crate):
    reached = set()
    visited = set()

    def visit(path, module_dir=None):
        path = path.resolve()
        if module_dir is None:
            module_dir = path.parent if path.name == 'mod.rs' else path.with_suffix('')
        key = (path, module_dir)
        if key in visited:
            return
        visited.add(key)
        reached.add(path)
        scan(tokens(path.read_text()), module_dir, path.parent, path)

    def scan(items, module_dir, path_base, source):
        i = 0
        override = None
        while i < len(items):
            token = items[i]
            if items[i:i + 2] == ['macro_rules', '!']:
                start = i + 3
                delimiters = {'{': '}', '(': ')', '[': ']'}
                i = closing(items, start, items[start], delimiters[items[start]]) + 1
                continue
            if items[i:i + 2] == ['#', '[']:
                end = closing(items, i + 1, '[', ']')
                attribute = items[i + 2:end]
                if attribute[:2] == ['path', '=']:
                    override = literal(attribute[2])
                elif attribute[:1] == ['cfg_attr'] and 'path' in attribute:
                    raise ValueError(f'{source.name}: cfg_attr path needs explicit guard support')
                i = end + 1
                continue
            if token == 'mod' and i + 2 < len(items) and items[i + 2] in [';', '{']:
                name, kind = items[i + 1:i + 3]
                if kind == ';':
                    candidates = ([path_base / override] if override is not None else
                                  [module_dir / (name + '.rs'), module_dir / name / 'mod.rs'])
                    existing = [path for path in candidates if path.is_file()]
                    if len(existing) != 1:
                        raise ValueError(f'{source.name}: cannot resolve module {name}')
                    visit(existing[0])
                    i += 3
                else:
                    end = closing(items, i + 2, '{', '}')
                    child = path_base / override if override is not None else module_dir / name
                    scan(items[i + 3:end], child, child, source)
                    i = end + 1
                override = None
                continue
            if items[i:i + 3] == ['include', '!', '(']:
                end = closing(items, i + 2, '(', ')')
                if end != i + 4:
                    raise ValueError(f'{source.name}: dynamic include needs explicit guard support')
                visit(source.parent / literal(items[i + 3]), module_dir)
                i = end + 1
                continue
            # Attributes apply only to the next item, not a later module.
            if token not in ['pub', '(', ')', 'crate', 'super', 'self', 'in', '::']:
                override = None
            i += 1

    for root in source_roots(crate):
        visit(root, root.parent)
    return reached


def closing(items, start, opening, end):
    depth = 0
    for i in range(start, len(items)):
        if items[i] == opening:
            depth += 1
        elif items[i] == end:
            depth -= 1
            if depth == 0:
                return i
    raise ValueError(f'unclosed {opening}')


def orphan_sources(server):
    orphans = []
    for manifest in sorted(server.glob('*/Cargo.toml')):
        crate = manifest.parent
        sources = {path.resolve() for path in (crate / 'src').rglob('*.rs')}
        orphans.extend(sources - reachable_sources(crate))
    return sorted(path.relative_to(server.resolve()) for path in orphans)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--server', type=Path, default=ROOT / 'server')
    args = parser.parse_args()
    try:
        orphans = orphan_sources(args.server)
    except (OSError, ValueError) as error:
        parser.exit(1, f'Rust module inventory: {error}\n')
    if orphans:
        parser.exit(1, 'Unreachable Rust sources:\n' + ''.join(f'  {path}\n' for path in orphans))
    print('Rust module inventory: OK (all server src files reachable)')


if __name__ == '__main__':
    main()
