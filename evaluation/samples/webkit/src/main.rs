#![deny(unsafe_op_in_unsafe_fn)]
mod capture;
mod memory;
mod policy;
mod smoke;

use std::cell::{Cell, OnceCell, RefCell};
use std::env;
use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::process::ExitCode;
use std::rc::Rc;

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, ProtocolObject};
use objc2::{DefinedClass, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::{
    NSApplication, NSApplicationActivationPolicy, NSApplicationDelegate, NSBackingStoreType,
    NSBezierPath, NSButton, NSColor, NSEvent, NSEventModifierFlags, NSEventType, NSTextField,
    NSView, NSWindow, NSWindowDelegate, NSWindowStyleMask,
};
use objc2_foundation::{
    MainThreadMarker, NSError, NSNotification, NSObject, NSObjectProtocol, NSPoint, NSRect, NSSize,
    NSString, NSTimer, NSURL, NSURLRequest, ns_string,
};
use objc2_web_kit::{
    WKNavigation, WKNavigationAction, WKNavigationActionPolicy, WKNavigationDelegate, WKWebView,
    WKWebViewConfiguration, WKWebsiteDataStore,
};
use policy::{BrowserPolicy, SessionId, TabId};

const PRIMARY: TabId = TabId(1);
const SECONDARY: TabId = TabId(2);
const SESSION: SessionId = SessionId(1);
const HIDDEN_FIRST_SESSION: SessionId = SessionId(2);
const HIDDEN_SECOND_SESSION: SessionId = SessionId(3);
const WIDTH: f64 = 1100.0;
const CONTENT_HEIGHT: f64 = 600.0;

struct Arguments {
    base: String,
    evidence: Option<PathBuf>,
    is_smoke: bool,
    memory: Option<memory::MemoryArguments>,
    youtube_evidence: Option<PathBuf>,
    cookie: Option<memory::CookieArguments>,
    media_memory_evidence: Option<PathBuf>,
}

impl Arguments {
    fn parse(arguments: &[String]) -> Result<Self, String> {
        match arguments {
            [base] => Ok(Self { base: validate_fixture_base(base)?, evidence: None, is_smoke: false, memory: None, youtube_evidence: None, cookie: None, media_memory_evidence: None }),
            [flag, base, evidence] if matches!(flag.as_str(), "--smoke" | "--manual") => Ok(Self {
                base: validate_fixture_base(base)?, evidence: Some(evidence.into()), is_smoke: flag == "--smoke", memory: None, youtube_evidence: None, cookie: None, media_memory_evidence: None,
            }),
            [flag, values @ ..] if flag == "--memory-smoke" => {
                let mut memory = memory::MemoryArguments::parse(values)?;
                memory.base = validate_fixture_base(&memory.base)?;
                Ok(Self { base: memory.base.clone(), evidence: None, is_smoke: false, memory: Some(memory), youtube_evidence: None, cookie: None, media_memory_evidence: None })
            }
            [flag, evidence] if flag == "--youtube-smoke" => Ok(Self {
                base: String::new(),
                evidence: None,
                is_smoke: false,
                memory: None,
                youtube_evidence: Some(evidence.into()),
                cookie: None,
                media_memory_evidence: None,
            }),
            [flag, evidence] if flag == "--media-memory-smoke" => Ok(Self {
                base: String::new(),
                evidence: None,
                is_smoke: false,
                memory: None,
                youtube_evidence: None,
                cookie: None,
                media_memory_evidence: Some(evidence.into()),
            }),
            [flag, base, evidence, stage, token] if flag == "--cookie-smoke" => {
                let cookie = memory::CookieArguments::new(
                    validate_fixture_base(base)?,
                    evidence.into(),
                    stage,
                    token,
                )?;
                Ok(Self {
                    base: cookie.base.clone(),
                    evidence: None,
                    is_smoke: false,
                    memory: None,
                    youtube_evidence: None,
                    cookie: Some(cookie),
                    media_memory_evidence: None,
                })
            }
            _ => Err("usage: webkit-smoke <fixture-origin> | --smoke|--manual <fixture-origin> <evidence-directory> | --memory-smoke <fixture-origin> <evidence-directory> <scenario-id> <tab-count> <background-state> <layout> | --youtube-smoke <evidence-directory> | --media-memory-smoke <evidence-directory> | --cookie-smoke <fixture-origin> <evidence-directory> <write|read> <token>".into()),
        }
    }
}

fn validate_fixture_base(value: &str) -> Result<String, String> {
    let authority = value
        .strip_prefix("http://")
        .ok_or("fixture origin must use http")?
        .trim_end_matches('/');
    let (host, port) = authority
        .rsplit_once(':')
        .ok_or("fixture origin requires an explicit port")?;
    let host_address = match (
        host.strip_prefix('['),
        host.strip_suffix(']'),
        host.contains(':'),
    ) {
        (Some(host), Some(_), _) => host
            .strip_suffix(']')
            .ok_or("IPv6 fixture hosts must use enclosing brackets")?,
        (None, None, false) => host,
        _ => return Err("IPv6 fixture hosts must use enclosing brackets".into()),
    };
    let is_loopback = host.eq_ignore_ascii_case("localhost")
        || host_address
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback());
    if !is_loopback || !port.parse::<u16>().is_ok_and(|port| port != 0) {
        return Err("fixture origin must name a loopback host and a valid port".into());
    }
    Ok(format!("http://{}:{port}", host.to_ascii_lowercase()))
}

