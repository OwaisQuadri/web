use super::*;
use objc2::rc::{Retained, Weak};
use std::time::{Duration, Instant};

struct Checks {
    directory: PathBuf,
    rows: Vec<(&'static str, &'static str, String)>,
}

impl Checks {
    fn add(&mut self, name: &'static str, result: Result<(), String>) {
        let (outcome, detail) = match result {
            Ok(()) => ("passed", String::new()),
            Err(error) => ("failed", error),
        };
        self.report(name, outcome, detail);
    }

    fn report(&mut self, name: &'static str, outcome: &'static str, detail: String) {
        println!("{name}: {outcome} {detail}");
        self.rows.push((name, outcome, detail));
    }

    fn finish(&self) -> Result<(), String> {
        let rows: Vec<_> = self
            .rows
            .iter()
            .map(|(name, outcome, detail)| {
                format!(
                    "{{\"name\":{},\"outcome\":{},\"detail\":{}}}",
                    json_string(name),
                    json_string(outcome),
                    json_string(detail)
                )
            })
            .collect();
        let document = format!(
            "{{\"schema_version\":1,\"candidate\":\"webkit\",\"scope\":\"synthetic_smoke_not_benchmark\",\"checks\":[{}]}}\n",
            rows.join(",")
        );
        std::fs::write(self.directory.join("smoke.json"), document)
            .map_err(|error| error.to_string())?;
        if self.rows.iter().any(|(_, outcome, _)| *outcome == "failed") {
            Err("one or more WebKit smoke checks failed; inspect smoke.json".into())
        } else {
            Ok(())
        }
    }
}

fn json_string(value: &str) -> String {
    let mut output = String::from("\"");
    for character in value.chars() {
        match character {
            '"' => output.push_str("\\\""),
            '\\' => output.push_str("\\\\"),
            character if character < ' ' => {
                output.push_str(&format!("\\u{:04x}", character as u32))
            }
            character => output.push(character),
        }
    }
    output.push('"');
    output
}

fn require(is_true: bool, message: &str) -> Result<(), String> {
    if is_true { Ok(()) } else { Err(message.into()) }
}

fn expect_value(view: &WKWebView, expression: &str, expected: &str) -> Result<(), String> {
    let actual = capture::evaluate(view, expression)?;
    require(
        actual == expected,
        &format!("expected {expected:?}, observed {actual:?}"),
    )
}

fn wait_value(view: &WKWebView, expression: &str, expected: &str) -> Result<(), String> {
    let start = Instant::now();
    let mut actual = String::new();
    while start.elapsed() < Duration::from_secs(10) {
        actual = capture::evaluate(view, expression)?;
        if actual == expected {
            return Ok(());
        }
        if actual.starts_with("error:") {
            return Err(actual);
        }
        capture::pump(0.05);
    }
    Err(format!(
        "timed out waiting for {expected:?}; observed {actual:?}"
    ))
}

fn policy_error(result: Result<(), String>, expected: &str) -> Result<(), String> {
    match result {
        Err(error) if error.contains(expected) => Ok(()),
        Err(error) => Err(format!("expected {expected}, observed {error}")),
        Ok(()) => Err(format!("expected {expected}, operation succeeded")),
    }
}

fn guarded_visible_snapshot(delegate: &Retained<Delegate>, path: &Path) -> Result<(), String> {
    let (target, view) = delegate.authorized_target_view()?;
    capture::snapshot_guarded(&view, path, &|| {
        delegate
            .ivars()
            .policy
            .borrow()
            .authorize(target)
            .map_err(|error| format!("control denied: {error:?}"))
    })
}

fn guarded_hidden_snapshot(
    delegate: &Retained<Delegate>,
    is_first: bool,
    path: &Path,
) -> Result<(), String> {
    let (target, view) = delegate.hidden_target_view(is_first)?;
    let policy = if is_first {
        &delegate.ivars().hidden_first_policy
    } else {
        &delegate.ivars().hidden_second_policy
    };
    capture::snapshot_guarded(&view, path, &|| {
        policy
            .borrow()
            .authorize(target)
            .map_err(|error| format!("control denied: {error:?}"))
    })
}

