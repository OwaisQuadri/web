use std::cell::Cell;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use objc2::rc::{Retained, Weak};
use objc2::{AnyThread, MainThreadOnly};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSAutoresizingMaskOptions, NSBackingStoreType,
    NSWindow, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSDate, NSDictionary, NSHTTPCookie, NSPoint, NSRect, NSSize, NSString, NSURL,
    NSURLRequest, NSUUID,
};
use objc2_web_kit::{WKFullscreenState, WKWebView, WKWebViewConfiguration, WKWebsiteDataStore};
use serde::{Deserialize, Serialize};

use crate::capture;

const READY_TIMEOUT: Duration = Duration::from_secs(60);
const STOP_TIMEOUT: Duration = Duration::from_secs(60);
const WIDTH: f64 = 800.0;
const HEIGHT: f64 = 600.0;
const YOUTUBE_URL: &str = "https://www.youtube.com/watch?v=M7lc1UVf-VE";

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
struct MediaStatus {
    current_time: f64,
    duration: f64,
    is_fullscreen: bool,
    is_muted: bool,
    is_paused: bool,
    is_seeking: bool,
    ready_state: u64,
    video_height: u64,
    video_width: u64,
    volume: f64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct MemoryArguments {
    pub(crate) base: String,
    pub(crate) evidence: PathBuf,
    pub(crate) scenario_id: String,
    pub(crate) tab_count: usize,
    pub(crate) background_state: String,
    pub(crate) layout: String,
}

impl MemoryArguments {
    pub(crate) fn parse(values: &[String]) -> Result<Self, String> {
        let [
            base,
            evidence,
            scenario_id,
            tab_count,
            background_state,
            layout,
        ] = values
        else {
            return Err("usage: --memory-smoke <fixture-origin> <evidence-directory> <scenario-id> <tab-count> <background-state> <layout>".into());
        };
        let tab_count = tab_count
            .parse::<usize>()
            .map_err(|_| "tab-count must be an integer")?;
        let is_valid = matches!(
            (background_state.as_str(), layout.as_str(), tab_count),
            ("loaded", "single", 7 | 12 | 20) | ("unloaded", "single", 20) | ("loaded", "split", 7)
        );
        let expected_scenario = format!("{background_state}-{layout}-{tab_count}");
        if !is_valid || scenario_id != &expected_scenario {
            return Err("accepted combinations are loaded-single-7/12/20, unloaded-single-20, and loaded-split-7".into());
        }
        Ok(Self {
            base: base.clone(),
            evidence: evidence.into(),
            scenario_id: scenario_id.clone(),
            tab_count,
            background_state: background_state.clone(),
            layout: layout.clone(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum CookieStage {
    Write,
    Read,
}

#[derive(Clone, Debug)]
pub(crate) struct CookieArguments {
    pub(crate) base: String,
    evidence: PathBuf,
    stage: CookieStage,
    token: String,
}

impl CookieArguments {
    pub(crate) fn new(
        base: String,
        evidence: PathBuf,
        stage: &str,
        token: &str,
    ) -> Result<Self, String> {
        let stage = match stage {
            "write" => CookieStage::Write,
            "read" => CookieStage::Read,
            _ => return Err("cookie stage must be write or read".into()),
        };
        if !(16..=64).contains(&token.len())
            || !token.bytes().all(|byte| byte.is_ascii_alphanumeric())
        {
            return Err("cookie token must contain 16 to 64 ASCII letters or digits".into());
        }
        Ok(Self {
            base,
            evidence,
            stage,
            token: token.to_owned(),
        })
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum Role {
    Visible,
    Background,
    HeadlessA,
    HeadlessB,
}

struct Record {
    role: Role,
    url: String,
    is_released: bool,
    view: Option<Retained<WKWebView>>,
    weak: Option<Weak<WKWebView>>,
}

pub(crate) fn run(mtm: MainThreadMarker, arguments: MemoryArguments) -> Result<(), String> {
    reject_existing(&arguments.evidence)?;
    fs::create_dir_all(&arguments.evidence).map_err(|error| error.to_string())?;
    let started = Instant::now();
    let visible_store = unsafe { WKWebsiteDataStore::nonPersistentDataStore(mtm) };
    let headless_first_store = unsafe { WKWebsiteDataStore::nonPersistentDataStore(mtm) };
    let headless_second_store = unsafe { WKWebsiteDataStore::nonPersistentDataStore(mtm) };
    let window = make_window(mtm, NSWindowStyleMask::Titled | NSWindowStyleMask::Closable)?;
    window.setFrameOrigin(NSPoint::new(100.0, 100.0));
    window.setTitle(&NSString::from_str("WebKit memory benchmark"));
    let mut visible = make_records(&arguments.base, arguments.tab_count, &visible_store, mtm)?;
    let mut headless = vec![
        make_record(
            &arguments.base,
            "article.html",
            Role::HeadlessA,
            &headless_first_store,
            mtm,
        )?,
        make_record(
            &arguments.base,
            "application.html",
            Role::HeadlessB,
            &headless_second_store,
            mtm,
        )?,
    ];
    let mut host_windows = Vec::new();
    for record in &mut headless {
        let host = make_window(mtm, NSWindowStyleMask::Borderless)?;
        host.setAlphaValue(0.0);
        host.setIgnoresMouseEvents(true);
        host.setContentView(record.view.as_deref().map(|view| &**view));
        host.orderFront(None);
        host_windows.push(host);
    }
    attach_visible(&window, &mut visible, &arguments.layout, None)?;
    let application = NSApplication::sharedApplication(mtm);
    let _ = application.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[expect(deprecated, reason = "unbundled Cargo sample needs AppKit activation")]
    application.activateIgnoringOtherApps(true);
    window.makeKeyAndOrderFront(None);
    wait_ready(&mut visible, &mut headless, &arguments, &window)?;
    let ready_ms = started.elapsed().as_millis();
    write_atomic(
        &arguments.evidence.join("memory-ready-v1.json"),
        &ready_document(&arguments, &visible, &headless, &window, ready_ms),
    )?;
    println!("WEBKIT_MEMORY ready");
    wait_stop(&arguments.evidence)?;
    release_records(&mut visible);
    release_records(&mut headless);
    for host in host_windows {
        host.setContentView(None);
        host.close();
    }
    window.close();
    let shutdown_deadline = Instant::now() + Duration::from_secs(5);
    while !is_all_released(&visible) || !is_all_released(&headless) {
        if is_deadline_expired(shutdown_deadline) {
            break;
        }
        capture::pump(0.05);
    }
    let is_native_release_observed = is_all_released(&visible) && is_all_released(&headless);
    write_atomic(
        &arguments.evidence.join("memory-shutdown-v1.json"),
        &shutdown_document(&arguments, &visible, &headless, is_native_release_observed),
    )?;
    println!("WEBKIT_MEMORY shutdown");
    drop(visible_store);
    drop(headless_first_store);
    drop(headless_second_store);
    Ok(())
}

fn make_window(
    mtm: MainThreadMarker,
    style: NSWindowStyleMask,
) -> Result<Retained<NSWindow>, String> {
    let window = unsafe {
        NSWindow::initWithContentRect_styleMask_backing_defer(
            NSWindow::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT)),
            style,
            NSBackingStoreType::Buffered,
            false,
        )
    };
    unsafe { window.setReleasedWhenClosed(false) };
    Ok(window)
}
fn make_records(
    base: &str,
    count: usize,
    store: &WKWebsiteDataStore,
    mtm: MainThreadMarker,
) -> Result<Vec<Record>, String> {
    ["article.html", "images.html", "application.html"]
        .into_iter()
        .cycle()
        .take(count)
        .enumerate()
        .map(|(index, route)| {
            make_record(
                base,
                route,
                if index == 0 {
                    Role::Visible
                } else {
                    Role::Background
                },
                store,
                mtm,
            )
        })
        .collect()
}
fn make_record(
    base: &str,
    route: &str,
    role: Role,
    store: &WKWebsiteDataStore,
    mtm: MainThreadMarker,
) -> Result<Record, String> {
    let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
    unsafe { configuration.setWebsiteDataStore(store) };
    let view = unsafe {
        WKWebView::initWithFrame_configuration(
            WKWebView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT)),
            &configuration,
        )
    };
    let url = format!("{base}/{route}");
    let native_url =
        NSURL::URLWithString(&NSString::from_str(&url)).ok_or("invalid fixture URL")?;
    unsafe {
        view.loadRequest(&NSURLRequest::requestWithURL(&native_url));
    }
    Ok(Record {
        role,
        url,
        is_released: false,
        weak: Some(Weak::from_retained(&view)),
        view: Some(view),
    })
}
fn attach_visible(
    window: &NSWindow,
    records: &mut [Record],
    layout: &str,
    selected: Option<usize>,
) -> Result<(), String> {
    let content = window
        .contentView()
        .ok_or("visible window lacks content view")?;
    for (index, record) in records.iter_mut().enumerate() {
        if let Some(view) = record.view.as_ref() {
            view.removeFromSuperview();
            let is_show = (layout == "split" && index < 2) || selected == Some(index);
            if is_show {
                let (x, width) = if layout == "split" {
                    if index == 0 {
                        (0.0, WIDTH / 2.0)
                    } else {
                        (WIDTH / 2.0, WIDTH / 2.0)
                    }
                } else {
                    (0.0, WIDTH)
                };
                view.setFrame(NSRect::new(
                    NSPoint::new(x, 0.0),
                    NSSize::new(width, HEIGHT),
                ));
                content.addSubview(view);
            }
        }
    }
    Ok(())
}
fn wait_ready(
    visible: &mut [Record],
    headless: &mut [Record],
    arguments: &MemoryArguments,
    window: &NSWindow,
) -> Result<(), String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    for index in 0..visible.len() {
        attach_visible(window, visible, "single", Some(index))?;
        ready_and_act(&mut visible[index], deadline)?;
        if is_unloaded_background(arguments, index) {
            release_record(&mut visible[index]);
        }
    }
    for record in headless {
        ready_and_act(record, deadline)?;
    }
    if arguments.layout == "split" {
        attach_visible(window, visible, "split", None)?;
    } else {
        attach_visible(window, visible, "single", Some(0))?;
    }
    Ok(())
}
fn content_condition(kind: &str) -> &'static str {
    match kind {
        "article" => "value.details.paragraphs === 100",
        "images" => "value.details.images === 3 && value.details.pixels === 2359296",
        "application" => "value.details.rows === 2000",
        _ => "false",
    }
}

fn status_script(kind: &str) -> String {
    format!(
        "(() => {{ if (typeof window.webBenchmarkStatus !== 'function') return 'pending'; const value = window.webBenchmarkStatus(); return value.ready === true && value.kind === {} && value.actionCount === 0 && {} ? 'ready' : 'pending'; }})()",
        json(kind),
        content_condition(kind)
    )
}

fn action_script(kind: &str) -> String {
    format!(
        "(() => {{ const value = window.webBenchmarkAct(); return value.ready === true && value.kind === {} && value.actionCount === 1 && {} ? 'acted' : 'invalid'; }})()",
        json(kind),
        content_condition(kind)
    )
}

fn ready_and_act(record: &mut Record, deadline: Instant) -> Result<(), String> {
    let view = record
        .view
        .as_ref()
        .ok_or("visible page was released before readiness")?;
    let status_script = status_script(expected_kind(&record.url));
    loop {
        if capture::evaluate(view, &status_script)? == "ready" {
            break;
        }
        if is_deadline_expired(deadline) {
            return Err("60-second readiness deadline elapsed".into());
        }
        capture::pump(0.05);
    }
    let acted = capture::evaluate(view, &action_script(expected_kind(&record.url)))?;
    if acted != "acted" {
        return Err("webBenchmarkAct did not produce a typed actionCount=1 result".into());
    }
    Ok(())
}
fn is_unloaded_background(arguments: &MemoryArguments, index: usize) -> bool {
    arguments.background_state == "unloaded" && index > 0
}

fn is_deadline_expired(deadline: Instant) -> bool {
    Instant::now() >= deadline
}

fn expected_kind(url: &str) -> &str {
    if url.ends_with("images.html") {
        "images"
    } else if url.ends_with("application.html") {
        "application"
    } else {
        "article"
    }
}
fn release_record(record: &mut Record) {
    if record.view.is_none() {
        return;
    }
    objc2::rc::autoreleasepool(|_| {
        let view = record.view.take().expect("checked view");
        unsafe {
            view.setNavigationDelegate(None);
            view.stopLoading();
        }
        view.removeFromSuperviewWithoutNeedingDisplay();
    });
    record.is_released = true;
}
fn release_records(records: &mut [Record]) {
    for record in records {
        release_record(record);
    }
}
fn is_native_release_observed(record: &Record) -> bool {
    record.is_released
        && record
            .weak
            .as_ref()
            .is_none_or(|weak| weak.load().is_none())
}
fn is_all_released(records: &[Record]) -> bool {
    records.iter().all(is_native_release_observed)
}
fn wait_stop(directory: &Path) -> Result<(), String> {
    let deadline = Instant::now() + STOP_TIMEOUT;
    let stop = directory.join("stop");
    while !stop.exists() {
        if is_deadline_expired(deadline) {
            return Err("60-second stop deadline elapsed without stop file".into());
        }
        capture::pump(0.05);
    }
    Ok(())
}
fn reject_existing(directory: &Path) -> Result<(), String> {
    for name in [
        "memory-ready-v1.json",
        "memory-shutdown-v1.json",
        "memory-ready-v1.json.pending",
        "memory-shutdown-v1.json.pending",
    ] {
        if directory.join(name).exists() {
            return Err(format!("evidence file already exists: {name}"));
        }
    }
    Ok(())
}
fn write_atomic(path: &Path, document: &str) -> Result<(), String> {
    let pending = path.with_extension("json.pending");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&pending)
        .map_err(|error| error.to_string())?;
    file.write_all(document.as_bytes())
        .map_err(|error| error.to_string())?;
    file.sync_all().map_err(|error| error.to_string())?;
    fs::hard_link(&pending, path)
        .map_err(|error| format!("refusing to overwrite {}: {error}", path.display()))?;
    fs::remove_file(pending).map_err(|error| error.to_string())
}
fn json(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}
fn records_json(records: &[Record]) -> String {
    records
        .iter()
        .map(|record| {
            format!(
                "{{\"role\":{},\"url\":{},\"is_owned_reference_released\":{},\"is_native_release_observed\":{}}}",
                json(match record.role {
                    Role::Visible => "visible",
                    Role::Background => "background",
                    Role::HeadlessA => "headless-a",
                    Role::HeadlessB => "headless-b",
                }),
                json(&record.url),
                record.is_released,
                is_native_release_observed(record)
            )
        })
        .collect::<Vec<_>>()
        .join(",")
}
fn ready_document(
    arguments: &MemoryArguments,
    visible: &[Record],
    headless: &[Record],
    window: &NSWindow,
    ready_ms: u128,
) -> String {
    format!(
        "{{\"schema_version\":1,\"candidate\":\"webkit\",\"classification\":\"functional-regression-with-system-service-attribution-pending\",\"state\":\"ready\",\"scenario_id\":{},\"tab_count\":{},\"background_state\":{},\"layout\":{},\"host_process_id\":{},\"profile_mode\":\"off-the-record\",\"logical_content_bounds\":{{\"width\":800,\"height\":600}},\"observed_backing_scale\":{},\"startup_to_ready_ms\":{},\"process_attribution\":\"incomplete_system_services\",\"visible\":[{}],\"headless\":[{}]}}\n",
        json(&arguments.scenario_id),
        arguments.tab_count,
        json(&arguments.background_state),
        json(&arguments.layout),
        std::process::id(),
        window.backingScaleFactor(),
        ready_ms,
        records_json(visible),
        records_json(headless)
    )
}
fn shutdown_document(
    arguments: &MemoryArguments,
    visible: &[Record],
    headless: &[Record],
    is_native_release_observed: bool,
) -> String {
    format!(
        "{{\"schema_version\":1,\"candidate\":\"webkit\",\"classification\":\"functional-regression-with-system-service-attribution-pending\",\"state\":\"shutdown\",\"scenario_id\":{},\"host_process_id\":{},\"is_native_release_observed\":{},\"visible\":[{}],\"headless\":[{}]}}\n",
        json(&arguments.scenario_id),
        std::process::id(),
        is_native_release_observed,
        records_json(visible),
        records_json(headless)
    )
}

fn wait_script_value(
    view: &WKWebView,
    script: &str,
    expected: &str,
    failure: &str,
) -> Result<(), String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    loop {
        let observation = match capture::evaluate(view, script) {
            Ok(value) if value == expected => return Ok(()),
            Ok(value) => format!("value={value:?}"),
            Err(error) => format!("error={error}"),
        };
        if is_deadline_expired(deadline) {
            return Err(format!("{failure}; last observation: {observation}"));
        }
        capture::pump(0.05);
    }
}

