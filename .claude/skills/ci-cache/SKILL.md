---
name: ci-cache
description: Rules and checks for the CI build cache of ike loud. Use before changing .github/workflows/ci.yml, scripts/ci-cache-key.sh, scripts/ci-clean-workspace.sh, scripts/check-fresh.py, Cargo/rustc/C toolchain settings, build-script environment variables, vendor/ handling, or anything else that can affect what is compiled; and whenever a cache seems stale or CI is slow.
---

# CI cache: only unchanged inputs, never our own code

The goal: CI is fast, and it always tests the **current** code, never an
older cached build of it. A cache that looks valid but is stale is worse
than no cache.

## Rules

1. **Cache only what did not change.** Today that is compiled third-party
   dependencies from `vendor/` (`target/`) and pinned tools (cargo-deny,
   cargo-audit, cargo-vet, Java, TLC). Results (tests, clippy, TLC,
   audits) are never cached.
2. **Our own crates are never cached.** `scripts/ci-clean-workspace.sh`
   removes their artifacts after restoring and before saving the cache.
   `scripts/check-fresh.py` fails CI if any of our crates was not compiled
   from the current sources.
3. **The key is the fingerprint of every input.** `scripts/ci-cache-key.sh`
   is the single place that computes it; CI never builds the key inline.
   Inputs today: OS, full `rustc -vV`, `cargo -V`, versions of the C
   compiler, archiver, linker and C library, every environment variable whose name
   matches a build-related pattern (`ENV_PATTERN`: `CARGO_*`, `RUST*`,
   `CC*`, `CFLAGS*`, `TARGET_*`, `HOST_*`, `PKG_CONFIG*`, `SQLITE*`, …),
   `Cargo.lock`, all manifests, `.cargo/config.toml`, `rust-toolchain.toml`,
   and every `vendor/**/.cargo-checksum.json`.
4. **Every input has a test.** `scripts/test-cache-key.sh` changes each
   input in a copy and asserts the key changes, including every variable any
   vendored build script declares with `rerun-if-env-changed` (found
   automatically, so a new dependency with a new variable fails the test).
   It also asserts the key does NOT change for our own code, docs and
   unrelated variables like `GITHUB_RUN_ID` (otherwise the cache would never
   hit). A missing input is a bug in the key.
5. **Exact keys only.** No `restore-keys`: never fall back to an older cache.
6. **vendor/ gets a fixed mtime in CI.** Cargo decides whether a dependency
   with a build script changed by mtimes, and a fresh checkout makes every
   file new. This is safe only because vendor/ content is in the key.
   Never apply the fixed mtime to our own sources.
7. **Tool caches are keyed by the same script** (`--tools`): exact pinned
   versions, the toolchain and the platform; CI checks the cached binaries
   report the pinned versions.
8. **Fingerprint what the artifacts depend on, not a label for it.** The
   runner image build number (`ImageVersion`) changes between runners of the
   same image without changing any tool, so it is not an input: it made the
   cache miss at random. The tools it stands for (cc, ar, ld, libc, rustc)
   are fingerprinted directly.
9. **The key must be deterministic.** The test computes it 21 times. Never
   pipe a multi-line command into `head` under `pipefail` (SIGPIPE race).

## When you change something

- **New build input** (a new system tool, a new config file, a new C
  dependency, an env var outside the patterns): add it to
  `scripts/ci-cache-key.sh` **and** a case to `scripts/test-cache-key.sh`.
  Variables declared with `rerun-if-env-changed` are found by the test
  itself; if it fails for one, extend `ENV_PATTERN`. Build scripts may also
  read variables without declaring them; when adding a dependency with a
  build script, read it: `grep -nE 'env::var|var_os' vendor/<crate>/build.rs`.
- **Changed meaning of the cache** (layout, cleaning rules): bump `SCHEMA`
  in `scripts/ci-cache-key.sh`.
- **New cache of any kind**: write down its inputs, put all of them in the
  key, add a test that each one changes the key, and make sure nothing of
  our own code can end up in it.
- **New workspace crate**: nothing to do; the clean script and the
  freshness check read the members from `cargo metadata`.

## How to verify

Locally, from the repository root:

```sh
scripts/test-cache-key.sh                 # every input changes the key
scripts/ci-cache-key.sh --explain         # what the key is made of
scripts/ci-clean-workspace.sh
cargo test --workspace --locked --offline --no-run --message-format=json-render-diagnostics \
  | scripts/check-fresh.py                # our crates compiled fresh
```

Also prove the checks can fail, after any change to them: remove one input
from the key script and run `scripts/test-cache-key.sh` (must fail); build
without cleaning and pipe to `scripts/check-fresh.py` (must fail). Restore.

In CI, the step "Build and check our crates are compiled fresh" prints how
many dependencies were rebuilt. On a cache hit that number should be 0; a
`::warning::` means the cache is not reused well (speed, not correctness).

## Background

Decision record: `docs/rozhodnuti/0008-cache-v-ci.md` (Czech).