fn is_fixture_address(base: &str, address: &str) -> bool {
    address == base
        || address
            .strip_prefix(base)
            .is_some_and(|suffix| suffix.starts_with('/'))
}

struct CaptureOperation<'a> {
    is_capture_in_progress: &'a Cell<bool>,
}

impl Drop for CaptureOperation<'_> {
    fn drop(&mut self) {
        self.is_capture_in_progress.set(false);
    }
}

// SAFETY: NSView supports subclassing, and OpaqueControls adds no ivars or Drop behavior.
define_class!(
    #[unsafe(super = NSView)]
    #[thread_kind = MainThreadOnly]
    struct OpaqueControls;
    impl OpaqueControls {
        #[unsafe(method(drawRect:))]
        fn draw_rect(&self, _: NSRect) {
            NSColor::windowBackgroundColor().setFill();
            NSBezierPath::fillRect(self.bounds());
        }

        #[unsafe(method(isOpaque))]
        fn is_opaque(&self) -> bool {
            true
        }
    }
);

impl OpaqueControls {
    fn with_frame(mtm: MainThreadMarker, frame: NSRect) -> Retained<NSView> {
        let controls = Self::alloc(mtm);
        // SAFETY: NSView's designated initializer returns the allocated OpaqueControls instance.
        let controls: Retained<Self> = unsafe { msg_send![controls, initWithFrame: frame] };
        controls.into_super()
    }
}

struct Ivars {
    base: String,
    evidence: Option<PathBuf>,
    window: OnceCell<Retained<NSWindow>>,
    address: OnceCell<Retained<NSTextField>>,
    controls: OnceCell<Retained<NSView>>,
    policy: RefCell<BrowserPolicy>,
    hidden_first_policy: RefCell<BrowserPolicy>,
    hidden_second_policy: RefCell<BrowserPolicy>,
    selected: Cell<TabId>,
    is_split: Cell<bool>,
    is_capture_in_progress: Cell<bool>,
    capture_sequence: Cell<u64>,
    primary_address: RefCell<String>,
    secondary_address: RefCell<String>,
    navigation_errors: Cell<u32>,
    denied_navigations: Cell<u32>,
    visible_store: OnceCell<Retained<WKWebsiteDataStore>>,
    hidden_first_store: OnceCell<Retained<WKWebsiteDataStore>>,
    hidden_second_store: OnceCell<Retained<WKWebsiteDataStore>>,
    primary: RefCell<Option<Retained<WKWebView>>>,
    secondary: RefCell<Option<Retained<WKWebView>>>,
    hidden_first: RefCell<Option<Retained<WKWebView>>>,
    hidden_second: RefCell<Option<Retained<WKWebView>>>,
    hidden_windows: RefCell<Vec<Retained<NSWindow>>>,
}