fn persistent_cookie(url: &NSURL, token: &str) -> Result<Retained<NSHTTPCookie>, String> {
    let key = NSString::from_str("Set-Cookie");
    let value = NSString::from_str(&format!(
        "web-session-proof={token}; Path=/; Max-Age=3600; SameSite=Strict"
    ));
    let headers = NSDictionary::from_slices(&[&*key], &[&*value]);
    NSHTTPCookie::cookiesWithResponseHeaderFields_forURL(&headers, url)
        .firstObject()
        .ok_or_else(|| "Foundation rejected the persistent cookie".into())
}

fn wait_cookie_operation(completed: &Cell<bool>, failure: &str) -> Result<(), String> {
    let deadline = Instant::now() + READY_TIMEOUT;
    while !completed.get() {
        if is_deadline_expired(deadline) {
            return Err(failure.into());
        }
        capture::pump(0.05);
    }
    Ok(())
}

fn set_persistent_cookie(
    store: &WKWebsiteDataStore,
    url: &NSURL,
    token: &str,
) -> Result<(), String> {
    let cookie = persistent_cookie(url, token)?;
    let completed = std::rc::Rc::new(Cell::new(false));
    let callback_completed = std::rc::Rc::clone(&completed);
    let callback = block2::RcBlock::new(move || callback_completed.set(true));
    let cookie_store = unsafe { store.httpCookieStore() };
    unsafe { cookie_store.setCookie_completionHandler(&cookie, Some(&callback)) };
    wait_cookie_operation(
        &completed,
        "persistent cookie store did not complete its write",
    )
}

