#!/usr/bin/env python3
"""Capture and compare Cargo test inventories across integration-target regrouping.

Run cargo test --workspace --locked --no-run --message-format=json into ARTIFACTS,
then cargo test --workspace --locked -- --list --format terse into LISTING and
cargo test --workspace --locked -- --ignored --list into IGNORED_LISTING.
Capture before moving tests, then after; compare also checks the ignored subset.
"""
import argparse
from collections import Counter
import json
from pathlib import Path
import subprocess


ROOT = Path(__file__).resolve().parents[1]


def listed(text):
    return [line.removesuffix(": test") for line in text.splitlines() if line.endswith(": test")]


def ignored_path(path):
    return path.with_name(path.stem + "-ignored" + path.suffix)


def capture(args):
    artifacts = [json.loads(line) for line in args.artifacts.read_text().splitlines()]
    targets = [a for a in artifacts if a.get("reason") == "compiler-artifact"
               and a.get("profile", {}).get("test") and a.get("executable")]
    inventories = [[], []]
    raw = [[], []]
    integrations = Counter()
    for artifact in targets:
        target = artifact["target"]
        source = Path(target["src_path"]).relative_to(ROOT / "server")
        crate = "porchlight-" + source.parts[0]
        if target["kind"] == ["test"]:
            integrations[crate] += 1
            # Cargo used to put each file in its own binary. The directory target
            # keeps that filename as a module, so only the old listing needs it.
            prefix = "" if source.name == "main.rs" else source.stem + "::"
            prefix = "integration::" + prefix
        else:
            prefix = target["kind"][0] + "::" + target["name"] + "::"
        for index, flags in enumerate(([], ["--ignored"])):
            result = subprocess.run([artifact["executable"], "--list", "--format", "terse", *flags],
                                    check=True, capture_output=True, text=True)
            names = listed(result.stdout)
            raw[index].extend(names)
            inventories[index].extend(f"{crate} {prefix}{name}" for name in names)
    for index, listing in enumerate((args.listing, args.ignored_listing)):
        expected = Counter(listed(listing.read_text()))
        remaining = expected - Counter(raw[index])
        assert not (Counter(raw[index]) - expected), "executable inventory differs from Cargo listing"
        for name, count in remaining.items():
            # rustdoc paths are crate-relative to the workspace, not test binaries.
            crate, separator, _ = name.partition("/src/")
            assert separator and " - " in name, f"unaccounted test: {name}"
            inventories[index].extend([f"porchlight-{crate} doc::{name}"] * count)
        assert len(inventories[index]) == expected.total(), "Cargo listing count differs"
        assert len(inventories[index]) == len(set(inventories[index])), "duplicate normalized test paths"
        output = args.output if index == 0 else ignored_path(args.output)
        output.write_text("\n".join(sorted(inventories[index])) + "\n")
    print(f"Captured {len(inventories[0])} tests ({len(inventories[1])} ignored)")
    print("Integration binaries: " + ", ".join(f"{crate}={count}" for crate, count in sorted(integrations.items())))


def compare(args):
    for before, after, label in ((args.before, args.after, "all"),
                                 (ignored_path(args.before), ignored_path(args.after), "ignored")):
        old = Counter(before.read_text().splitlines())
        new = Counter(after.read_text().splitlines())
        for key in sorted((old - new) | (new - old)):
            print(f"{label}: before={old[key]} after={new[key]} {key}")
        assert old == new, f"{label} inventory changed"
        print(f"PASS {label}: {old.total()} tests map 1:1 (only the old filename module prefix added)")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest="command", required=True)
    snapshot = commands.add_parser("capture")
    for name in ("artifacts", "listing", "ignored_listing", "output"):
        snapshot.add_argument(name, type=Path)
    snapshot.set_defaults(run=capture)
    check = commands.add_parser("compare")
    check.add_argument("before", type=Path)
    check.add_argument("after", type=Path)
    check.set_defaults(run=compare)
    args = parser.parse_args()
    args.run(args)


if __name__ == "__main__":
    main()