define_class!(
    #[unsafe(super = NSObject)]
    #[thread_kind = MainThreadOnly]
    #[ivars = Ivars]
    struct Delegate;
    unsafe impl NSObjectProtocol for Delegate {}
    unsafe impl NSApplicationDelegate for Delegate {
        #[unsafe(method(applicationDidFinishLaunching:))]
        fn did_finish_launching(&self, _: &NSNotification) {
            if self.ivars().window.get().is_none() { self.launch(); }
        }
    }
    unsafe impl NSWindowDelegate for Delegate {
        #[unsafe(method(windowWillClose:))]
        fn window_will_close(&self, _: &NSNotification) {
            self.drop_views();
            NSApplication::sharedApplication(self.mtm()).stop(None);
        }
    }
    unsafe impl WKNavigationDelegate for Delegate {
        #[unsafe(method(webView:decidePolicyForNavigationAction:decisionHandler:))]
        fn decide_navigation(&self, _: &WKWebView, action: &WKNavigationAction, decision: &block2::DynBlock<dyn Fn(WKNavigationActionPolicy)>) {
            let request = unsafe { action.request() };
            let is_allowed = request.URL().and_then(|url| url.absoluteString()).is_some_and(|url| is_fixture_address(&self.ivars().base, &url.to_string()));
            if !is_allowed { self.ivars().denied_navigations.set(self.ivars().denied_navigations.get() + 1); }
            decision.call((if is_allowed { WKNavigationActionPolicy::Allow } else { WKNavigationActionPolicy::Cancel },));
        }
        #[unsafe(method(webView:didStartProvisionalNavigation:))]
        fn navigation_started(&self, view: &WKWebView, _: Option<&WKNavigation>) {
            self.navigation_started_for(view);
        }
        #[unsafe(method(webView:didFinishNavigation:))]
        fn navigation_finished(&self, view: &WKWebView, _: Option<&WKNavigation>) {
            self.navigation_finished_for(view);
            self.set_title("ready");
        }
        #[unsafe(method(webView:didFailProvisionalNavigation:withError:))]
        fn navigation_failed(&self, _: &WKWebView, _: Option<&WKNavigation>, _: &NSError) {
            self.ivars().navigation_errors.set(self.ivars().navigation_errors.get() + 1);
            self.set_title("navigation failed");
        }
    }
    impl Delegate {
        #[unsafe(method(selectFirst:))]
        fn select_first(&self, _: &AnyObject) { self.select(PRIMARY); }
        #[unsafe(method(selectSecond:))]
        fn select_second(&self, _: &AnyObject) { self.select(SECONDARY); }
        #[unsafe(method(split:))]
        fn split(&self, _: &AnyObject) { self.ivars().is_split.set(!self.ivars().is_split.get()); self.layout(); }
        #[unsafe(method(navigate:))]
        fn navigate(&self, _: &AnyObject) { self.load_selected(); }
        #[unsafe(method(unload:))]
        fn unload(&self, _: &AnyObject) {
            objc2::rc::autoreleasepool(|_| {
            let tab = self.ivars().selected.get();
            let view = self.slot(tab).borrow_mut().take();
            if let Some(view) = view {
                unsafe { view.setNavigationDelegate(None); view.stopLoading(); }
                view.removeFromSuperviewWithoutNeedingDisplay();
            }
            let _ = self.ivars().policy.borrow_mut().navigation_started(tab);
            self.set_title("selected tab unloaded");
            });
        }
        #[unsafe(method(reload:))]
        fn reload(&self, _: &AnyObject) {
            let tab = self.ivars().selected.get();
            let address = self.committed_address(tab);
            if self.slot(tab).borrow().is_none() {
                let view = self.make_view(self.ivars().visible_store.get().expect("visible store"));
                *self.slot(tab).borrow_mut() = Some(view.clone());
                self.load_view(&view, &address);
                self.layout();
            } else {
                self.load_selected_address(&address);
            }
        }
        #[unsafe(method(grant:))]
        fn grant(&self, _: &AnyObject) {
            self.set_title(if self.ivars().policy.borrow_mut().grant_selected().is_ok() { "control granted" } else { "grant denied" });
        }
        #[unsafe(method(revoke:))]
        fn revoke(&self, _: &AnyObject) { self.ivars().policy.borrow_mut().revoke(); self.set_title("control revoked"); }
        #[unsafe(method(snapshot:))]
        fn snapshot(&self, _: &AnyObject) {
            let result = self.capture_selected();
            self.set_title(result.as_ref().map_or_else(|error| error.as_str(), |_| "snapshot saved"));
        }
        #[unsafe(method(record:))]
        fn record(&self, _: &AnyObject) {
            let result = self.record_selected();
            self.set_title(result.as_ref().map_or_else(|error| error.as_str(), |_| "recording saved"));
        }
    }
);

impl Delegate {
    fn new(mtm: MainThreadMarker, arguments: Arguments) -> Retained<Self> {
        let primary_address = arguments.base.clone() + "/smoke.html?tab=1";
        let secondary_address = arguments.base.clone() + "/smoke.html?tab=2";
        let this = Self::alloc(mtm).set_ivars(Ivars {
            base: arguments.base,
            evidence: arguments.evidence,
            window: OnceCell::new(),
            address: OnceCell::new(),
            controls: OnceCell::new(),
            policy: RefCell::new(BrowserPolicy::new(SESSION, [PRIMARY, SECONDARY])),
            hidden_first_policy: RefCell::new(BrowserPolicy::new(HIDDEN_FIRST_SESSION, [PRIMARY])),
            hidden_second_policy: RefCell::new(BrowserPolicy::new(
                HIDDEN_SECOND_SESSION,
                [SECONDARY],
            )),
            selected: Cell::new(PRIMARY),
            is_split: Cell::new(false),
            is_capture_in_progress: Cell::new(false),
            capture_sequence: Cell::new(0),
            primary_address: RefCell::new(primary_address),
            secondary_address: RefCell::new(secondary_address),
            navigation_errors: Cell::new(0),
            denied_navigations: Cell::new(0),
            visible_store: OnceCell::new(),
            hidden_first_store: OnceCell::new(),
            hidden_second_store: OnceCell::new(),
            primary: RefCell::new(None),
            secondary: RefCell::new(None),
            hidden_first: RefCell::new(None),
            hidden_second: RefCell::new(None),
            hidden_windows: RefCell::new(Vec::new()),
        });
        unsafe { msg_send![super(this), init] }
    }

