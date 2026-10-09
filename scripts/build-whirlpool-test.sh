#!/usr/bin/env bash
# Build a test-only external program; never include it in our release deploy directory.
set -euo pipefail

repository_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
fixture_root="${WHIRLPOOL_FIXTURE_DIR:-$repository_root/target/whirlpool-fixture}"
revision=f2a3d13fa04eb15cf5b5a309ef9b226fd5d34e36
archive_sha256=b23c9a917eacd89ac2e4d2544fe5efbd593c487ec6ff7f3c17cb1b976668c433

mkdir -p "$fixture_root"
if [[ ! -f "$fixture_root/source.tar.gz" ]]; then
    curl --fail --location --silent --show-error \
        "https://codeload.github.com/orca-so/whirlpools/tar.gz/$revision" \
        --output "$fixture_root/source.tar.gz"
fi
printf '%s  %s\n' "$archive_sha256" "$fixture_root/source.tar.gz" | sha256sum --check --status
if [[ ! -f "$fixture_root/source/Cargo.lock" ]]; then
    mkdir -p "$fixture_root/source"
    tar --extract --gzip --file "$fixture_root/source.tar.gz" \
        --directory "$fixture_root/source" --strip-components=1
fi

# Use the repository's already installed host toolchain, with the same pinned
# platform-tools as PaySH. The upstream lockfile is used without modification.
# Platform-tools v1.57's compiler panics while rendering an upstream dead-code
# warning. Disable that lint for this test fixture; source and lock stay intact.
RUSTUP_TOOLCHAIN=1.98.0 RUSTFLAGS="-Adead_code" CARGO_TARGET_DIR="$fixture_root/target" \
    cargo build-sbf --manifest-path "$fixture_root/source/programs/whirlpool/Cargo.toml" \
    --arch v3 --optimize-size --tools-version v1.57 -- --locked
printf 'Whirlpool test fixture: %s\n' "$fixture_root/target/deploy/whirlpool.so"
