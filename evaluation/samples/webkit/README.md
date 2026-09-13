# WebKit smoke sample

This disposable Rust application uses system WebKit through pinned native bindings. It is not the production browser.

The application has two visible tab records and two separate hidden sessions. Both visible tabs share a new nonpersistent store. Each hidden session has its own new nonpersistent store. Every created web view uses an explicitly assigned nonpersistent data store.

## Run

Build with the approved task-local Cargo cache, target directory, developer tools, and deployment target. Keep warnings as errors and use the checked lockfile.

```text
cargo build --manifest-path evaluation/samples/webkit/Cargo.toml --locked --offline
cargo test --manifest-path evaluation/samples/webkit/Cargo.toml --locked --offline
cargo clippy --manifest-path evaluation/samples/webkit/Cargo.toml --locked --offline --all-targets -- -D warnings
```

Start the shared `fixture-server` with `--port 0`. It prints its chosen loopback origin. Pass that exact origin and a new task-local evidence directory to the sample:

```text
webkit-smoke --smoke <fixture-origin> <evidence-directory>
webkit-smoke --manual <fixture-origin> <evidence-directory>
```

Manual mode provides tab selection, split, navigation, unload/reload, grant/revoke, snapshot, and recording controls. Snapshot and recording require a grant for the selected tab. The sample restricts navigation to the supplied fixture origin.

Smoke mode runs the native application event loop, drives the controls, writes per-case results to `smoke.json`, and exits. A failed check produces a nonzero exit. The manifest also names checks that remain blocked or unrun. A zero exit is not approval of the complete engine evaluation.

The evaluation also has three bounded WebKit modes:

```text
webkit-smoke --youtube-smoke <evidence-directory>
webkit-smoke --media-memory-smoke <evidence-directory>
webkit-smoke --cookie-smoke <fixture-origin> <evidence-directory> <write|read> <token>
```

## Memory evidence scope

The live YouTube memory run remains local release evidence. The repository retains its sample mode and published result, but it does not install the deferred performance runner.

YouTube mode loads one fixed public video. It records video readiness, five seconds of playback, a scripted seek, and full-screen state. The page state proves unmuted playback but cannot prove acoustic output. Record a separate user observation when a person hears the test.

Media-memory mode creates sequential visible YouTube `WKWebView` lifetimes and keeps exactly one view live. During each live stage, it retains the three preceding screenshot-backed records and 16 metadata-only older records. Each bounded Objective-C autorelease pool creates, loads, readies, measures, detaches, and drops one fixed-URL, off-the-record view. After each runner marker, the application stops loading, clears its window content view, removes the view, and creates a native weak reference. The application drops its owned reference, drains the pool, observes the weak reference for a bounded interval, and writes a release receipt.

Before it creates the next slot, the runner records whether newly cycle-attributed WebKit services remain after release. The runner measures the host with all current system WebKit services; each 11-sample window stays below the approved ceiling.

Cookie mode uses an identified persistent WebKit data store. Run its write and read stages in separate application processes with the same loopback origin and token. The read stage proves cookie survival across restart and clears the proof cookie.

## Capture behavior

The hidden sessions use transparent, borderless host windows that ignore mouse events. They require a macOS graphical session and the window server. They are not windowless browser processes.

The unparented-view experiment produced static frames. Transparent host windows allowed the fixture animation and recording checks to pass without a visible interactive window. This construction adds native window resources that future accounting must include.

Engine snapshots contain the actual WebKit page. Native control snapshots cache only a dedicated native-control subtree: it structurally rejects any subtree that contains a `WKWebView` before allocating a bitmap. AppKit caching of the whole root can include WebKit page content, so native control snapshots are neither full-window nor page captures. Inspect both kinds of evidence. Recordings use 12 engine snapshots and the locally installed FFmpeg program. The sample checks changing frames and decodes the resulting video.

## Limits

The smoke checks cover storage, controls, native view release, capture, and cleanup. A weak-reference check establishes release of the native view, not renderer-memory reclamation. The sample does not measure total browser memory.

The current fixture uses unencrypted local transport. An invalid-certificate fixture remains necessary before external-site tests. This sample configures no certificate bypass. It requests no device permission and does not establish independent system permission decisions.

Password managers, codecs for distribution, ad blocking, translation, and application-level tab audio capture remain separate evaluation work. The YouTube mode covers one live site and one video. It does not establish broad live-site compatibility.

## Native safety

The main thread owns every native window, view, store, and delegate. The entry point retains the delegate until the application loop ends. It clears weak delegate links before releasing the views and closing the windows. Native buttons reference only selectors implemented by that live delegate.

The application creates each WebKit configuration and assigns its store before constructing the view. The store owner outlives its views. The application retains the native controls in their own 80-point-high sibling subtree above the page region; WebKit views are never descendants of that subtree. The unload operation detaches the selected view, stops loading, clears its delegate, and releases the owned reference. Scoped autorelease pools drain temporary native references.

Native method declarations come from the pinned bindings. The application passes typed geometry records and valid retained objects. Its navigation decision callback calls the supplied completion block exactly once. The one-shot timer retains its callback state until completion and posts an application event after stopping the loop.

`CAPTURE-SAFETY.md` describes capture callback lifetimes. Revocation stops new frame writes; the sample retains previously authorized partial frames as local evidence and are not a successful recording. The experiment uses standard WebKit isolation and certificate behavior. It does not enable private rendering switches or disable security for capture.