    fn launch(&self) {
        let mtm = self.mtm();
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(WIDTH, CONTENT_HEIGHT + 80.0),
                ),
                NSWindowStyleMask::Titled | NSWindowStyleMask::Closable,
                NSBackingStoreType::Buffered,
                false,
            )
        };
        unsafe { window.setReleasedWhenClosed(false) };
        for store in [
            &self.ivars().visible_store,
            &self.ivars().hidden_first_store,
            &self.ivars().hidden_second_store,
        ] {
            store
                .set(unsafe { WKWebsiteDataStore::nonPersistentDataStore(mtm) })
                .expect("fresh store");
        }
        let content = window.contentView().expect("content view");
        let controls = OpaqueControls::with_frame(
            mtm,
            NSRect::new(NSPoint::new(0.0, CONTENT_HEIGHT), NSSize::new(WIDTH, 80.0)),
        );
        content.addSubview(&controls);
        let address = NSTextField::initWithFrame(
            NSTextField::alloc(mtm),
            NSRect::new(NSPoint::new(8.0, 44.0), NSSize::new(WIDTH - 16.0, 24.0)),
        );
        address.setStringValue(&NSString::from_str(&self.url("/smoke.html")));
        controls.addSubview(&address);
        self.ivars().address.set(address).expect("address field");
        self.ivars()
            .controls
            .set(controls.clone())
            .expect("controls view");
        for (index, (title, action)) in [
            ("Tab 1", sel!(selectFirst:)),
            ("Tab 2", sel!(selectSecond:)),
            ("Split", sel!(split:)),
            ("Go", sel!(navigate:)),
            ("Unload", sel!(unload:)),
            ("Reload", sel!(reload:)),
            ("Grant", sel!(grant:)),
            ("Revoke", sel!(revoke:)),
            ("Snapshot", sel!(snapshot:)),
            ("Record", sel!(record:)),
        ]
        .into_iter()
        .enumerate()
        {
            let button = unsafe {
                NSButton::buttonWithTitle_target_action(
                    &NSString::from_str(title),
                    Some(self),
                    Some(action),
                    mtm,
                )
            };
            button.setBordered(false);
            button.setContentTintColor(Some(&NSColor::labelColor()));
            button.setFrame(NSRect::new(
                NSPoint::new(8.0 + 106.0 * index as f64, 8.0),
                NSSize::new(102.0, 26.0),
            ));
            controls.addSubview(&button);
        }
        self.ivars().window.set(window).expect("window");
        for (slot, store, address) in [
            (
                &self.ivars().primary,
                &self.ivars().visible_store,
                self.committed_address(PRIMARY),
            ),
            (
                &self.ivars().secondary,
                &self.ivars().visible_store,
                self.committed_address(SECONDARY),
            ),
            (
                &self.ivars().hidden_first,
                &self.ivars().hidden_first_store,
                self.url("/smoke.html?hidden=1"),
            ),
            (
                &self.ivars().hidden_second,
                &self.ivars().hidden_second_store,
                self.url("/smoke.html?hidden=2"),
            ),
        ] {
            let view = self.make_view(store.get().expect("store"));
            *slot.borrow_mut() = Some(view.clone());
            self.load_view(&view, &address);
        }
        for slot in [&self.ivars().hidden_first, &self.ivars().hidden_second] {
            let view = slot.borrow().clone().expect("hidden view");
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(
                    NSWindow::alloc(mtm),
                    NSRect::new(
                        NSPoint::new(0.0, 0.0),
                        NSSize::new(WIDTH / 2.0, CONTENT_HEIGHT),
                    ),
                    NSWindowStyleMask::Borderless,
                    NSBackingStoreType::Buffered,
                    false,
                )
            };
            unsafe { window.setReleasedWhenClosed(false) };
            window.setAlphaValue(0.0);
            window.setIgnoresMouseEvents(true);
            window.setContentView(Some(&view));
            window.orderFront(None);
            self.ivars().hidden_windows.borrow_mut().push(window);
        }
        self.select(PRIMARY);
        let window = self.ivars().window.get().expect("window");
        window.setDelegate(Some(ProtocolObject::from_ref(self)));
        window.setTitle(ns_string!("WebKit smoke"));
        window.makeKeyAndOrderFront(None);
        let app = NSApplication::sharedApplication(mtm);
        let _ = app.setActivationPolicy(NSApplicationActivationPolicy::Regular);
        #[expect(deprecated, reason = "unbundled Cargo sample needs AppKit activation")]
        app.activateIgnoringOtherApps(true);
    }

    fn slot(&self, tab: TabId) -> &RefCell<Option<Retained<WKWebView>>> {
        if tab == PRIMARY {
            &self.ivars().primary
        } else {
            &self.ivars().secondary
        }
    }
    fn selected_view(&self) -> Result<Retained<WKWebView>, String> {
        self.slot(self.ivars().selected.get())
            .borrow()
            .clone()
            .ok_or("selected tab is unloaded".into())
    }
    fn select(&self, tab: TabId) {
        if self.ivars().policy.borrow_mut().select(tab).is_err() {
            return;
        }
        self.ivars().selected.set(tab);
        self.ivars()
            .address
            .get()
            .expect("address")
            .setStringValue(&NSString::from_str(&self.committed_address(tab)));
        self.layout();
        self.set_title(if tab == PRIMARY {
            "tab 1 selected"
        } else {
            "tab 2 selected"
        });
    }
    fn layout(&self) {
        objc2::rc::autoreleasepool(|_| self.layout_contents());
    }
    fn layout_contents(&self) {
        let content = self.content();
        for (tab, x) in [(PRIMARY, 0.0), (SECONDARY, WIDTH / 2.0)] {
            if let Some(view) = self.slot(tab).borrow().as_ref() {
                view.removeFromSuperview();
                if self.ivars().is_split.get() || self.ivars().selected.get() == tab {
                    let (x, width) = if self.ivars().is_split.get() {
                        (x, WIDTH / 2.0)
                    } else {
                        (0.0, WIDTH)
                    };
                    view.setFrame(NSRect::new(
                        NSPoint::new(x, 0.0),
                        NSSize::new(width, CONTENT_HEIGHT),
                    ));
                    content.addSubview(view);
                }
            }
        }
    }
    fn content(&self) -> Retained<NSView> {
        self.ivars()
            .window
            .get()
            .expect("window")
            .contentView()
            .expect("content")
    }
    fn controls(&self) -> Retained<NSView> {
        self.ivars().controls.get().expect("controls").clone()
    }
    fn url(&self, route: &str) -> String {
        format!("{}{}", self.ivars().base, route)
    }
    fn committed_address(&self, tab: TabId) -> String {
        if tab == PRIMARY {
            self.ivars().primary_address.borrow().clone()
        } else {
            self.ivars().secondary_address.borrow().clone()
        }
    }
    fn set_committed_address(&self, tab: TabId, address: String) {
        if tab == PRIMARY {
            *self.ivars().primary_address.borrow_mut() = address;
        } else {
            *self.ivars().secondary_address.borrow_mut() = address;
        }
    }
    fn navigation_started_for(&self, view: &WKWebView) {
        for (tab, slot) in [
            (PRIMARY, &self.ivars().primary),
            (SECONDARY, &self.ivars().secondary),
        ] {
            if slot
                .borrow()
                .as_ref()
                .is_some_and(|stored| std::ptr::eq(&**stored, view))
            {
                let _ = self.ivars().policy.borrow_mut().navigation_started(tab);
                return;
            }
        }
        if self
            .ivars()
            .hidden_first
            .borrow()
            .as_ref()
            .is_some_and(|stored| std::ptr::eq(&**stored, view))
        {
            let _ = self
                .ivars()
                .hidden_first_policy
                .borrow_mut()
                .navigation_started(PRIMARY);
        } else if self
            .ivars()
            .hidden_second
            .borrow()
            .as_ref()
            .is_some_and(|stored| std::ptr::eq(&**stored, view))
        {
            let _ = self
                .ivars()
                .hidden_second_policy
                .borrow_mut()
                .navigation_started(SECONDARY);
        }
    }
    fn navigation_finished_for(&self, view: &WKWebView) {
        let Some(address) = unsafe { view.URL() }
            .and_then(|url| url.absoluteString())
            .map(|url| url.to_string())
        else {
            return;
        };
        for (tab, slot) in [
            (PRIMARY, &self.ivars().primary),
            (SECONDARY, &self.ivars().secondary),
        ] {
            if slot
                .borrow()
                .as_ref()
                .is_some_and(|stored| std::ptr::eq(&**stored, view))
            {
                self.set_committed_address(tab, address.clone());
                if self.ivars().selected.get() == tab {
                    self.ivars()
                        .address
                        .get()
                        .expect("address")
                        .setStringValue(&NSString::from_str(&address));
                }
                return;
            }
        }
    }
    fn set_title(&self, title: &str) {
        if let Some(window) = self.ivars().window.get() {
            window.setTitle(&NSString::from_str(title));
        }
    }
    fn drop_views(&self) {
        for slot in [
            &self.ivars().primary,
            &self.ivars().secondary,
            &self.ivars().hidden_first,
            &self.ivars().hidden_second,
        ] {
            let view = slot.borrow_mut().take();
            if let Some(view) = view {
                unsafe {
                    view.setNavigationDelegate(None);
                    view.stopLoading();
                }
                view.removeFromSuperview();
            }
        }
        for window in self.ivars().hidden_windows.borrow_mut().drain(..) {
            window.setContentView(None);
            window.close();
        }
        let mut policy = self.ivars().policy.borrow_mut();
        let _ = policy.close(PRIMARY);
        let _ = policy.close(SECONDARY);
        policy.end_session();
        self.ivars().hidden_first_policy.borrow_mut().end_session();
        self.ivars().hidden_second_policy.borrow_mut().end_session();
    }
    fn load_selected(&self) {
        let address = self
            .ivars()
            .address
            .get()
            .expect("address")
            .stringValue()
            .to_string();
        self.load_selected_address(&address);
    }
    fn load_selected_address(&self, address: &str) {
        if !is_fixture_address(&self.ivars().base, address) {
            self.set_title("non-fixture navigation denied");
            return;
        }
        let Ok(view) = self.selected_view() else {
            self.set_title("selected tab is unloaded");
            return;
        };
        let Some(url) = NSURL::URLWithString(&NSString::from_str(address)) else {
            self.set_title("invalid address");
            return;
        };
        unsafe {
            view.loadRequest(&NSURLRequest::requestWithURL(&url));
        }
    }
    fn load_view(&self, view: &WKWebView, address: &str) {
        let url = NSURL::URLWithString(&NSString::from_str(address)).expect("fixture address");
        unsafe {
            view.loadRequest(&NSURLRequest::requestWithURL(&url));
        }
    }
    fn make_view(&self, store: &WKWebsiteDataStore) -> Retained<WKWebView> {
        let mtm = self.mtm();
        let configuration = unsafe { WKWebViewConfiguration::new(mtm) };
        unsafe { configuration.setWebsiteDataStore(store) };
        let view = unsafe {
            WKWebView::initWithFrame_configuration(
                WKWebView::alloc(mtm),
                NSRect::new(
                    NSPoint::new(0.0, 0.0),
                    NSSize::new(WIDTH / 2.0, CONTENT_HEIGHT),
                ),
                &configuration,
            )
        };
        unsafe { view.setNavigationDelegate(Some(ProtocolObject::from_ref(self))) };
        view
    }
    fn authorized_target_view(&self) -> Result<(policy::PageTarget, Retained<WKWebView>), String> {
        let policy = self.ivars().policy.borrow();
        let target = policy
            .target(self.ivars().selected.get())
            .map_err(|error| format!("control denied: {error:?}"))?;
        policy
            .authorize(target)
            .map_err(|error| format!("control denied: {error:?}"))?;
        Ok((target, self.selected_view()?))
    }
    fn authorized_view(&self) -> Result<Retained<WKWebView>, String> {
        self.authorized_target_view().map(|(_, view)| view)
    }
    fn capture_operation(&self, kind: &str) -> Result<CaptureOperation<'_>, String> {
        if self.ivars().is_capture_in_progress.replace(true) {
            return Err(format!(
                "{kind} rejected: capture operation already in progress"
            ));
        }
        Ok(CaptureOperation {
            is_capture_in_progress: &self.ivars().is_capture_in_progress,
        })
    }
    fn next_capture_path(&self, name: &str, extension: &str) -> Result<PathBuf, String> {
        let directory = self.evidence()?;
        let sequence = self.ivars().capture_sequence.get() + 1;
        self.ivars().capture_sequence.set(sequence);
        Ok(directory.join(format!("{name}-{sequence:03}.{extension}")))
    }
    fn next_capture_directory(&self, name: &str) -> Result<PathBuf, String> {
        let directory = self.evidence()?;
        let sequence = self.ivars().capture_sequence.get() + 1;
        self.ivars().capture_sequence.set(sequence);
        Ok(directory.join(format!("{name}-{sequence:03}")))
    }
    fn hidden_target_view(
        &self,
        is_first: bool,
    ) -> Result<(policy::PageTarget, Retained<WKWebView>), String> {
        let (policy, slot, tab) = if is_first {
            (
                &self.ivars().hidden_first_policy,
                &self.ivars().hidden_first,
                PRIMARY,
            )
        } else {
            (
                &self.ivars().hidden_second_policy,
                &self.ivars().hidden_second,
                SECONDARY,
            )
        };
        let policy = policy.borrow();
        let target = policy
            .target(tab)
            .map_err(|error| format!("control denied: {error:?}"))?;
        policy
            .authorize(target)
            .map_err(|error| format!("control denied: {error:?}"))?;
        let view = slot.borrow().clone().ok_or("missing hidden WebView")?;
        Ok((target, view))
    }
    fn grant_hidden(&self, is_first: bool) -> Result<(), String> {
        let policy = if is_first {
            &self.ivars().hidden_first_policy
        } else {
            &self.ivars().hidden_second_policy
        };
        policy
            .borrow_mut()
            .grant_selected()
            .map_err(|error| format!("control denied: {error:?}"))
    }
    fn select_hidden(&self, is_first: bool) -> Result<(), String> {
        let (policy, tab) = if is_first {
            (&self.ivars().hidden_first_policy, PRIMARY)
        } else {
            (&self.ivars().hidden_second_policy, SECONDARY)
        };
        policy
            .borrow_mut()
            .select(tab)
            .map_err(|error| format!("control denied: {error:?}"))
    }
    fn revoke_hidden(&self, is_first: bool) {
        if is_first {
            self.ivars().hidden_first_policy.borrow_mut().revoke();
        } else {
            self.ivars().hidden_second_policy.borrow_mut().revoke();
        }
    }
    fn evidence(&self) -> Result<&Path, String> {
        self.ivars()
            .evidence
            .as_deref()
            .ok_or("supply an evidence directory with --manual or --smoke".into())
    }
    fn capture_controls(&self, path: &Path) -> Result<(), String> {
        let _operation = self.capture_operation("control snapshot")?;
        capture::snapshot_controls(&self.controls(), path)
    }
    fn capture_selected(&self) -> Result<(), String> {
        let _operation = self.capture_operation("snapshot")?;
        let (target, view) = self.authorized_target_view()?;
        let path = self.next_capture_path("snapshot", "tiff")?;
        capture::snapshot_guarded(&view, &path, &|| {
            self.ivars()
                .policy
                .borrow()
                .authorize(target)
                .map_err(|error| format!("control denied: {error:?}"))
        })
    }
    fn record_selected(&self) -> Result<(), String> {
        let _operation = self.capture_operation("record")?;
        let (target, view) = self.authorized_target_view()?;
        let directory = self.next_capture_directory("recording")?;
        capture::record_guarded(&view, &directory, &|| {
            self.ivars()
                .policy
                .borrow()
                .authorize(target)
                .map_err(|error| format!("control denied: {error:?}"))
        })
    }
}

