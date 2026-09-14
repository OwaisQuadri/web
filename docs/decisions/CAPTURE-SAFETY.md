# Capture safety

This note records the deleted Rust WebKit capture trial from issue 1. It is historical evidence, not current product code.

Every function runs on AppKit's main thread, and each callback checks its native pointers for null before reading them. No borrowed native pointer survives its callback. The copied native block retains the Rust-owned result state after a timeout.

A timed-out snapshot cannot write a late image. Scoped autorelease pools drain temporary native objects without releasing the caller's retained view. The snapshot callback returns retained image data without writing a file. The caller checks the original grant and page target immediately before writing. Recordings apply that check to each frame.

A scoped guard rejects overlapping captures and resets when each operation ends. Each completed manual capture gets a fresh output path. Hidden sessions use separate policies and their own retained views.

Engine captures use `WKWebView` snapshots only. Native control capture caches a dedicated native-control subtree, whose opaque `NSView` background is part of the captured subtree. It rejects any subtree that contains a `WKWebView` before bitmap allocation or output creation.

The runtime check requires every sampled button interior to be opaque and have visible luminance contrast. It rejects uniform white, uniform black, and transparent backdrops. This bounded content-contrast check does not recognize titles. AppKit caching of a whole root can include page content, so this path is not a full-window or page capture. Keep the local autorelease pool around `is_web_view_descendant`. The release test failed without it and passed with it, including a restored-baseline comparison.

Revocation stops new frame writes. Frames written during the original grant remain canceled partial local evidence. The trial retains them and does not report a successful completed recording.