fn guarded_hidden_record(
    delegate: &Retained<Delegate>,
    is_first: bool,
    directory: &Path,
) -> Result<(), String> {
    let (target, view) = delegate.hidden_target_view(is_first)?;
    let policy = if is_first {
        &delegate.ivars().hidden_first_policy
    } else {
        &delegate.ivars().hidden_second_policy
    };
    capture::record_guarded(&view, directory, &|| {
        policy
            .borrow()
            .authorize(target)
            .map_err(|error| format!("control denied: {error:?}"))
    })
}

fn control_geometry(delegate: &Retained<Delegate>) -> Result<(), String> {
    let controls = delegate.controls();
    let content = delegate.content();
    let primary = delegate
        .ivars()
        .primary
        .borrow()
        .clone()
        .ok_or("missing primary WebView")?;
    let controls_frame = controls.frame();
    let controls_bounds = controls.bounds();
    let primary_frame = primary.frame();
    require(
        controls.isDescendantOf(&content)
            && !capture::is_web_view_descendant(&controls)
            && capture::is_web_view_descendant(&content)
            && primary.isDescendantOf(&content)
            && !primary.isDescendantOf(&controls)
            && controls_frame.origin.x == 0.0
            && controls_frame.origin.y == CONTENT_HEIGHT
            && controls_frame.size.width == WIDTH
            && controls_frame.size.height == 80.0
            && controls_bounds.origin.x == 0.0
            && controls_bounds.origin.y == 0.0
            && controls_bounds.size.width == WIDTH
            && controls_bounds.size.height == 80.0
            && primary_frame.origin.x == 0.0
            && primary_frame.origin.y == 0.0
            && primary_frame.size.width == WIDTH
            && primary_frame.size.height == CONTENT_HEIGHT,
        "control subtree does not exclude WebViews or use the expected control and page bounds",
    )
}

fn denied_snapshot_writes_nothing(
    path: &Path,
    expected: &str,
    attempt: impl FnOnce() -> Result<(), String>,
) -> Result<(), String> {
    require(
        !path.exists(),
        "denied snapshot path exists before the attempt",
    )?;
    policy_error(attempt(), expected)?;
    require(!path.exists(), "denied snapshot wrote an output file")
}

fn split_geometry(delegate: &Retained<Delegate>) -> Result<(), String> {
    let primary = delegate
        .ivars()
        .primary
        .borrow()
        .clone()
        .ok_or("missing primary WebView")?;
    let secondary = delegate
        .ivars()
        .secondary
        .borrow()
        .clone()
        .ok_or("missing secondary WebView")?;
    let window = delegate
        .ivars()
        .window
        .get()
        .ok_or("missing owned window")?;
    let primary_window = primary.window().ok_or("primary has no window")?;
    let secondary_window = secondary.window().ok_or("secondary has no window")?;
    let primary_frame = primary.frame();
    let secondary_frame = secondary.frame();
    require(
        primary.isDescendantOf(&delegate.content())
            && secondary.isDescendantOf(&delegate.content())
            && !primary.isHiddenOrHasHiddenAncestor()
            && !secondary.isHiddenOrHasHiddenAncestor()
            && std::ptr::eq(&**primary_window, &***window)
            && std::ptr::eq(&**secondary_window, &***window)
            && primary_frame.origin.x == 0.0
            && primary_frame.origin.y == 0.0
            && primary_frame.size.width == WIDTH / 2.0
            && primary_frame.size.height == CONTENT_HEIGHT
            && secondary_frame.origin.x == WIDTH / 2.0
            && secondary_frame.origin.y == 0.0
            && secondary_frame.size.width == WIDTH / 2.0
            && secondary_frame.size.height == CONTENT_HEIGHT
            && primary_frame.origin.x + primary_frame.size.width <= secondary_frame.origin.x,
        "split views are not visible, owned by the same window, or have nonoverlapping expected frames",
    )
}

