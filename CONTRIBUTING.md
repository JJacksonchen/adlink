# Contributing to AdLink

Thank you for contributing to AdLink (Apache License 2.0)!

## Development Standards
1. **Formatting & Linting**: Run `cargo fmt --all` and `cargo clippy --workspace --all-targets -- -D warnings`.
2. **Testing**: Ensure `cargo test --workspace` passes completely, including mock official-JSON parser tests and the 180-day concurrent sync stress test.
3. **Monetary Integrity**: Store all money in `adlink_domain::Fen(i64)` and convert Yuan floats only at API boundaries (`Fen::from_yuan_f64`) while preserving upstream raw keys in `raw_fields`.
4. **Plugin Architecture**: New connectors, calibers, storage drivers, or guardrails should implement `adlink_kernel::Plugin` and register capabilities cleanly through `PluginContext`.
5. **No Scraping / Bypass**: Only implement official public APIs, whitelisted partner APIs, or compliant user-triggered export ingestion.

## Licensing
All contributions are licensed under the **Apache License 2.0**.
