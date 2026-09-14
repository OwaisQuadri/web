# Engine selection

WebKit is the version-one engine for Web on macOS. The native Objective-C application uses AppKit, WebKit, and the persistent default WebKit data store. Manual website sign-in persists through recreated views without a password manager or autofill implementation.

## Product memory policy

The production browser owns exactly one selected `WKWebView`. Every inactive tab is metadata only: title, address, bounded application-managed history, pin state, and available scroll coordinates. It retains no preview image, document object model, WebKit back-forward list, script state, media, call, form, or hidden view. Selecting another tab releases the current view and reloads the selected address through the default store.

Every accepted one-live and reopened phase must stay below 2,000,000,000 bytes. CEF remains a future fallback only if daily use exceeds that ceiling or requires application-level audio capture. Native browser audio remains in scope; application-level capture, gain, and recording are not.

## Historical comparative evidence

The engine comparison ran on an Apple M4 MacBook Pro with 16 gigabytes of memory. It used fixed local pages, two hidden sessions, eleven observations over at least ten seconds, three repetitions, process-change rejection, and independently deduplicated footprint accounting. The file [MEMORY.md](MEMORY.md) preserves its full 15-row numeric matrix, historical media cycle, release counts, ceiling, and incomplete Chrome reference.

The reported WebKit median loaded peaks were 353,314,344 bytes at 7 tabs, 482,309,800 bytes at 12 tabs, and 673,059,120 bytes at 20 tabs. The seven-tab split median was 350,807,520 bytes. The historical WebKit 20-tab unloaded median was 662,097,888 bytes. It did not prove native view release, so the product uses one live view.

Four simultaneous YouTube views peaked at 3,603,854,752 bytes and failed the ceiling. The historical one-live media sequence peaked at 1,102,536,048, 1,162,714,792, 1,135,812,624, and 1,289,757,144 bytes, with four owned-reference and native-view releases. Those results predate the final validator and are decision input, not a current regression baseline.

## Durable limits

The benchmark must use deterministic in-memory content, three repetitions, and eleven observations across at least ten seconds. It must check `proc_pid_rusage` process-start identities, WebKit service correlation, stable membership, and weak native-view release. It must use `/usr/bin/footprint -j` for deduplicated totals. The 7, 12, and 20-tab loaded, one-live, and reopened phases plus benchmark-only seven-tab split stress remain required. Process counters are a separate ledger and are never treated as deduplicated totals.

WebKit keeps standard certificate checks enabled; the app cancels server-trust failures and does not contain a certificate bypass. It uses normal WebKit security isolation. Visible site compatibility, audio playback, persistent cookies, upload/download, popup login, and system permission outcomes remain manual checks.

Tab switching cannot recover arbitrary script execution, form values, media, calls, or every in-page navigation. Persistent cookies cover signed-in sessions; Apple Passwords, passkeys, extensions, ad blocking, translation, previews, split browsing, and bookmarks remain later work.

## Licensing and platform boundary

The project uses GPLv3. Apple supplies and updates WebKit with macOS; Web does not redistribute the engine. The AppKit and WebKit application is macOS-only. A Linux release requires a separate engine port and new measurements.
