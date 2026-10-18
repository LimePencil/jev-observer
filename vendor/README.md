# Linux Secret Service compatibility patches

These two crates retain their upstream MIT/Apache-2.0 licenses. `UPSTREAM.json`
records the published source version and Git provenance, and `Cargo.toml.orig`
retains each original manifest. The root `[patch.crates-io]` entries select these
sources; unmodified Apple and Windows stores still come from crates.io.

## Local changes

- `secret-service` 5.2.0: migrate to zbus 6 root wire types, owned property return
  values, deferred builder validation and its unified error type. Keep the
  existing asynchronous Secret Service implementation and DH/AES/HKDF behavior.
  The removed zbus blocking API and dependent synchronous facade are not shipped
  in this private patch. `rt-async-io` is retained as an alias to zbus's default
  runtime; `rt-tokio` remains available. This is not an upstream 5.2.0 API promise.
- `zbus-secret-service-keyring-store` 1.0.1: use that asynchronous API inside one
  `zbus::block_on` per synchronous keyring operation. The existing mutex still
  serializes shared service access. Credential attributes, lookup, collection
  selection, replacement, error mapping and public keyring interface are kept.
- Tokio is locked to 1.53.3 to satisfy zbus 6's minimum version.

Observer already runs credential mutations on Tokio's blocking pool. No provider
key, database-key, service name, workspace account hash or staged-approval format
is migrated. Startup reads and mutations use the same Secret Service protocol as
0.3.0; the daemon and its on-disk keyring format remain unchanged.

## Verification and maintenance

`scripts/test-linux-keyring.py` starts its own D-Bus daemon and GNOME keyring with
private XDG directories. It checks published 0.3.0 → candidate → 0.3.0 persistence,
rotation, deletion and session revocation across process/keyring restarts, using
only synthetic credentials and a loopback provider. Normal CI and both native
Linux jobs run it. Ordinary `cargo test` never accesses a personal keyring.

The script can additionally run the upstream backend test executable through
`--backend-tests`; it is run serially inside that same isolated environment.
Build it with:

```sh
cargo test --manifest-path vendor/zbus-secret-service-keyring-store/Cargo.toml \
  --target-dir target --features crypto-rust --lib --no-run \
  --config 'patch.crates-io.secret-service.path="vendor/secret-service"'
```

Remove these path patches when published compatible upstream replacements pass
that same old/new interoperability suite. Do not restore retired standalone
`zvariant`, `zvariant_derive` or `zbus_names` dependencies to silence an API error.