fn revoke_while_snapshot_pending(delegate: &Retained<Delegate>, path: &Path) -> Result<(), String> {
    require(
        !path.exists(),
        "revoked snapshot path exists before the attempt",
    )?;
    let (target, view) = delegate.authorized_target_view()?;
    let callback_delegate = delegate.clone();
    let callback = block2::RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| {
        callback_delegate.ivars().policy.borrow_mut().revoke();
    });
    let timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &callback) };
    let result = capture::snapshot_guarded(&view, path, &|| {
        delegate
            .ivars()
            .policy
            .borrow()
            .authorize(target)
            .map_err(|error| format!("control denied: {error:?}"))
    });
    drop(timer);
    policy_error(result, "control denied: Denied")?;
    require(!path.exists(), "revoked snapshot wrote an output file")
}

fn frame_identities(directory: &Path) -> Result<Vec<String>, String> {
    let mut identities = std::fs::read_dir(directory)
        .map_err(|error| format!("read recording directory: {error}"))?
        .filter_map(Result::ok)
        .filter_map(|entry| {
            let name = entry.file_name().to_string_lossy().into_owned();
            (name.starts_with("frame-") && name.ends_with(".tiff")).then_some(name)
        })
        .collect::<Vec<_>>();
    identities.sort_unstable();
    Ok(identities)
}

fn revoke_mid_recording(delegate: &Retained<Delegate>, directory: &Path) -> Result<(), String> {
    let (target, view) = delegate.authorized_target_view()?;
    let observation = Rc::new(RefCell::new(None));
    let callback_observation = Rc::clone(&observation);
    let callback_directory = directory.to_path_buf();
    let callback_delegate = delegate.clone();
    let callback = block2::RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| {
        if callback_observation.borrow().is_none() {
            match frame_identities(&callback_directory) {
                Ok(identities) if !identities.is_empty() => {
                    callback_delegate.ivars().policy.borrow_mut().revoke();
                    *callback_observation.borrow_mut() = Some(Ok(identities));
                }
                Ok(_) => {}
                Err(error) => *callback_observation.borrow_mut() = Some(Err(error)),
            }
        }
    });
    let timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.01, true, &callback) };
    let result = capture::record_guarded(&view, directory, &|| {
        delegate
            .ivars()
            .policy
            .borrow()
            .authorize(target)
            .map_err(|error| format!("control denied: {error:?}"))
    });
    timer.invalidate();
    let observed = observation
        .borrow_mut()
        .take()
        .ok_or("recording observer did not observe a written frame")??;
    let after = frame_identities(directory)?;
    let outcome = match result {
        Err(error) if error.contains("control denied: Denied") => error,
        Err(error) => return Err(format!("expected control denied: Denied, observed {error}")),
        Ok(()) => return Err("expected control denied: Denied, recording succeeded".into()),
    };
    std::fs::write(
        directory.join("revocation-observation.txt"),
        format!(
            "observed_frame_count={}\nobserved_frame_identities={}\npost_revocation_frame_count={}\npost_revocation_frame_identities={}\noutcome={}\nrecording_completed=false\n",
            observed.len(),
            observed.join(","),
            after.len(),
            after.join(","),
            outcome,
        ),
    )
    .map_err(|error| format!("write revocation observation: {error}"))?;
    require(
        observed == after && !after.is_empty(),
        "revocation allowed a new recording frame after the observer revoked capture",
    )?;
    require(
        !directory.join("recording.mp4").exists(),
        "revoked recording created a completed movie",
    )
}

