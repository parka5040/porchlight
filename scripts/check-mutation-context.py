#!/usr/bin/env python3
"""Reject direct pool access in IdemTx/BoxFuture mutation callbacks.

This targeted guard does not prove transitive helper behavior. Helpers still must use
IdemTx's transaction; it prevents callbacks from accidentally acquiring state.pool.
"""
from pathlib import Path
import re
import sys
import unittest


def mask(source):
    # Keep offsets/braces, but ignore comments and normal/raw string contents.
    tokens = re.compile(r'//[^\n]*|/\*[\s\S]*?\*/|r(#+)?"[\s\S]*?"\1|r"[\s\S]*?"|"(?:\\.|[^"\\])*"')
    return tokens.sub(lambda m: " " * len(m[0]), source)


def violations(source):
    text = mask(source)
    failures = []
    for fn in re.finditer(r'\bfn\s+(\w+)[^{;]*\{', text):
        signature = fn[0]
        if not re.search(r'\bIdemTx\b', signature) or "BoxFuture" not in signature:
            continue
        depth = 1
        end = fn.end()
        while depth and end < len(text):
            depth += (text[end] == "{") - (text[end] == "}")
            end += 1
        body = text[fn.end():end]
        # after_commit runs only after IdemTx releases its transaction; its API
        # deliberately supplies AppState for fresh worker/read transactions.
        for hook in reversed(list(re.finditer(r'\.\s*after_commit\s*\(', body))):
            depth = 1
            stop = hook.end()
            while depth and stop < len(body):
                depth += (body[stop] == "(") - (body[stop] == ")")
                stop += 1
            body = body[:hook.start()] + " " * (stop - hook.start()) + body[stop:]
        if re.search(r'\.\s*pool\b', body):
            failures.append((source.count("\n", 0, fn.start()) + 1, fn[1]))
    return failures


class GuardTests(unittest.TestCase):
    def test_callback_pool_access_is_rejected(self):
        for body in ["sqlx::query(q).execute(&state.pool).await", "let p = state .\n pool.clone();", "state.0.pool.acquire().await"]:
            self.assertEqual(len(violations("fn write(s: &AppState, tx: &mut IdemTx) -> BoxFuture<'_, Result> {" + body + "}")), 1)

    def test_transaction_and_unrelated_read_are_allowed(self):
        source = '''fn write(s: &AppState, tx: &mut IdemTx) -> BoxFuture<'_, Result> {
            // state.pool is forbidden
            let note = "state.pool { }";
            let raw = r#"state.pool { }"#;
            sqlx::query(q).execute(&mut *tx.tx).await;
            tx.after_commit(Box::new(move |state, original| { state.pool.acquire() }));
        }
        async fn read(s: &AppState) { s.pool.acquire().await; }'''
        self.assertEqual(violations(source), [])


if __name__ == "__main__":
    if "--self-test" in sys.argv:
        unittest.main(argv=[sys.argv[0]])
    else:
        root = Path(__file__).resolve().parent.parent
        failures = []
        callbacks = 0
        for path in sorted((root / "server/api/src").rglob("*.rs")):
            text = path.read_text()
            callbacks += sum("IdemTx" in m[0] and "BoxFuture" in m[0] for m in re.finditer(r'\bfn\s+(\w+)[^{;]*\{', mask(text)))
            failures.extend(f"{path.relative_to(root)}:{line}: {name}: mutation callback must use IdemTx, not a pool" for line, name in violations(text))
        if failures:
            print("\n".join(failures))
            sys.exit(1)
        if not callbacks:
            sys.exit("No mutation callbacks checked; update the guard for the callback signature")
        print(f"Mutation callback pool guard: {callbacks} callbacks checked")
