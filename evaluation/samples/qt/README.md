# Qt WebEngine smoke sample

This disposable sample targets Qt 6.11.2 on macOS. The repository does not include Qt frameworks. Set `QT_PREFIX` to an installed Qt 6.11.2 prefix that contains `lib/QtWebEngineWidgets.framework`.

Use the checked lockfile and promote warnings to errors:

```text
QT_PREFIX=<qt-prefix> RUSTFLAGS="-D warnings" cargo build --manifest-path evaluation/samples/qt/Cargo.toml --locked --offline
QT_PREFIX=<qt-prefix> RUSTFLAGS="-D warnings" cargo test --manifest-path evaluation/samples/qt/Cargo.toml --locked --offline
QT_PREFIX=<qt-prefix> cargo clippy --manifest-path evaluation/samples/qt/Cargo.toml --locked --offline --all-targets -- -D warnings
```

The sample exists to preserve the rejected candidate evidence. Web does not use Qt WebEngine for version one.