fn revoke_while_recording(delegate: &Retained<Delegate>, directory: &Path) -> Result<(), String> {
    require(
        !directory.exists(),
        "revoked recording directory exists before the attempt",
    )?;
    let (target, view) = delegate.authorized_target_view()?;
    let callback_delegate = delegate.clone();
    let callback = block2::RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| {
        callback_delegate.ivars().policy.borrow_mut().revoke();
    });
    let timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &callback) };
    let result = capture::record_guarded(&view, directory, &|| {
        delegate
            .ivars()
            .policy
            .borrow()
            .authorize(target)
            .map_err(|error| format!("control denied: {error:?}"))
    });
    drop(timer);
    policy_error(result, "control denied: Denied")?;
    let files = if directory.exists() {
        std::fs::read_dir(directory)
            .map_err(|error| format!("read revoked recording directory: {error}"))?
            .count()
    } else {
        0
    };
    require(
        files == 0,
        "revocation before the first frame wrote recording output",
    )
}

fn nested_control_capture_rejected(
    delegate: &Retained<Delegate>,
    path: &Path,
) -> Result<(), String> {
    require(
        !path.exists(),
        "nested control capture path exists before the attempt",
    )?;
    let reentry = Rc::new(RefCell::new(None));
    let callback_reentry = Rc::clone(&reentry);
    let callback_delegate = delegate.clone();
    let callback_path = path.to_path_buf();
    let callback = block2::RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| {
        *callback_reentry.borrow_mut() = Some(callback_delegate.capture_controls(&callback_path));
    });
    let timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &callback) };
    let outer = delegate.record_selected();
    drop(timer);
    outer?;
    let result = reentry
        .borrow_mut()
        .take()
        .ok_or("nested control capture timer did not fire")?;
    policy_error(
        result,
        "control snapshot rejected: capture operation already in progress",
    )?;
    require(
        !path.exists(),
        "nested control capture wrote an output file",
    )
}

fn nested_record_rejected(delegate: &Retained<Delegate>) -> Result<(), String> {
    let reentry = Rc::new(RefCell::new(None));
    let callback_reentry = Rc::clone(&reentry);
    let callback_delegate = delegate.clone();
    let callback = block2::RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| {
        *callback_reentry.borrow_mut() = Some(callback_delegate.record_selected());
    });
    let timer =
        unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.0, false, &callback) };
    let outer = delegate.record_selected();
    drop(timer);
    outer?;
    let result = reentry
        .borrow_mut()
        .take()
        .ok_or("nested record timer did not fire")?;
    policy_error(
        result,
        "record rejected: capture operation already in progress",
    )
}

fn wait_ready(view: &WKWebView) -> Result<(), String> {
    wait_value(
        view,
        "String(document.readyState === 'complete' && !!document.querySelector('#input'))",
        "true",
    )
}

fn wait_for_address_field(delegate: &Retained<Delegate>, expected: &str) -> Result<(), String> {
    let selected = delegate.selected_view()?;
    wait_value(&selected, "String(location.href)", expected)?;
    let start = Instant::now();
    while start.elapsed() < Duration::from_secs(10) {
        let actual = delegate
            .ivars()
            .address
            .get()
            .expect("address")
            .stringValue()
            .to_string();
        if actual == expected {
            return Ok(());
        }
        capture::pump(0.05);
    }
    Err(format!("timed out waiting for address field {expected:?}"))
}

fn write_storage(view: &WKWebView, token: &str) -> Result<(), String> {
    let token = json_string(token);
    let script = format!(
        r#"(() => {{
        const token = {token};
        window.storageWrite = 'pending';
        localStorage.setItem('isolation-token', token);
        document.cookie = 'isolation-token=' + token + '; Path=/; SameSite=Strict';
        document.querySelector('#input').value = token;
        document.querySelector('#input').dispatchEvent(new Event('input', {{bubbles: true}}));
        (async () => {{
            await new Promise((resolve, reject) => {{
                const request = indexedDB.open('smoke-db', 1);
                request.onerror = () => reject(request.error);
                request.onsuccess = () => {{
                    const database = request.result;
                    const transaction = database.transaction('values', 'readwrite');
                    transaction.objectStore('values').put(token, 'isolation-token');
                    transaction.oncomplete = () => {{ database.close(); resolve(); }};
                    transaction.onerror = () => {{ database.close(); reject(transaction.error); }};
                }};
            }});
            const cache = await caches.open('isolation-cache');
            await cache.put('/isolation-token', new Response(token));
            window.storageWrite = 'done';
        }})().catch(error => {{ window.storageWrite = 'error:' + String(error); }});
        return 'started';
    }})()"#
    );
    expect_value(view, &script, "started")?;
    wait_value(view, "String(window.storageWrite)", "done")
}