fn delete_persistent_cookie(
    store: &WKWebsiteDataStore,
    url: &NSURL,
    token: &str,
) -> Result<(), String> {
    let cookie = persistent_cookie(url, token)?;
    let completed = std::rc::Rc::new(Cell::new(false));
    let callback_completed = std::rc::Rc::clone(&completed);
    let callback = block2::RcBlock::new(move || callback_completed.set(true));
    let cookie_store = unsafe { store.httpCookieStore() };
    unsafe { cookie_store.deleteCookie_completionHandler(&cookie, Some(&callback)) };
    wait_cookie_operation(
        &completed,
        "persistent cookie store did not complete its deletion",
    )
}

fn clear_persistent_website_data(
    mtm: MainThreadMarker,
    store: &WKWebsiteDataStore,
) -> Result<(), String> {
    let completed = std::rc::Rc::new(Cell::new(false));
    let callback_completed = std::rc::Rc::clone(&completed);
    let callback = block2::RcBlock::new(move || callback_completed.set(true));
    let data_types = unsafe { WKWebsiteDataStore::allWebsiteDataTypes(mtm) };
    unsafe {
        store.removeDataOfTypes_modifiedSince_completionHandler(
            &data_types,
            &NSDate::distantPast(),
            &callback,
        )
    };
    wait_cookie_operation(
        &completed,
        "persistent website data cleanup did not complete",
    )
}

