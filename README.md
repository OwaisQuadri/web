# Web

Web is a native macOS browser built with Objective-C, AppKit, and WebKit. It keeps exactly one production `WKWebView` alive: inactive tabs retain address, title, bounded history, pin state, and available scroll position only. Reopening a tab uses the persistent default WebKit data store, so normal website sign-in cookies survive recreated views and application restarts.

## Build and test

```text
xcodebuild -project Web.xcodeproj -scheme Web build
xcodebuild -project Web.xcodeproj -scheme Web analyze
xcodebuild -project Web.xcodeproj -scheme Web test
```

The application has no storyboard, package manager, browser wrapper, or third-party dependency.

## Use

The address field accepts `http` and `https` URLs, adds `https://` to host-like input, and sends other text to Google. Unsupported schemes show a native error.

- Command-L focuses the address field.
- Command-T, Command-W, Command-Shift-T, and Command-R create, close, restore, and reload tabs.
- Command-1 through Command-8 select tab positions; Command-9 selects the last tab.
- Command-Option-Left and Command-Option-Right move between tabs.
- Command-Left and Command-Right move through application-managed history when the user does not edit the address field.

Switching a tab unloads its current page immediately before loading the new selected tab. This preserves the one-live-view policy but loses script execution, form state, media, calls, and some in-page history. Session recovery restores durable tab metadata from an atomic current-plus-backup JSON session. It cannot restore those live page states.

Application-managed Back and Forward reload saved addresses as GET requests. They never persist or resubmit POST form bodies. This first-release safety limit protects credentials and one-time actions. Issue 13 owns later active-work protection.

## Manual checks

Public-site navigation, sign-in persistence, upload, download, permission decisions, certificate rejection, popup login, and content-process recovery require a graphical manual check. Issue 2 excludes bookmarks, preview images, split browsing, password autofill, full keyboard mode, and release packaging.

## Memory benchmark

```text
xcodebuild -project Web.xcodeproj -scheme Web -configuration Benchmark build
.context/build/Products/Benchmark/Web.app/Contents/MacOS/Web --memory-benchmark
```

Xcode builds Benchmark without the App Sandbox because macOS App Sandbox blocks `/usr/bin/footprint`. Xcode ad hoc signs Debug and Release and keeps their sandboxes.

The strict benchmark generates its workload in memory and writes a temporary JSON receipt. It samples loaded, one-live, and reopened 7, 12, and 20-tab phases plus seven-tab split stress. Each measurement requires three repetitions and eleven observations spanning at least ten seconds. It checks `proc_pid_rusage` identity, correlated WebKit services, `/usr/bin/footprint -j` totals, stable membership, and weak native-view release. One-live and reopened phases must stay below 2,000,000,000 bytes. The file [evaluation/MEMORY.md](evaluation/MEMORY.md) retains the historical engine evidence and contract.