fn read_storage(view: &WKWebView, token: &str) -> Result<(), String> {
    let script = r#"(() => {
        window.storageRead = 'pending';
        (async () => {
            const databaseValue = await new Promise((resolve, reject) => {
                const request = indexedDB.open('smoke-db', 1);
                request.onerror = () => reject(request.error);
                request.onsuccess = () => {
                    const database = request.result;
                    const transaction = database.transaction('values', 'readonly');
                    const value = transaction.objectStore('values').get('isolation-token');
                    value.onsuccess = () => resolve(value.result);
                    value.onerror = () => reject(value.error);
                    transaction.oncomplete = () => database.close();
                };
            });
            const cache = await caches.open('isolation-cache');
            const response = await cache.match('/isolation-token');
            const cookie = document.cookie.split('; ').find(value => value.startsWith('isolation-token='));
            window.storageRead = [localStorage.getItem('isolation-token'), cookie?.split('=')[1], databaseValue, response ? await response.text() : 'missing', document.querySelector('#input').value].join('|');
        })().catch(error => { window.storageRead = 'error:' + String(error); });
        return 'started';
    })()"#;
    expect_value(view, script, "started")?;
    wait_value(view, "String(window.storageRead)", &[token; 5].join("|"))
}

pub(super) fn run(delegate: &Retained<Delegate>) -> Result<(), String> {
    let directory = delegate.evidence()?.to_path_buf();
    std::fs::create_dir_all(&directory).map_err(|error| error.to_string())?;
    let mut checks = Checks {
        directory: directory.clone(),
        rows: Vec::new(),
    };
    if let Err(error) = run_checks(delegate, &directory, &mut checks) {
        checks.add("smoke_setup_or_runtime", Err(error));
    }
    checks.finish()
}

