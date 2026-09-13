# Engine selection

WebKit is the version-one engine for Web on macOS. The decision uses measured memory, native lifecycle evidence, normal YouTube playback, persistent cookies, maintenance ownership, and distribution cost.

## Product policy

Web keeps one selected page live. It keeps the three most recent inactive tabs as screenshots and metadata. Older tabs retain metadata only. Selecting an inactive tab creates a new view and reloads its address through the persistent website data store.

Every accepted product case must stay below 2,000,000,000 bytes. CEF(Chromium Embedded Framework) remains the fallback if a daily-use case exceeds that limit or the product later requires application-level audio capture.

## Comparative workload

The comparison ran on an Apple M4 MacBook Pro with 16 gigabytes of memory. Each candidate used the same fixed loopback pages, page actions, off-the-record profile mode, and logical viewport. Each run included two isolated hidden sessions. The collector kept 11 observations over at least ten seconds. Run orchestration rejected detected process changes and build activity.

The frozen matrix covered 7, 12, and 20 loaded tabs, 7 loaded tabs in split view, and 20 unloaded tab records. Each candidate completed three reported repetitions for each shape. `results/memory-summary-v1.json` contains the aggregate bytes, startup times, process counts, sample windows, and memory-pressure readings. `results/webkit-media-memory-v1.json` contains the media-cycle result and the failed four-live-view result.

These files preserve historical decision inputs. The final validator requires dependency and build-activity receipts that the earlier comparative runs did not retain. Later source hardening also changed the tested inputs. The published results therefore do not validate the current staged sample and cannot serve as its regression baseline. GitHub issue 2 owns that new baseline and its automated check.

The reported median loaded peaks were:

- At 7 tabs, WebKit used 353,314,344 bytes, CEF used 796,346,680 bytes, and Qt WebEngine used 1,127,577,016 bytes.
- At 12 tabs, WebKit used 482,309,800 bytes, CEF used 1,126,934,064 bytes, and Qt WebEngine used 1,899,352,488 bytes.
- At 20 tabs, WebKit used 673,059,120 bytes, CEF used 1,562,365,904 bytes, and Qt WebEngine used 2,829,106,256 bytes.
- In the 7-tab split view, WebKit used 350,807,520 bytes, CEF used 795,019,408 bytes, and Qt WebEngine used 1,154,921,936 bytes.

The WebKit unloaded run did not prove native release and did not reduce memory. This failed route led to the one-live-view policy instead of a claim that WebKit can suspend 19 retained views cheaply.

## Real media result

Four simultaneous YouTube views peaked at 3,603,854,752 bytes and failed the limit. Pausing and muting three background videos did not reclaim enough memory.

The decision run created and released one WebKit view for each of four consecutive recent-tab selections. Its peaks were 1,102,536,048, 1,162,714,792, 1,135,812,624, and 1,289,757,144 bytes. Each stage stayed below the limit.

At execution time, the run reported stable process identities for all 11-observation windows. It reported all four native view releases and targeted the exact native window for each capture. It reported no build work, verified its then-current inputs, and restored the WebKit service baseline after exit. One idle WebContent service could remain between selections, but every following complete live-slot measurement stayed below the limit.

The staged sample now rejects a missing native release and applies stricter provenance checks. It has not repeated the external-site measurement after those changes.

The media-cycle captures prove native window targeting, not visible playback in every retained image. A separate WebKit check passed visible YouTube video, sustained unmuted playback, a scripted seek, full screen, and clean shutdown. The user heard its audio. Two separate application processes also proved that a persistent cookie survived restart. The read check then removed the proof cookie. The staged sample now uses a cookie-only fixture and clears the complete test store after reading it.

Chrome 153.0.8010.37 used an isolated temporary profile and no personal data. Process changes prevented a complete four-stage result. Its incomplete footprint cannot rank Chrome against WebKit, and the user accepted it as a non-blocking reference.

## Compatibility boundaries

Version one requires normal page audio, not application-level capture or gain. WebKit passed the normal playback requirement. Application-level tab recording, gain, and capture remain unsupported in the selected design.

Product work defers Apple Passwords, passkeys, 1Password, and browser extensions. Persistent cookies cover the current signed-in-session requirement. Ad blocking and translation have engine routes, but this evaluation does not mark those product features complete.

The sample keeps certificate checks and process isolation enabled. It does not contain a certificate bypass. Invalid-certificate fixtures and broader live-site coverage belong to product implementation.

## Licensing and updates

The project uses the General Public License version 3. Apple supplies WebKit with macOS and owns its security updates, so Web does not redistribute or update the engine separately.

The selected Rust bindings use MIT, Apache 2.0, or zlib-compatible licenses. Qt WebEngine would require its applicable General Public License, Lesser General Public License, or commercial obligations. CEF permits redistribution under its license, but Chromium component notices and codec rights would still need release review.

The Linux release needs a separate engine port and measurements. The native AppKit and WKWebView implementation does not run on Ubuntu.

## Deferred enforcement

The automated performance gate belongs to GitHub issue 2, where the production tab lifecycle and recovery behavior will exist. This ticket publishes the selected engine, workload, measurements, limits, and known boundaries without installing that future gate.
