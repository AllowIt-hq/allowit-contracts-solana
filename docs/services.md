# Host service packages

The SDK service facade delegates PaySH execution to these offchain packages:

- `service-upstreams` selects configured service implementations.
- `provider-google-air-quality` and `provider-open-meteo` implement air quality calls.
- `paysh-catalog` owns the operator discovery catalog.
- `delegated-x402` constructs and checks Solana x402 packets.
- `paysh-stock` implements the stock PaySH challenge and payment protocol.
- `paysh-cooperating` implements the cooperating-resource HTTP protocol.
- `service-rpc` constructs typed Solana RPC requests.

The server retains policy admission, spending limits, operation journals, signatures and recovery decisions. Catalog entries grant no execution authority. These packages do not execute in onchain programs.

Each package declares its own license. Packages extracted from the engine retain `LicenseRef-Proprietary` and their source revision in `Cargo.toml`.

Run each package's checks with its manifest:

```sh
cargo fmt --manifest-path crates/service-upstreams/Cargo.toml --check
cargo clippy --manifest-path crates/service-upstreams/Cargo.toml --locked --all-targets -- -D warnings
cargo test --manifest-path crates/service-upstreams/Cargo.toml --locked
```