fn load_cookie_fixture(view: &WKWebView, url: &NSURL) -> Result<(), String> {
    unsafe { view.loadRequest(&NSURLRequest::requestWithURL(url)) };
    wait_script_value(
        view,
        "document.readyState",
        "complete",
        "cookie fixture did not finish loading",
    )
}

pub(crate) fn run_cookie(mtm: MainThreadMarker, arguments: CookieArguments) -> Result<(), String> {
    fs::create_dir_all(&arguments.evidence).map_err(|error| error.to_string())?;
    let receipt_name = match arguments.stage {
        CookieStage::Write => "cookie-write-v1.json",
        CookieStage::Read => "cookie-read-v1.json",
    };
    if arguments.evidence.join(receipt_name).exists() {
        return Err(format!("evidence file already exists: {receipt_name}"));
    }
    let store_identifier = NSUUID::initWithUUIDString(
        NSUUID::alloc(),
        &NSString::from_str("79B5A5AF-6E3E-4C65-9929-9B9C42BBE3A9"),
    )
    .ok_or("persistent cookie store identifier is invalid")?;
    let store = unsafe { WKWebsiteDataStore::dataStoreForIdentifier(&store_identifier, mtm) };
    let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
    unsafe { configuration.setWebsiteDataStore(&store) };
    let view = unsafe {
        WKWebView::initWithFrame_configuration(
            WKWebView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT)),
            &configuration,
        )
    };
    let address = format!("{}/cookie.html", arguments.base);
    let url = NSURL::URLWithString(&NSString::from_str(&address))
        .ok_or("cookie fixture URL is invalid")?;
    let cookie_script = "(() => { const entry=document.cookie.split('; ').find(value=>value.startsWith('web-session-proof=')); return entry ? entry.slice('web-session-proof='.length) : ''; })()";
    match arguments.stage {
        CookieStage::Write => {
            load_cookie_fixture(&view, &url)?;
            set_persistent_cookie(&store, &url, &arguments.token)?;
            wait_script_value(
                &view,
                cookie_script,
                &arguments.token,
                "persistent cookie was not readable after write",
            )?;
            capture::pump(2.0);
        }
        CookieStage::Read => {
            let read_result = (|| {
                load_cookie_fixture(&view, &url)?;
                wait_script_value(
                    &view,
                    cookie_script,
                    &arguments.token,
                    "persistent cookie did not survive process restart",
                )?;
                delete_persistent_cookie(&store, &url, &arguments.token)?;
                wait_script_value(
                    &view,
                    cookie_script,
                    "",
                    "persistent cookie remained after cleanup",
                )
            })();
            let cleanup_result = clear_persistent_website_data(mtm, &store);
            read_result?;
            cleanup_result?;
        }
    }
    let document = serde_json::json!({
        "schema_version": 1,
        "candidate": "webkit",
        "profile_mode": "persistent",
        "stage": arguments.stage,
        "state": "passed",
        "fixture_origin": arguments.base,
        "host_process_id": std::process::id(),
        "token_length": arguments.token.len(),
    });
    write_atomic(
        &arguments.evidence.join(receipt_name),
        &serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?,
    )?;
    unsafe { view.stopLoading() };
    view.removeFromSuperviewWithoutNeedingDisplay();
    capture::pump(0.2);
    Ok(())
}

fn media_status_script() -> &'static str {
    "(() => { const video = document.querySelector('video'); if (!video) return ''; return JSON.stringify({currentTime:video.currentTime,duration:video.duration,isFullscreen:document.fullscreenElement!==null||video.webkitDisplayingFullscreen===true,isMuted:video.muted,isPaused:video.paused,isSeeking:video.seeking,readyState:video.readyState,videoHeight:video.videoHeight,videoWidth:video.videoWidth,volume:video.volume}); })()"
}

fn read_media_status(view: &WKWebView) -> Option<MediaStatus> {
    capture::evaluate(view, media_status_script())
        .ok()
        .and_then(|value| serde_json::from_str(&value).ok())
}

fn wait_media_status(
    view: &WKWebView,
    deadline: Instant,
    condition: impl Fn(&MediaStatus) -> bool,
    failure: &str,
) -> Result<MediaStatus, String> {
    loop {
        if let Some(status) = read_media_status(view)
            && condition(&status)
        {
            return Ok(status);
        }
        if is_deadline_expired(deadline) {
            return Err(failure.to_owned());
        }
        capture::pump(0.05);
    }
}

const MEDIA_CYCLE_SLOTS: usize = 4;
const MEDIA_CYCLE_TAB_RECORDS: usize = 20;
const MEDIA_CYCLE_CLASSIFICATION: &str =
    "one-live-media-slot-cycle-with-retained-snapshots-and-metadata";
