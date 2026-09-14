# Web engineering

Web is a programmatic Objective-C AppKit and WebKit application. Keep every tracked executable source file Objective-C. Project configuration, entitlements, generated property-list settings, Markdown, and the license are the only non-Objective-C tracked artifacts allowed.

Use only Foundation, AppKit, and WebKit in the application. Preserve automatic reference counting, warnings as errors, no storyboard, no package manager, and no third-party dependency. Name every new Boolean with an `is` prefix. Keep production ownership to one `WKWebView`; inactive tabs are metadata only.

Run and save output under `.context/issue-2/`:

```text
xcodebuild -project Web.xcodeproj -scheme Web -configuration Debug build
xcodebuild -project Web.xcodeproj -scheme Web -configuration Debug analyze
xcodebuild -project Web.xcodeproj -scheme Web -configuration Debug test
xcodebuild -project Web.xcodeproj -scheme Web -configuration Benchmark build
.context/build/Products/Benchmark/Web.app/Contents/MacOS/Web --memory-benchmark
```

Xcode builds Benchmark without the App Sandbox because macOS App Sandbox blocks `/usr/bin/footprint`. Xcode ad hoc signs Debug and Release and keeps their sandboxes.

The benchmark is a strict evidence gate, not a synthetic host-memory substitute. It must use all required phases. It must reject incomplete process correlation, process churn, reused identifiers, missing weak-view release, and malformed `footprint -j` totals. It must also reject a one-live or reopened total at or above 2,000,000,000 bytes. Do not claim benchmark success from a partial run.

Public-site behavior remains manual. Preserve durable historical evidence in `evaluation/MEMORY.md`, `evaluation/ENGINE-SELECTION.md`, and `docs/decisions/`; do not delete migrated source evidence until its replacement has passed the applicable checks.