fn main() -> ExitCode {
    let arguments = match Arguments::parse(&env::args().skip(1).collect::<Vec<_>>()) {
        Ok(arguments) => arguments,
        Err(error) => {
            eprintln!("{error}");
            return ExitCode::FAILURE;
        }
    };
    let is_smoke = arguments.is_smoke;
    let memory_arguments = arguments.memory.clone();
    let youtube_evidence = arguments.youtube_evidence.clone();
    let cookie_arguments = arguments.cookie.clone();
    let media_memory_evidence = arguments.media_memory_evidence.clone();
    let mtm = MainThreadMarker::new().expect("main thread");
    let app = NSApplication::sharedApplication(mtm);
    if let Some(media_memory_evidence) = media_memory_evidence {
        let completion = Rc::new(RefCell::new(None));
        let callback_completion = Rc::clone(&completion);
        let callback = block2::RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| {
            *callback_completion.borrow_mut() =
                Some(memory::run_media_memory(mtm, media_memory_evidence.clone()));
            NSApplication::sharedApplication(mtm).stop(None);
        });
        let timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.2, false, &callback) };
        app.run();
        drop(timer);
        return match completion
            .borrow_mut()
            .take()
            .unwrap_or_else(|| Err("media memory smoke stopped before completion".into()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }
    if let Some(cookie_arguments) = cookie_arguments {
        let completion = Rc::new(RefCell::new(None));
        let callback_completion = Rc::clone(&completion);
        let callback = block2::RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| {
            *callback_completion.borrow_mut() =
                Some(memory::run_cookie(mtm, cookie_arguments.clone()));
            NSApplication::sharedApplication(mtm).stop(None);
        });
        let timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.2, false, &callback) };
        app.run();
        drop(timer);
        return match completion
            .borrow_mut()
            .take()
            .unwrap_or_else(|| Err("cookie smoke stopped before completion".into()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }
    if let Some(youtube_evidence) = youtube_evidence {
        let completion = Rc::new(RefCell::new(None));
        let callback_completion = Rc::clone(&completion);
        let callback = block2::RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| {
            *callback_completion.borrow_mut() =
                Some(memory::run_youtube(mtm, youtube_evidence.clone()));
            NSApplication::sharedApplication(mtm).stop(None);
        });
        let timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.2, false, &callback) };
        app.run();
        drop(timer);
        return match completion
            .borrow_mut()
            .take()
            .unwrap_or_else(|| Err("YouTube smoke stopped before completion".into()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }
    if let Some(memory_arguments) = memory_arguments {
        let completion = Rc::new(RefCell::new(None));
        let callback_completion = Rc::clone(&completion);
        let callback = block2::RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| {
            *callback_completion.borrow_mut() = Some(memory::run(mtm, memory_arguments.clone()));
            NSApplication::sharedApplication(mtm).stop(None);
        });
        let timer =
            unsafe { NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.2, false, &callback) };
        app.run();
        drop(timer);
        return match completion
            .borrow_mut()
            .take()
            .unwrap_or_else(|| Err("memory smoke stopped before completion".into()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(error) => {
                eprintln!("{error}");
                ExitCode::FAILURE
            }
        };
    }
    let delegate = Delegate::new(mtm, arguments);
    app.setDelegate(Some(ProtocolObject::from_ref(&*delegate)));
    let completion = Rc::new(RefCell::new(None));
    let timer = if is_smoke {
        let callback_delegate = delegate.clone();
        let callback_completion = Rc::clone(&completion);
        let callback = block2::RcBlock::new(move |_: std::ptr::NonNull<NSTimer>| {
            if callback_delegate.ivars().window.get().is_none() {
                callback_delegate.launch();
            }
            *callback_completion.borrow_mut() = Some(smoke::run(&callback_delegate));
            let app = NSApplication::sharedApplication(callback_delegate.mtm());
            app.stop(None);
            if let Some(event) = NSEvent::otherEventWithType_location_modifierFlags_timestamp_windowNumber_context_subtype_data1_data2(NSEventType::ApplicationDefined, NSPoint::new(0.0, 0.0), NSEventModifierFlags::empty(), 0.0, 0, None, 0, 0, 0) {
                app.postEvent_atStart(&event, false);
            }
        });
        Some(unsafe {
            NSTimer::scheduledTimerWithTimeInterval_repeats_block(0.2, false, &callback)
        })
    } else {
        None
    };
    app.run();
    drop(timer);
    let result = if is_smoke {
        completion
            .borrow_mut()
            .take()
            .unwrap_or_else(|| Err("sample closed before smoke checks finished".into()))
    } else {
        Ok(())
    };
    delegate.drop_views();
    if let Some(window) = delegate.ivars().window.get() {
        window.setDelegate(None);
        window.close();
    }
    app.setDelegate(None);
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("{error}");
            ExitCode::FAILURE
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn accepts_only_exact_media_memory_arguments() {
        let arguments =
            Arguments::parse(&["--media-memory-smoke".to_owned(), "evidence".to_owned()]).unwrap();
        assert_eq!(
            arguments.media_memory_evidence,
            Some(PathBuf::from("evidence"))
        );
        assert!(Arguments::parse(&["--media-memory-smoke".to_owned()]).is_err());
    }

    #[test]
    fn accepts_only_exact_cookie_smoke_arguments() {
        let arguments = Arguments::parse(&[
            "--cookie-smoke".to_owned(),
            "http://localhost:1234".to_owned(),
            "evidence".to_owned(),
            "write".to_owned(),
            "0123456789abcdef".to_owned(),
        ])
        .unwrap();
        assert!(arguments.cookie.is_some());
        assert!(
            Arguments::parse(&[
                "--cookie-smoke".to_owned(),
                "https://example.com".to_owned(),
                "evidence".to_owned(),
                "write".to_owned(),
                "0123456789abcdef".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn accepts_only_exact_youtube_smoke_arguments() {
        let arguments =
            Arguments::parse(&["--youtube-smoke".to_owned(), "evidence".to_owned()]).unwrap();
        assert_eq!(arguments.youtube_evidence, Some(PathBuf::from("evidence")));
        assert!(Arguments::parse(&["--youtube-smoke".to_owned()]).is_err());
        assert!(
            Arguments::parse(&[
                "--youtube-smoke".to_owned(),
                "evidence".to_owned(),
                "extra".to_owned(),
            ])
            .is_err()
        );
    }

    #[test]
    fn rejects_origin_prefix_confusion() {
        let base = "http://localhost:1234";
        assert!(is_fixture_address(base, "http://localhost:1234/smoke.html"));
        assert!(!is_fixture_address(
            base,
            "http://localhost:12345/smoke.html"
        ));
        assert!(!is_fixture_address(
            base,
            "http://localhost:1234@example.org/"
        ));
        assert!(!is_fixture_address(
            base,
            "http://localhost:1234.example.org/"
        ));
    }
    #[test]
    fn validates_local_fixture_origins() {
        for (input, expected) in [
            ("http://localhost:1234/", "http://localhost:1234"),
            ("http://127.0.0.1:1234", "http://127.0.0.1:1234"),
            ("http://[::1]:1234", "http://[::1]:1234"),
        ] {
            assert_eq!(validate_fixture_base(input).unwrap(), expected);
        }
        for value in [
            "http://example.org:80",
            "file:///tmp/page",
            "http://localhost:0",
            "http://localhost:1234/path",
            "http://user@localhost:1234",
            "http://::1:1234",
            "http://::ffff:127.0.0.1:1234",
        ] {
            assert!(validate_fixture_base(value).is_err());
        }
    }
}