const MEDIA_CYCLE_PROFILE_MODE: &str = "off-the-record";
const BUILD_SOURCE_MANIFEST_SHA256: &str = match option_env!("WEBKIT_SOURCE_MANIFEST_SHA256") {
    Some(value) => value,
    None => "unbound-development-build",
};
const NATIVE_RELEASE_TIMEOUT: Duration = Duration::from_secs(3);

fn wait_marker(path: &Path, failure: &str) -> Result<(), String> {
    let deadline = Instant::now() + STOP_TIMEOUT;
    while !path.exists() {
        if is_deadline_expired(deadline) {
            return Err(failure.to_owned());
        }
        capture::pump(0.05);
    }
    Ok(())
}

fn write_media_status(path: &Path, state: &str, status: &MediaStatus) -> Result<(), String> {
    let document = serde_json::json!({
        "schema_version": 1,
        "candidate": "webkit",
        "classification": "youtube-playback-state-not-acoustic-capture",
        "state": state,
        "url": YOUTUBE_URL,
        "host_process_id": std::process::id(),
        "video": status,
    });
    write_atomic(
        path,
        &serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?,
    )
}

fn media_cycle_records(
    slot: usize,
    is_live: bool,
) -> (Vec<serde_json::Value>, Vec<serde_json::Value>) {
    let tabs = (0..MEDIA_CYCLE_TAB_RECORDS)
        .map(|tab_index| {
            let state = if tab_index < MEDIA_CYCLE_SLOTS {
                if tab_index == slot && is_live {
                    "live"
                } else if tab_index <= slot {
                    "snapshot-retained"
                } else {
                    "metadata-retained"
                }
            } else {
                "metadata-retained"
            };
            serde_json::json!({
                "tab_index": tab_index,
                "url": YOUTUBE_URL,
                "state": state,
                "slot": if tab_index < MEDIA_CYCLE_SLOTS { Some(tab_index) } else { None::<usize> },
                "snapshot_path": if tab_index < slot || (!is_live && tab_index == slot) {
                    Some(format!("media-cycle-slot-{tab_index}-window.png"))
                } else {
                    None
                },
            })
        })
        .collect::<Vec<_>>();
    let recent_slots = tabs[..MEDIA_CYCLE_SLOTS].to_vec();
    (tabs, recent_slots)
}

fn media_cycle_document(
    slot: usize,
    state: &str,
    video: Option<&MediaStatus>,
) -> serde_json::Value {
    let is_live = state == "ready";
    let (tab_records, recent_slots) = media_cycle_records(slot, is_live);
    serde_json::json!({
        "schema_version": 1,
        "candidate": "webkit",
        "classification": MEDIA_CYCLE_CLASSIFICATION,
        "state": state,
        "slot": slot,
        "tab_record_count": tab_records.len(),
        "recent_slot_count": recent_slots.len(),
        "live_slot_count": usize::from(is_live),
        "profile_mode": MEDIA_CYCLE_PROFILE_MODE,
        "host_process_id": std::process::id(),
        "build_source_manifest_sha256": BUILD_SOURCE_MANIFEST_SHA256,
        "tab_records": tab_records,
        "recent_slots": recent_slots,
        "video": video,
    })
}

fn write_media_cycle_ready_receipt(
    evidence: &Path,
    window: &NSWindow,
    slot: usize,
    status: &MediaStatus,
) -> Result<(), String> {
    let mut document = media_cycle_document(slot, "ready", Some(status));
    document["window_number"] = serde_json::json!(window.windowNumber());
    write_atomic(
        &evidence.join(format!("media-cycle-slot-{slot}-ready-v1.json")),
        &serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?,
    )
}

fn write_media_cycle_release_receipt(
    evidence: &Path,
    slot: usize,
    is_native_release_observed: bool,
) -> Result<(), String> {
    let state = if slot + 1 == MEDIA_CYCLE_SLOTS {
        "release-observation-before-process-exit"
    } else {
        "release-observation-before-next-slot"
    };
    let mut document = media_cycle_document(slot, state, None);
    document["is_owned_reference_released"] = serde_json::Value::Bool(true);
    document["is_native_release_observed"] = serde_json::Value::Bool(is_native_release_observed);
    write_atomic(
        &evidence.join(format!("media-cycle-slot-{slot}-release-v1.json")),
        &serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?,
    )
}

fn observe_media_release(weak: &Weak<WKWebView>) -> bool {
    let deadline = Instant::now() + NATIVE_RELEASE_TIMEOUT;
    while weak.load().is_some() && !is_deadline_expired(deadline) {
        capture::pump(0.05);
    }
    weak.load().is_none()
}

fn require_native_release(is_native_release_observed: bool, slot: usize) -> Result<(), String> {
    if is_native_release_observed {
        Ok(())
    } else {
        Err(format!(
            "media-cycle slot {slot}: native view release was not observed"
        ))
    }
}

fn reject_existing_media_cycle(evidence: &Path) -> Result<(), String> {
    for slot in 0..MEDIA_CYCLE_SLOTS {
        let mut names = vec![
            format!("media-cycle-slot-{slot}-ready-v1.json"),
            format!("media-cycle-slot-{slot}-measured"),
            format!("media-cycle-slot-{slot}-window.png"),
        ];
        names.push(format!("media-cycle-slot-{slot}-release-v1.json"));
        if slot + 1 < MEDIA_CYCLE_SLOTS {
            names.push(format!("media-cycle-slot-{slot}-release-observed"));
            names.push(format!(
                "media-cycle-slot-{slot}-renderer-release-observation-v1.json"
            ));
        }
        for name in names {
            if evidence.join(&name).exists() || evidence.join(format!("{name}.pending")).exists() {
                return Err(format!("evidence file already exists: {name}"));
            }
        }
    }
    for name in [
        "media-cycle-complete-v1.json",
        "media-cycle-complete-v1.json.pending",
    ] {
        if evidence.join(name).exists() {
            return Err(format!("evidence file already exists: {name}"));
        }
    }
    Ok(())
}

fn make_media_view(mtm: MainThreadMarker, store: &WKWebsiteDataStore) -> Retained<WKWebView> {
    let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
    unsafe { configuration.setWebsiteDataStore(store) };
    let view = unsafe {
        WKWebView::initWithFrame_configuration(
            WKWebView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT)),
            &configuration,
        )
    };
    view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    view
}