fn run_checks(
    delegate: &Retained<Delegate>,
    directory: &Path,
    checks: &mut Checks,
) -> Result<(), String> {
    {
        for (name, slot) in [
            ("visible_primary_ready", &delegate.ivars().primary),
            ("visible_secondary_ready", &delegate.ivars().secondary),
            ("hidden_first_ready", &delegate.ivars().hidden_first),
            ("hidden_second_ready", &delegate.ivars().hidden_second),
        ] {
            let view = slot.borrow().clone().ok_or("missing WebView")?;
            checks.add(name, wait_ready(&view));
        }
    }
    checks.add("control_subtree_geometry", control_geometry(delegate));
    checks.add(
        "control_capture_without_page_grant",
        delegate.capture_controls(&directory.join("ungranted-controls.tiff")),
    );
    let old_root_path = directory.join("old-root-controls-rejected.tiff");
    checks.add(
        "control_capture_rejects_old_root_without_output",
        denied_snapshot_writes_nothing(
            &old_root_path,
            "control snapshot rejected: view contains a WKWebView",
            || {
                let _operation = delegate.capture_operation("control snapshot")?;
                capture::snapshot_controls(&delegate.content(), &old_root_path)
            },
        ),
    );
    checks.add(
        "ungranted_capture_denied",
        policy_error(delegate.capture_selected(), "control denied: Denied"),
    );
    delegate.select_second(sel!(selectSecond:), delegate);
    checks.add(
        "select_second",
        require(
            delegate.ivars().selected.get() == SECONDARY
                && delegate
                    .selected_view()?
                    .isDescendantOf(&delegate.content()),
            "second tab is not visible and selected",
        ),
    );
    checks.add(
        "second_tab_controls",
        delegate.capture_controls(&directory.join("second-tab-controls.tiff")),
    );
    delegate.select_first(sel!(selectFirst:), delegate);
    delegate.grant(sel!(grant:), delegate);
    delegate.split(sel!(split:), delegate);
    checks.add("split_visible", split_geometry(delegate));
    for (name, slot) in [
        ("split_primary_engine_ready", &delegate.ivars().primary),
        ("split_secondary_engine_ready", &delegate.ivars().secondary),
    ] {
        let view = slot.borrow().clone().ok_or("missing split WebView")?;
        checks.add(name, wait_ready(&view));
    }
    checks.add(
        "split_controls",
        delegate.capture_controls(&directory.join("split-controls.tiff")),
    );
    checks.add(
        "split_left_capture",
        guarded_visible_snapshot(delegate, &directory.join("split-left.tiff")),
    );
    delegate.select_second(sel!(selectSecond:), delegate);
    delegate.grant(sel!(grant:), delegate);
    checks.add(
        "split_right_capture",
        guarded_visible_snapshot(delegate, &directory.join("split-right.tiff")),
    );
    delegate.select_first(sel!(selectFirst:), delegate);
    delegate.grant(sel!(grant:), delegate);
    delegate.split(sel!(split:), delegate);
    {
        let visible = delegate.selected_view()?;
        let first = delegate
            .ivars()
            .hidden_first
            .borrow()
            .clone()
            .ok_or("missing hidden first view")?;
        let second = delegate
            .ivars()
            .hidden_second
            .borrow()
            .clone()
            .ok_or("missing hidden second view")?;
        for (name, view, token) in [
            ("write_visible_storage", &visible, "visible"),
            ("write_hidden_first_storage", &first, "hidden-first"),
            ("write_hidden_second_storage", &second, "hidden-second"),
        ] {
            checks.add(name, write_storage(view, token));
        }
        for (name, view, token) in [
            ("visible_storage_independent", &visible, "visible"),
            ("hidden_first_storage_independent", &first, "hidden-first"),
            (
                "hidden_second_storage_independent",
                &second,
                "hidden-second",
            ),
        ] {
            checks.add(name, read_storage(view, token));
        }
        let secondary = delegate
            .ivars()
            .secondary
            .borrow()
            .clone()
            .ok_or("missing secondary view")?;
        checks.add(
            "visible_tabs_share_storage",
            wait_value(
                &secondary,
                "String(localStorage.getItem('isolation-token'))",
                "visible",
            ),
        );
        delegate.select_hidden(true)?;
        delegate.grant_hidden(true)?;
        delegate.revoke_hidden(true);
        checks.add(
            "hidden_revoke_denied",
            denied_snapshot_writes_nothing(
                &directory.join("hidden-revoked.tiff"),
                "control denied: Denied",
                || guarded_hidden_snapshot(delegate, true, &directory.join("hidden-revoked.tiff")),
            ),
        );
        delegate.grant_hidden(true)?;
        delegate.select_hidden(false)?;
        delegate.grant_hidden(false)?;
        checks.add(
            "hidden_first_capture",
            guarded_hidden_snapshot(delegate, true, &directory.join("hidden-first.tiff")),
        );
        checks.add(
            "hidden_second_capture",
            guarded_hidden_snapshot(delegate, false, &directory.join("hidden-second.tiff")),
        );
        checks.add(
            "hidden_first_recording",
            guarded_hidden_record(delegate, true, &directory.join("hidden-first-recording")),
        );
        checks.add(
            "hidden_second_recording",
            guarded_hidden_record(delegate, false, &directory.join("hidden-second-recording")),
        );
        for (name, view) in [
            ("visible_service_worker_ready", &visible),
            ("hidden_first_service_worker_ready", &first),
            ("hidden_second_service_worker_ready", &second),
        ] {
            checks.add(
                name,
                wait_value(
                    view,
                    "String(window.fixtureResults?.serviceWorker)",
                    "ready",
                ),
            );
        }
        let unregister = "(() => { window.workerRemoval = 'pending'; navigator.serviceWorker.getRegistrations().then(registrations => Promise.all(registrations.map(registration => registration.unregister()))).then(() => window.workerRemoval = 'done').catch(error => window.workerRemoval = 'error:' + error); return 'started'; })()";
        checks.add(
            "visible_worker_unregister",
            expect_value(&visible, unregister, "started")
                .and_then(|()| wait_value(&visible, "String(window.workerRemoval)", "done")),
        );
        let count_workers = "(() => { window.workerCount = 'pending'; navigator.serviceWorker.getRegistrations().then(registrations => window.workerCount = String(registrations.length)).catch(error => window.workerCount = 'error:' + error); return 'started'; })()";
        for (name, view, expected) in [
            ("visible_worker_removed", &visible, "0"),
            ("hidden_first_worker_independent", &first, "1"),
            ("hidden_second_worker_independent", &second, "1"),
        ] {
            checks.add(
                name,
                expect_value(view, count_workers, "started")
                    .and_then(|()| wait_value(view, "String(window.workerCount)", expected)),
            );
        }
        delegate
            .ivars()
            .hidden_first_policy
            .borrow_mut()
            .end_session();
        checks.add(
            "hidden_session_end_denied",
            denied_snapshot_writes_nothing(
                &directory.join("hidden-ended.tiff"),
                "control denied: SessionEnded",
                || guarded_hidden_snapshot(delegate, true, &directory.join("hidden-ended.tiff")),
            ),
        );
    }
    delegate.grant(sel!(grant:), delegate);
    let stale = delegate
        .ivars()
        .policy
        .borrow()
        .target(PRIMARY)
        .map_err(|error| format!("{error:?}"))?;
    checks.add(
        "granted_control_allowed",
        require(delegate.authorized_view().is_ok(), "granted target denied"),
    );
    delegate.select_second(sel!(selectSecond:), delegate);
    checks.add(
        "wrong_selection_denied",
        policy_error(
            delegate.authorized_view().map(|_| ()),
            "control denied: Denied",
        ),
    );
    delegate.select_first(sel!(selectFirst:), delegate);
    delegate
        .ivars()
        .address
        .get()
        .expect("address")
        .setStringValue(&NSString::from_str(
            &delegate.url("/smoke.html?navigation=2"),
        ));
    delegate.load_selected();
    checks.add(
        "navigation_ready",
        wait_value(
            &*delegate.selected_view()?,
            "String(location.search === '?navigation=2' && !!document.querySelector('#input'))",
            "true",
        ),
    );
    checks.add(
        "stale_navigation_denied",
        require(
            delegate.ivars().policy.borrow().authorize(stale)
                == Err(policy::PolicyError::StaleTarget),
            "old target did not return StaleTarget after navigation",
        ),
    );
    checks.add(
        "grant_survives_navigation",
        require(delegate.authorized_view().is_ok(), "tab grant was lost"),
    );
    checks.add(
        "revoke_while_snapshot_pending",
        revoke_while_snapshot_pending(delegate, &directory.join("revoked-snapshot.tiff")),
    );
    delegate.grant(sel!(grant:), delegate);
    checks.add(
        "recording_revoke_before_first_frame_denied",
        revoke_while_recording(delegate, &directory.join("revoked-recording")),
    );
    delegate.grant(sel!(grant:), delegate);
    checks.add(
        "recording_revoke_after_first_frame_retains_partial_evidence",
        revoke_mid_recording(delegate, &directory.join("revoked-mid-recording")),
    );
    delegate.grant(sel!(grant:), delegate);
    checks.add(
        "nested_control_capture_rejected",
        nested_control_capture_rejected(delegate, &directory.join("nested-controls.tiff")),
    );
    delegate.grant(sel!(grant:), delegate);
    checks.add("nested_record_rejected", nested_record_rejected(delegate));
    delegate.revoke(sel!(revoke:), delegate);
    checks.add(
        "revoked_capture_denied",
        policy_error(delegate.capture_selected(), "control denied: Denied"),
    );
    let selected = delegate.selected_view()?;
    let weak = Weak::from_retained(&selected);
    delegate.unload(sel!(unload:), delegate);
    drop(selected);
    checks.add(
        "unloaded_slot_empty",
        require(
            delegate.selected_view().is_err(),
            "unload retained the selected view slot",
        ),
    );
    let release_start = Instant::now();
    while weak.load().is_some() && release_start.elapsed() < Duration::from_secs(3) {
        capture::pump(0.05);
    }
    checks.add(
        "unloaded_view_released",
        require(
            weak.load().is_none(),
            "native view remains retained after unload; renderer memory not measured",
        ),
    );
    delegate.select_second(sel!(selectSecond:), delegate);
    delegate.select_first(sel!(selectFirst:), delegate);
    checks.add(
        "unloaded_selection_restores_committed_address",
        require(
            delegate
                .ivars()
                .address
                .get()
                .expect("address")
                .stringValue()
                .to_string()
                == delegate.url("/smoke.html?navigation=2"),
            "selecting the unloaded tab did not restore its committed address",
        ),
    );
    delegate.reload(sel!(reload:), delegate);
    checks.add(
        "reloaded_page_ready",
        wait_ready(&*delegate.selected_view()?),
    );
    checks.add(
        "reloaded_committed_location",
        expect_value(
            &*delegate.selected_view()?,
            "String(location.pathname + location.search)",
            "/smoke.html?navigation=2",
        ),
    );
    checks.add(
        "reloaded_committed_address_field",
        wait_for_address_field(delegate, &delegate.url("/smoke.html?navigation=2")),
    );
    checks.add(
        "reloaded_storage_restored",
        wait_value(
            &*delegate.selected_view()?,
            "String(localStorage.getItem('isolation-token'))",
            "visible",
        ),
    );
    delegate.grant(sel!(grant:), delegate);
    checks.add("selected_snapshot", delegate.capture_selected());
    checks.add("visible_recording", delegate.record_selected());
    checks.add(
        "final_native_controls",
        delegate.capture_controls(&directory.join("final-controls.tiff")),
    );
    let denied_before = delegate.ivars().denied_navigations.get();
    let rejected = NSURL::URLWithString(ns_string!("about:blank")).expect("blank address");
    unsafe {
        delegate
            .selected_view()?
            .loadRequest(&NSURLRequest::requestWithURL(&rejected));
    }
    let denial_start = Instant::now();
    while delegate.ivars().denied_navigations.get() == denied_before
        && denial_start.elapsed() < Duration::from_secs(3)
    {
        capture::pump(0.05);
    }
    checks.add(
        "nonfixture_navigation_denied",
        require(
            delegate.ivars().denied_navigations.get() > denied_before,
            "navigation policy did not reject the nonfixture address",
        ),
    );
    checks.report("certificate_rejection", "not_run", "A local invalid-certificate fixture is required before external-site tests. No certificate bypass is configured.".into());
    checks.report("permission_state_independence", "blocked", "Mutating system permission decisions would require a separate authorization; this run requests no device permission.".into());
    let target = delegate
        .ivars()
        .policy
        .borrow()
        .target(PRIMARY)
        .map_err(|error| format!("{error:?}"))?;
    delegate.drop_views();
    checks.add(
        "session_end_denies_control",
        require(
            delegate.ivars().policy.borrow().authorize(target)
                == Err(policy::PolicyError::SessionEnded),
            "control did not return SessionEnded after teardown",
        ),
    );
    checks.add(
        "closed_tab_removed",
        require(
            delegate.ivars().policy.borrow().target(PRIMARY)
                == Err(policy::PolicyError::UnknownTab),
            "closed tab did not return UnknownTab",
        ),
    );
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn escapes_json_control_characters() {
        assert_eq!(
            json_string("line\n\"\\\t"),
            "\"line\\u000a\\\"\\\\\\u0009\""
        );
    }
}
