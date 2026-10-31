# Repository scope

Keep contract implementation, technical build/API documentation and release inputs in this repository. Planning, research, reviews, acceptance reports, recordings and generated evidence belong to `ackrate/ackrate-project`: raw Markdown sources in numbered `sources/` resources, curated knowledge in `wiki/`, and task artifacts in the matching `instance/artifacts/` resource. Preserve original source bodies and file hashes before removing historical outputs here. Do not commit task artifacts or reports in this repository. Use explicit output paths in the main project for retained generated evidence.

# Verification

For changes under `verification/`, including documentation, run `python3 -m unittest discover -s verification/tests`. Before changing a verification file, check whether observations, locks or receipts bind its hash, including references from other verification folders. Preserve hash-bound inputs. Never re-hash a lock, receipt or correspondence file to make a check pass.