fn load_media_view(view: &WKWebView) -> Result<(), String> {
    let url = NSURL::URLWithString(&NSString::from_str(YOUTUBE_URL))
        .ok_or("fixed media-cycle URL is invalid")?;
    unsafe { view.loadRequest(&NSURLRequest::requestWithURL(&url)) };
    Ok(())
}

fn run_media_cycle_slot(
    mtm: MainThreadMarker,
    store: &WKWebsiteDataStore,
    window: &NSWindow,
    evidence: &Path,
    slot: usize,
) -> Result<Weak<WKWebView>, String> {
    objc2::rc::autoreleasepool(|_| {
        let view = make_media_view(mtm, store);
        window.setContentView(Some(&view));
        load_media_view(&view)?;
        let status = wait_media_status(
            &view,
            Instant::now() + READY_TIMEOUT,
            |status| status.ready_state >= 2 && status.video_width > 0 && status.video_height > 0,
            "a visible media-cycle slot did not load YouTube video",
        )
        .map_err(|error| format!("media-cycle slot {slot}: {error}"))?;
        write_media_cycle_ready_receipt(evidence, window, slot, &status)?;
        println!("WEBKIT_MEDIA_CYCLE slot={slot} ready");
        wait_marker(
            &evidence.join(format!("media-cycle-slot-{slot}-measured")),
            "media-cycle measurement marker did not arrive",
        )?;

        unsafe { view.stopLoading() };
        window.setContentView(None);
        view.removeFromSuperviewWithoutNeedingDisplay();
        let weak = Weak::from_retained(&view);
        drop(view);
        Ok(weak)
    })
}

pub(crate) fn run_media_memory(mtm: MainThreadMarker, evidence: PathBuf) -> Result<(), String> {
    fs::create_dir_all(&evidence).map_err(|error| error.to_string())?;
    reject_existing_media_cycle(&evidence)?;
    let store = unsafe { WKWebsiteDataStore::nonPersistentDataStore(mtm) };
    let window = make_window(
        mtm,
        NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable,
    )?;
    window.setFrameOrigin(NSPoint::new(100.0, 100.0));
    window.setTitle(&NSString::from_str("WebKit one-live media memory cycle"));
    let application = NSApplication::sharedApplication(mtm);
    let _ = application.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[expect(deprecated, reason = "unbundled Cargo sample needs AppKit activation")]
    application.activateIgnoringOtherApps(true);
    window.makeKeyAndOrderFront(None);

    let result = (|| {
        for slot in 0..MEDIA_CYCLE_SLOTS {
            let weak = run_media_cycle_slot(mtm, &store, &window, &evidence, slot)?;
            let is_native_release_observed = observe_media_release(&weak);
            write_media_cycle_release_receipt(&evidence, slot, is_native_release_observed)?;
            require_native_release(is_native_release_observed, slot)?;
            println!("WEBKIT_MEDIA_CYCLE slot={slot} released");
            if slot + 1 < MEDIA_CYCLE_SLOTS {
                wait_marker(
                    &evidence.join(format!("media-cycle-slot-{slot}-release-observed")),
                    "media-cycle renderer release observation did not arrive",
                )?;
            }
        }
        let document = media_cycle_document(MEDIA_CYCLE_SLOTS - 1, "complete", None);
        write_atomic(
            &evidence.join("media-cycle-complete-v1.json"),
            &serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?,
        )?;
        println!("WEBKIT_MEDIA_CYCLE complete");
        Ok(())
    })();
    window.setContentView(None);
    window.close();
    result
}

