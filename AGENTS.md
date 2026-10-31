# Repository scope

Keep contract implementation, technical build/API documentation and release inputs in this repository. Planning, research, reviews, acceptance reports, recordings and generated evidence belong to `ackrate/ackrate-project`: raw Markdown sources in numbered `sources/` resources, curated knowledge in `wiki/`, and task artifacts in the matching `instance/artifacts/` resource. Preserve original source bodies and file hashes before removing historical outputs here. Do not commit task artifacts or reports in this repository. Use explicit output paths in the main project for retained generated evidence.

# Build and pins

All packages share one Cargo workspace, one `Cargo.lock` and one version, and depend on each other by path. The SBF test harnesses `sbf-tests/` and `programs/mandate/sbf-tests/` keep their own locks, because their exact Mollusk pins would change the programs' dependencies. `allowit-sdk` is the only git dependency. It is declared once in the root `[workspace.dependencies]` (the mandate harness repeats it), and `[workspace.metadata.allowit.track]` says which revision it must be pinned to. Run `scripts/build.sh` before pushing: it fails on a stale pin and builds the workspace. `scripts/build.sh --update` moves stale pins and refreshes the locks.
