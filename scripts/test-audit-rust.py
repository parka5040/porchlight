#!/usr/bin/env python3
"""Audit B-rust: release/dependency contracts and module-inventory regressions."""
import importlib.util
from pathlib import Path
import tempfile
import tomllib
import unittest


ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('rust_modules', ROOT / 'scripts/check-rust-modules.py')
modules = importlib.util.module_from_spec(spec)
spec.loader.exec_module(modules)


def manifest(crate=''):
    return tomllib.loads((ROOT / 'server' / crate / 'Cargo.toml').read_text())


class AuditRustManifests(unittest.TestCase):
    def test_release_is_tuned_without_disabling_panic_containment(self):
        release = manifest().get('profile', {}).get('release', {})
        self.assertEqual(release.get('lto'), 'thin', 'release must enable thin LTO')
        self.assertEqual(release.get('codegen-units'), 1)
        self.assertEqual(release.get('strip'), 'symbols')
        self.assertEqual(release.get('panic', 'unwind'), 'unwind')

    def test_cli_has_no_unused_direct_runtime_dependencies(self):
        dependencies = manifest('cli')['dependencies']
        self.assertNotIn('anyhow', dependencies, 'CLI has no anyhow calls')
        self.assertNotIn('chrono-tz', dependencies, 'CLI has no chrono_tz calls')

    def test_core_json_is_test_only(self):
        core = manifest('core')
        self.assertNotIn('serde_json', core['dependencies'], 'core JSON is test-only')
        self.assertIn('serde_json', core['dev-dependencies'])


class AuditRustModules(unittest.TestCase):
    def setUp(self):
        (ROOT / 'var').mkdir(exist_ok=True)
        self.temp = tempfile.TemporaryDirectory(dir=ROOT / 'var')
        self.addCleanup(self.temp.cleanup)
        self.server = Path(self.temp.name)
        self.crate = self.server / 'fixture'
        self.write('Cargo.toml', '[package]\nname = "fixture"\nversion = "0.1.0"\n')

    def write(self, name, content=''):
        path = self.crate / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_text(content)

    def test_real_tree_has_no_orphans(self):
        self.assertEqual(modules.orphan_sources(ROOT / 'server'), [])

    def test_orphan_chain_does_not_count_as_a_root(self):
        self.write('src/lib.rs', 'mod live;')
        self.write('src/live.rs')
        self.write('src/stale.rs', 'mod child;')
        self.write('src/stale/child.rs')
        self.assertEqual(modules.orphan_sources(self.server), [
            Path('fixture/src/stale/child.rs'), Path('fixture/src/stale.rs')])

    def test_cfg_path_inline_include_and_binary_roots(self):
        self.write('src/lib.rs', '''
#[cfg(test)] mod test_only;
#[path = "renamed.rs"] pub(crate) mod alias;
mod outer { mod nested; }
include!("included.rs");
''')
        for path in ['test_only.rs', 'renamed.rs', 'outer/nested.rs', 'included.rs',
                     'main.rs', 'bin/tool.rs', 'bin/other/main.rs']:
            self.write('src/' + path)
        self.assertEqual(modules.orphan_sources(self.server), [])

    def test_comments_and_strings_do_not_create_module_edges(self):
        self.write('src/lib.rs', '''
// mod stale;
/* nested /* mod stale; */ mod stale; */
const SAMPLE: &str = r###"mod stale; }"###;
const OTHER: &str = "mod stale;";
const CHAR: char = '}';
''')
        self.write('src/stale.rs')
        self.assertEqual(modules.orphan_sources(self.server), [Path('fixture/src/stale.rs')])

    def test_outline_and_inline_module_directories(self):
        self.write('src/lib.rs', 'mod outer; mod folder;')
        self.write('src/outer.rs', 'mod child; mod inline { mod leaf; }')
        self.write('src/outer/child.rs')
        self.write('src/outer/inline/leaf.rs')
        self.write('src/folder/mod.rs', 'mod child;')
        self.write('src/folder/child.rs')
        self.assertEqual(modules.orphan_sources(self.server), [])

    def test_explicit_roots_do_not_keep_replaced_defaults_alive(self):
        self.write('Cargo.toml', '''[package]
name = "fixture"
version = "0.1.0"
autobins = false
[lib]
path = "src/custom.rs"
[[bin]]
name = "tool"
path = "src/tool.rs"
''')
        for path in ['lib.rs', 'main.rs', 'custom.rs', 'tool.rs', 'bin/stale.rs']:
            self.write('src/' + path)
        self.assertEqual(modules.orphan_sources(self.server), [
            Path('fixture/src/bin/stale.rs'), Path('fixture/src/lib.rs'),
            Path('fixture/src/main.rs')])

    def test_unexpanded_macro_is_not_a_module_edge(self):
        self.write('src/lib.rs', 'macro_rules! unused { () => { mod stale; } }')
        self.write('src/stale.rs')
        self.assertEqual(modules.orphan_sources(self.server), [Path('fixture/src/stale.rs')])

    def test_dynamic_includes_fail_closed(self):
        self.write('src/lib.rs', 'include!(concat!("generated", ".rs"));')
        with self.assertRaisesRegex(ValueError, 'dynamic include'):
            modules.orphan_sources(self.server)


if __name__ == '__main__':
    unittest.main()