pub(crate) fn run_youtube(mtm: MainThreadMarker, evidence: PathBuf) -> Result<(), String> {
    fs::create_dir_all(&evidence).map_err(|error| error.to_string())?;
    for name in [
        "youtube-ready-v1.json",
        "youtube-playback-v1.json",
        "youtube-seek-v1.json",
        "youtube-fullscreen-entered-v1.json",
        "youtube-fullscreen-v1.json",
        "youtube-fullscreen-native-v1.json",
        "youtube-shutdown-v1.json",
    ] {
        if evidence.join(name).exists() || evidence.join(format!("{name}.pending")).exists() {
            return Err(format!("evidence file already exists: {name}"));
        }
    }
    let store = unsafe { WKWebsiteDataStore::nonPersistentDataStore(mtm) };
    let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
    unsafe {
        configuration.setWebsiteDataStore(&store);
        configuration
            .preferences()
            .setElementFullscreenEnabled(true);
    }
    let view = unsafe {
        WKWebView::initWithFrame_configuration(
            WKWebView::alloc(mtm),
            NSRect::new(NSPoint::new(0.0, 0.0), NSSize::new(WIDTH, HEIGHT)),
            &configuration,
        )
    };
    view.setAutoresizingMask(
        NSAutoresizingMaskOptions::ViewWidthSizable | NSAutoresizingMaskOptions::ViewHeightSizable,
    );
    let url = NSURL::URLWithString(&NSString::from_str(YOUTUBE_URL))
        .ok_or("fixed YouTube URL is invalid")?;
    unsafe {
        view.loadRequest(&NSURLRequest::requestWithURL(&url));
    }
    let window = make_window(
        mtm,
        NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::Resizable,
    )?;
    window.setFrameOrigin(NSPoint::new(100.0, 100.0));
    window.setTitle(&NSString::from_str("WebKit YouTube playback test"));
    window.setContentView(Some(&view));
    let application = NSApplication::sharedApplication(mtm);
    let _ = application.setActivationPolicy(NSApplicationActivationPolicy::Regular);
    #[expect(deprecated, reason = "unbundled Cargo sample needs AppKit activation")]
    application.activateIgnoringOtherApps(true);
    window.makeKeyAndOrderFront(None);

    let ready = wait_media_status(
        &view,
        Instant::now() + READY_TIMEOUT,
        |status| status.ready_state >= 2 && status.video_width > 0 && status.video_height > 0,
        "YouTube video did not become ready",
    )?;
    write_media_status(&evidence.join("youtube-ready-v1.json"), "ready", &ready)?;
    println!("WEBKIT_YOUTUBE ready-for-input");

    let playing = wait_media_status(
        &view,
        Instant::now() + STOP_TIMEOUT,
        |status| !status.is_paused && !status.is_muted && status.volume > 0.0,
        "YouTube did not start unmuted playback",
    )?;
    capture::pump(5.0);
    let sustained = wait_media_status(
        &view,
        Instant::now() + Duration::from_secs(2),
        |status| {
            !status.is_paused
                && !status.is_muted
                && status.volume > 0.0
                && status.current_time >= playing.current_time + 4.0
        },
        "YouTube playback did not advance for five seconds",
    )?;
    write_media_status(
        &evidence.join("youtube-playback-v1.json"),
        "sustained-playback",
        &sustained,
    )?;
    println!("WEBKIT_YOUTUBE playback");

    wait_marker(
        &evidence.join("seek-input"),
        "YouTube seek input did not arrive",
    )?;
    let seek_script = format!(
        "(() => {{ document.querySelector('video').currentTime={}; return 'seek-requested'; }})()",
        sustained.current_time + 10.0
    );
    wait_script_value(
        &view,
        &seek_script,
        "seek-requested",
        "YouTube scripted seek request failed",
    )?;
    let sought = wait_media_status(
        &view,
        Instant::now() + Duration::from_secs(2),
        |status| {
            status.current_time >= sustained.current_time + 8.0
                && !status.is_seeking
                && status.ready_state >= 2
        },
        "YouTube scripted seek did not finish with decoded data",
    )?;
    capture::pump(2.0);
    let sought = wait_media_status(
        &view,
        Instant::now() + Duration::from_secs(2),
        |status| {
            status.current_time >= sought.current_time + 1.5
                && !status.is_paused
                && !status.is_seeking
        },
        "YouTube playback did not advance after seeking",
    )?;
    write_media_status(
        &evidence.join("youtube-seek-v1.json"),
        "seeked-playback",
        &sought,
    )?;
    println!("WEBKIT_YOUTUBE seek");

    wait_marker(
        &evidence.join("fullscreen-input"),
        "YouTube full-screen input did not arrive",
    )?;
    let fullscreen = wait_media_status(
        &view,
        Instant::now() + Duration::from_secs(5),
        |status| status.is_fullscreen,
        "YouTube did not enter full screen",
    )?;
    let native_deadline = Instant::now() + Duration::from_secs(5);
    let native_fullscreen_state = loop {
        let state = unsafe { view.fullscreenState() };
        if state == WKFullscreenState::InFullscreen {
            break state;
        }
        if is_deadline_expired(native_deadline) {
            return Err(format!(
                "WebKit native full-screen state is wrong: {}",
                state.0
            ));
        }
        capture::pump(0.05);
    };
    write_media_status(
        &evidence.join("youtube-fullscreen-entered-v1.json"),
        "fullscreen-entered",
        &fullscreen,
    )?;
    println!("WEBKIT_YOUTUBE fullscreen-entered");
    wait_marker(
        &evidence.join("fullscreen-play-input"),
        "YouTube full-screen playback input did not arrive",
    )?;
    wait_script_value(
        &view,
        "(() => { void document.querySelector('video').play(); return 'resume-requested'; })()",
        "resume-requested",
        "YouTube full-screen playback could not be requested",
    )?;
    let fullscreen = wait_media_status(
        &view,
        Instant::now() + Duration::from_secs(5),
        |status| {
            status.is_fullscreen && !status.is_paused && !status.is_muted && status.volume > 0.0
        },
        "YouTube did not resume unmuted playback in full screen",
    )?;
    write_media_status(
        &evidence.join("youtube-fullscreen-v1.json"),
        "fullscreen-playback",
        &fullscreen,
    )?;
    let native_document = serde_json::json!({
        "schema_version": 1,
        "candidate": "webkit",
        "state": "fullscreen",
        "native_fullscreen_state": native_fullscreen_state.0,
    });
    write_atomic(
        &evidence.join("youtube-fullscreen-native-v1.json"),
        &serde_json::to_string_pretty(&native_document).map_err(|error| error.to_string())?,
    )?;
    capture::snapshot_guarded(
        &view,
        &evidence.join("youtube-fullscreen-page.tiff"),
        &|| Ok(()),
    )?;
    println!("WEBKIT_YOUTUBE fullscreen");

    wait_marker(&evidence.join("stop"), "YouTube stop input did not arrive")?;
    wait_script_value(
        &view,
        "(() => { void document.exitFullscreen(); return 'exit-requested'; })()",
        "exit-requested",
        "YouTube full-screen exit could not be requested",
    )?;
    let exit_deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let state = unsafe { view.fullscreenState() };
        if state == WKFullscreenState::NotInFullscreen {
            break;
        }
        if is_deadline_expired(exit_deadline) {
            return Err(format!("WebKit did not exit full screen: {}", state.0));
        }
        capture::pump(0.05);
    }
    window.setContentView(None);
    window.close();
    let weak = Weak::from_retained(&view);
    drop(view);
    let deadline = Instant::now() + Duration::from_secs(3);
    while weak.load().is_some() && !is_deadline_expired(deadline) {
        capture::pump(0.05);
    }
    let document = serde_json::json!({
        "schema_version": 1,
        "candidate": "webkit",
        "state": "release-observation-before-process-exit",
        "host_process_id": std::process::id(),
        "is_owned_reference_released": true,
        "is_native_release_observed": weak.load().is_none(),
    });
    write_atomic(
        &evidence.join("youtube-shutdown-v1.json"),
        &serde_json::to_string_pretty(&document).map_err(|error| error.to_string())?,
    )?;
    println!("WEBKIT_YOUTUBE shutdown");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_bounded_cookie_arguments() {
        assert!(
            CookieArguments::new(
                "http://localhost:1234".into(),
                "evidence".into(),
                "write",
                "0123456789abcdef",
            )
            .is_ok()
        );
        assert!(
            CookieArguments::new(
                "http://localhost:1234".into(),
                "evidence".into(),
                "read",
                "0123456789abcdef",
            )
            .is_ok()
        );
        assert!(
            CookieArguments::new(
                "http://localhost:1234".into(),
                "evidence".into(),
                "other",
                "0123456789abcdef",
            )
            .is_err()
        );
        assert!(
            CookieArguments::new(
                "http://localhost:1234".into(),
                "evidence".into(),
                "write",
                "short",
            )
            .is_err()
        );
    }

    #[test]
    fn parses_typed_youtube_media_status() {
        let status: MediaStatus = serde_json::from_str(
            r#"{"currentTime":10.5,"duration":60.0,"isFullscreen":false,"isMuted":false,"isPaused":false,"isSeeking":false,"readyState":4,"videoHeight":720,"videoWidth":1280,"volume":1.0}"#,
        )
        .unwrap();
        assert_eq!(status.current_time, 10.5);
        assert!(status.video_width > 0);
        assert!(media_status_script().contains("document.querySelector('video')"));
    }

    #[test]
    fn media_cycle_receipt_has_twenty_metadata_records_and_one_live_slot() {
        let document = media_cycle_document(2, "ready", None);
        assert_eq!(document["classification"], MEDIA_CYCLE_CLASSIFICATION);
        assert_eq!(document["tab_record_count"], MEDIA_CYCLE_TAB_RECORDS);
        assert_eq!(document["recent_slot_count"], MEDIA_CYCLE_SLOTS);
        assert_eq!(document["profile_mode"], MEDIA_CYCLE_PROFILE_MODE);
        assert_eq!(
            document["build_source_manifest_sha256"],
            BUILD_SOURCE_MANIFEST_SHA256
        );
        assert_eq!(document["tab_records"].as_array().unwrap().len(), 20);
        assert_eq!(document["recent_slots"].as_array().unwrap().len(), 4);
        assert_eq!(
            document["tab_records"]
                .as_array()
                .unwrap()
                .iter()
                .filter(|record| record["state"] == "live")
                .count(),
            1
        );
        assert_eq!(document["recent_slots"][2]["state"], "live");
        assert_eq!(
            document["recent_slots"][0]["snapshot_path"],
            "media-cycle-slot-0-window.png"
        );
    }

    #[test]
    fn media_cycle_release_receipt_preserves_metadata_and_never_claims_native_release() {
        let document = media_cycle_document(1, "release-observation-before-next-slot", None);
        assert_eq!(document["live_slot_count"], 0);
        assert_eq!(document["recent_slots"][1]["state"], "snapshot-retained");
        assert_eq!(
            document["recent_slots"][1]["snapshot_path"],
            "media-cycle-slot-1-window.png"
        );
    }

    #[test]
    fn media_cycle_stops_when_native_release_is_not_observed() {
        require_native_release(true, 0).unwrap();
        assert!(require_native_release(false, 0).is_err());
    }

    #[test]
    fn media_cycle_uses_a_fresh_view_for_the_fixed_url_without_location_replacement() {
        let source = include_str!("memory.rs");
        assert!(source.contains("fn run_media_cycle_slot"));
        assert!(source.contains("objc2::rc::autoreleasepool"));
        assert!(source.contains("window.setContentView(None);"));
        assert!(source.contains("let weak = Weak::from_retained(&view);"));
        assert!(!source.contains(&["location", "replace"].join(".")));
        assert!(!source.contains(&["web", "ready", "slot"].join("_")));
    }

    #[test]
    fn accepts_exact_memory_combinations() {
        for values in [
            (7, "loaded", "single"),
            (12, "loaded", "single"),
            (20, "loaded", "single"),
            (20, "unloaded", "single"),
            (7, "loaded", "split"),
        ] {
            let args = vec![
                "http://localhost:1".into(),
                "/tmp/evidence".into(),
                format!("{}-{}-{}", values.1, values.2, values.0),
                values.0.to_string(),
                values.1.into(),
                values.2.into(),
            ];
            assert!(MemoryArguments::parse(&args).is_ok());
        }
    }
    #[test]
    fn rejects_other_memory_combinations() {
        let invalid_shape = vec![
            "x".into(),
            "x".into(),
            "unloaded-split-7".into(),
            "7".into(),
            "unloaded".into(),
            "split".into(),
        ];
        assert!(MemoryArguments::parse(&invalid_shape).is_err());
        let wrong_id = vec![
            "x".into(),
            "x".into(),
            "case".into(),
            "7".into(),
            "loaded".into(),
            "single".into(),
        ];
        assert!(MemoryArguments::parse(&wrong_id).is_err());
    }
    #[test]
    fn assigns_repeating_visible_and_distinct_headless_routes() {
        let routes = ["article.html", "images.html", "application.html"];
        assert_eq!(
            routes.iter().cycle().take(7).collect::<Vec<_>>(),
            vec![
                &"article.html",
                &"images.html",
                &"application.html",
                &"article.html",
                &"images.html",
                &"application.html",
                &"article.html"
            ]
        );
        assert_eq!(
            ["article.html", "application.html"],
            ["article.html", "application.html"]
        );
    }
    #[test]
    fn scripts_require_typed_readiness_and_actions() {
        let status = status_script("images");
        assert!(status.contains("value.ready === true"));
        assert!(status.contains("value.kind === \"images\""));
        assert!(status.contains("value.actionCount === 0"));
        assert!(status.contains("value.details.images === 3"));
        let action = action_script("application");
        assert!(action.contains("value.actionCount === 1"));
        assert!(action.contains("value.details.rows === 2000"));
    }
    #[test]
    fn unloaded_records_and_deadlines_fail_closed() {
        let arguments = MemoryArguments {
            base: "http://localhost:1".into(),
            evidence: PathBuf::from("e"),
            scenario_id: "s".into(),
            tab_count: 20,
            background_state: "unloaded".into(),
            layout: "single".into(),
        };
        assert!(!is_unloaded_background(&arguments, 0));
        assert!(is_unloaded_background(&arguments, 1));
        assert!(is_deadline_expired(Instant::now() - Duration::from_secs(1)));
    }
    #[test]
    fn does_not_overwrite_atomic_evidence() {
        let directory = std::env::temp_dir().join(format!("webkit-memory-{}", std::process::id()));
        let _ = fs::remove_dir_all(&directory);
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("memory-ready-v1.json");
        write_atomic(&path, "one").unwrap();
        assert!(write_atomic(&path, "two").is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "one");
        fs::remove_dir_all(directory).unwrap();
    }
}
