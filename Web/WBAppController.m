#import "WBAppController.h"
#import <WebKit/WebKit.h>
#import "WBSessionStore.h"
#import "WBTab.h"

static const NSTimeInterval WBScrollCaptureTimeout = 1.0;
static const NSUInteger WBOpenTabLimit = 100;
static const NSUInteger WBTabOperationLimit = 64;

typedef void (^WBTabOperation)(void (^completion)(void));

@interface WBAppController () <WKNavigationDelegate, WKUIDelegate, WKDownloadDelegate, NSSearchFieldDelegate, NSMenuItemValidation>
@property (nonatomic) NSWindow *window;
@property (nonatomic) NSStackView *tabStack;
@property (nonatomic) NSView *contentView;
@property (nonatomic) NSSearchField *addressField;
@property (nonatomic) WKWebView *webView;
@property (nonatomic) NSMutableArray<WBTab *> *tabs;
@property (nonatomic) NSMutableArray<NSDictionary *> *closedTabs;
@property (nonatomic) NSUUID *selectedTabIdentifier;
@property (nonatomic) NSMutableSet<WKDownload *> *downloads;
@property (nonatomic) NSMapTable<WKDownload *, NSDictionary<NSString *, NSURL *> *> *downloadDestinations;
@property (nonatomic) WBSessionStore *sessionStore;
@property (nonatomic) NSMutableArray *tabOperations;
@property (nonatomic) WKNavigation *historyNavigation;
@property (nonatomic) BOOL isProcessingTabOperation;
@property (nonatomic) BOOL isRestoringHistory;
@property (nonatomic) BOOL isScrollRestorationPending;
@property (nonatomic) BOOL isTerminationReplyPending;
@property (nonatomic) BOOL isSessionSaveFailureReported;
@end

@implementation WBAppController

- (NSUInteger)liveViewCount {
    return self.webView ? 1 : 0;
}

- (void)applicationDidFinishLaunching:(NSNotification *)notification {
    self.tabs = [NSMutableArray array];
    self.closedTabs = [NSMutableArray array];
    self.downloads = [NSMutableSet set];
    self.downloadDestinations = [NSMapTable strongToStrongObjectsMapTable];
    self.tabOperations = [NSMutableArray array];
    NSURL *applicationSupport = [NSFileManager.defaultManager URLForDirectory:NSApplicationSupportDirectory inDomain:NSUserDomainMask appropriateForURL:nil create:YES error:nil];
    self.sessionStore = [[WBSessionStore alloc] initWithDirectoryURL:[applicationSupport URLByAppendingPathComponent:@"Web" isDirectory:YES]];
    [self restoreSession];
    [self buildWindow];
    [self refreshTabStrip];
    [self createLiveViewAndLoadSelectedTab];
    [self installMainMenu];
    [self.window makeKeyAndOrderFront:nil];
    [NSApp activate];
}

- (NSApplicationTerminateReply)applicationShouldTerminate:(NSApplication *)sender {
    if (self.isTerminationReplyPending) {
        return NSTerminateLater;
    }
    if (!self.webView || !self.selectedTab) {
        return [self saveSession] ? NSTerminateNow : NSTerminateCancel;
    }
    self.isTerminationReplyPending = YES;
    __weak typeof(self) weakSelf = self;
    [self captureCurrentScrollWithCompletion:^{
        [weakSelf completeTerminationAfterScrollCapture];
    }];
    [self performSelector:@selector(completeTerminationAfterScrollCapture) withObject:nil afterDelay:WBScrollCaptureTimeout];
    return NSTerminateLater;
}

- (void)completeTerminationAfterScrollCapture {
    if (!self.isTerminationReplyPending) {
        return;
    }
    self.isTerminationReplyPending = NO;
    [NSObject cancelPreviousPerformRequestsWithTarget:self selector:@selector(completeTerminationAfterScrollCapture) object:nil];
    [self cancelHistoryRestoration];
    [NSApp replyToApplicationShouldTerminate:[self saveSession]];
}

- (BOOL)applicationShouldTerminateAfterLastWindowClosed:(NSApplication *)sender {
    return YES;
}

- (void)restoreSession {
    NSDictionary *session = [self.sessionStore loadSessionWithError:nil];
    if (!session) {
        NSURL *URL = [NSURL URLWithString:@"https://www.google.com/"];
        [self.tabs addObject:[[WBTab alloc] initWithURL:URL]];
        self.selectedTabIdentifier = self.tabs.firstObject.identifier;
        return;
    }
    for (NSDictionary *dictionary in session[@"tabs"]) {
        WBTab *tab = [[WBTab alloc] initWithDictionary:dictionary];
        if (tab) {
            [self.tabs addObject:tab];
        }
    }
    [self.closedTabs addObjectsFromArray:session[@"closedTabs"]];
    self.selectedTabIdentifier = [[NSUUID alloc] initWithUUIDString:session[@"selectedIdentifier"]];
}

- (void)buildWindow {
    NSRect frame = NSMakeRect(0, 0, 1180, 760);
    self.window = [[NSWindow alloc] initWithContentRect:frame styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable | NSWindowStyleMaskMiniaturizable | NSWindowStyleMaskResizable) backing:NSBackingStoreBuffered defer:NO];
    self.window.title = @"Web";
    self.window.minSize = NSMakeSize(640, 420);
    [self.window center];
    NSStackView *root = [[NSStackView alloc] initWithFrame:self.window.contentView.bounds];
    root.orientation =NSUserInterfaceLayoutOrientationVertical;
    root.alignment = NSLayoutAttributeLeading;
    root.distribution = NSStackViewDistributionFill;
    root.spacing = 0;
    root.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    [self.window.contentView addSubview:root];

    NSStackView *toolbarStack = [NSStackView stackViewWithViews:@[]];
    toolbarStack.orientation = NSUserInterfaceLayoutOrientationHorizontal;
    toolbarStack.spacing = 6;
    toolbarStack.edgeInsets = NSEdgeInsetsMake(6, 8, 6, 8);
    [toolbarStack.heightAnchor constraintEqualToConstant:38].active = YES;
    [toolbarStack addArrangedSubview:[self buttonWithTitle:@"‹" action:@selector(goBack:)]];
    [toolbarStack addArrangedSubview:[self buttonWithTitle:@"›" action:@selector(goForward:)]];
    [toolbarStack addArrangedSubview:[self buttonWithTitle:@"↻" action:@selector(reloadSelected:)]];
    self.addressField = [[NSSearchField alloc] initWithFrame:NSMakeRect(0, 0, 600, 28)];
    self.addressField.delegate = self;
    self.addressField.target = self;
    self.addressField.action = @selector(navigateAddress:);
    self.addressField.sendsWholeSearchString = YES;
    self.addressField.placeholderString = @"Address or Google search";
    [self.addressField.widthAnchor constraintGreaterThanOrEqualToConstant:260].active = YES;
    [toolbarStack addArrangedSubview:self.addressField];
    [toolbarStack addArrangedSubview:[self buttonWithTitle:@"+" action:@selector(newTab:)]];
    [toolbarStack addArrangedSubview:[self buttonWithTitle:@"×" action:@selector(closeSelectedTab:)]];

    self.tabStack = [NSStackView stackViewWithViews:@[]];
    self.tabStack.orientation =NSUserInterfaceLayoutOrientationHorizontal;
    self.tabStack.spacing = 2;
    self.tabStack.edgeInsets = NSEdgeInsetsMake(2, 8, 3, 8);
    [self.tabStack.heightAnchor constraintEqualToConstant:31].active = YES;

    self.contentView = [[NSView alloc] initWithFrame:NSZeroRect];
    self.contentView.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    [root addArrangedSubview:toolbarStack];
    [root addArrangedSubview:self.tabStack];
    [root addArrangedSubview:self.contentView];
}

- (NSButton *)buttonWithTitle:(NSString *)title action:(SEL)action {
    NSButton *button = [NSButton buttonWithTitle:title target:self action:action];
    button.bezelStyle = NSBezelStyleTexturedRounded;
    button.font = [NSFont systemFontOfSize:15 weight:NSFontWeightMedium];
    return button;
}

- (void)installMainMenu {
    NSMenu *menu = [[NSMenu alloc] initWithTitle:@"Web"];
    NSMenuItem *applicationItem = [[NSMenuItem alloc] initWithTitle:@"Web" action:nil keyEquivalent:@""];
    NSMenu *applicationMenu = [[NSMenu alloc] initWithTitle:@"Web"];
    [applicationMenu addItemWithTitle:@"Quit Web" action:@selector(terminate:) keyEquivalent:@"q"];
    applicationItem.submenu = applicationMenu;
    [menu addItem:applicationItem];
    NSMenuItem *browserItem = [[NSMenuItem alloc] initWithTitle:@"Browser" action:nil keyEquivalent:@""];
    NSMenu *browserMenu = [[NSMenu alloc] initWithTitle:@"Browser"];
    [self addMenuItemToMenu:browserMenu title:@"Focus Address" action:@selector(focusAddress:) keyEquivalent:@"l" modifiers:NSEventModifierFlagCommand tag:0];
    [self addMenuItemToMenu:browserMenu title:@"New Tab" action:@selector(newTab:) keyEquivalent:@"t" modifiers:NSEventModifierFlagCommand tag:0];
    [self addMenuItemToMenu:browserMenu title:@"Close Tab" action:@selector(closeSelectedTab:) keyEquivalent:@"w" modifiers:NSEventModifierFlagCommand tag:0];
    [self addMenuItemToMenu:browserMenu title:@"Restore Closed Tab" action:@selector(restoreClosedTab:) keyEquivalent:@"t" modifiers:(NSEventModifierFlagCommand | NSEventModifierFlagShift) tag:0];
    [self addMenuItemToMenu:browserMenu title:@"Reload" action:@selector(reloadSelected:) keyEquivalent:@"r" modifiers:NSEventModifierFlagCommand tag:0];
    [browserMenu addItem:NSMenuItem.separatorItem];
    for (NSInteger index = 1; index <= 8; index += 1) {
        [self addMenuItemToMenu:browserMenu title:[NSString stringWithFormat:@"Select Tab %ld", (long)index] action:@selector(selectTabShortcut:) keyEquivalent:[NSString stringWithFormat:@"%ld", (long)index] modifiers:NSEventModifierFlagCommand tag:index - 1];
    }
    [self addMenuItemToMenu:browserMenu title:@"Select Last Tab" action:@selector(selectLastTab:) keyEquivalent:@"9" modifiers:NSEventModifierFlagCommand tag:0];
    unichar leftArrowCharacter = (unichar)NSLeftArrowFunctionKey;
    unichar rightArrowCharacter = (unichar)NSRightArrowFunctionKey;
    NSString *leftArrow = [[NSString alloc] initWithCharacters:&leftArrowCharacter length:1];
    NSString *rightArrow = [[NSString alloc] initWithCharacters:&rightArrowCharacter length:1];
    [self addMenuItemToMenu:browserMenu title:@"Previous Tab" action:@selector(selectAdjacentTab:) keyEquivalent:leftArrow modifiers:(NSEventModifierFlagCommand | NSEventModifierFlagOption) tag:-1];
    [self addMenuItemToMenu:browserMenu title:@"Next Tab" action:@selector(selectAdjacentTab:) keyEquivalent:rightArrow modifiers:(NSEventModifierFlagCommand | NSEventModifierFlagOption) tag:1];
    [self addMenuItemToMenu:browserMenu title:@"Back" action:@selector(goBack:) keyEquivalent:leftArrow modifiers:NSEventModifierFlagCommand tag:0];
    [self addMenuItemToMenu:browserMenu title:@"Forward" action:@selector(goForward:) keyEquivalent:rightArrow modifiers:NSEventModifierFlagCommand tag:0];
    browserItem.submenu = browserMenu;
    [menu addItem:browserItem];
    NSApp.mainMenu = menu;
}

- (void)addMenuItemToMenu:(NSMenu *)menu title:(NSString *)title action:(SEL)action keyEquivalent:(NSString *)keyEquivalent modifiers:(NSEventModifierFlags)modifiers tag:(NSInteger)tag {
    NSMenuItem *item = [[NSMenuItem alloc] initWithTitle:title action:action keyEquivalent:keyEquivalent];
    item.target = self;
    item.keyEquivalentModifierMask = modifiers;
    item.tag = tag;
    [menu addItem:item];
}

- (BOOL)validateMenuItem:(NSMenuItem *)menuItem {
    if (menuItem.action == @selector(restoreClosedTab:)) {
        return self.closedTabs.count > 0 && self.tabs.count < WBOpenTabLimit;
    }
    return YES;
}

- (WBTab *)tabWithIdentifier:(NSUUID *)identifier {
    for (WBTab *tab in self.tabs) {
        if ([tab.identifier isEqual:identifier]) {
            return tab;
        }
    }
    return nil;
}

- (WBTab *)selectedTab {
    return [self tabWithIdentifier:self.selectedTabIdentifier] ?: self.tabs.firstObject;
}

- (void)cancelHistoryRestoration {
    if (!self.isRestoringHistory) {
        return;
    }
    [[self selectedTab] rollbackHistoryNavigation];
    self.isRestoringHistory = NO;
    self.historyNavigation = nil;
    self.isScrollRestorationPending = NO;
}

- (BOOL)isHistoryNavigation:(WKNavigation *)navigation {
    return self.isRestoringHistory && navigation == self.historyNavigation;
}

- (void)resetScrollForSelectedTab {
    WBTab *tab = self.selectedTab;
    tab.scrollX = 0;
    tab.scrollY = 0;
    self.isScrollRestorationPending = NO;
}

- (void)refreshTabStrip {
    for (NSView *view in self.tabStack.arrangedSubviews) {
        [self.tabStack removeArrangedSubview:view];
        [view removeFromSuperview];
    }
    NSUInteger index = 0;
    for (WBTab *tab in self.tabs) {
        NSString *marker = tab.isPinned ? @"• " : @"";
        NSString *title = tab.title;
        if (title.length > 24) {
            NSRange boundary = [title rangeOfComposedCharacterSequenceAtIndex:24];
            NSUInteger length = boundary.location < 24 ? boundary.location : 24;
            title = [[title substringToIndex:length] stringByAppendingString:@"…"];
        }
        NSButton *button = [NSButton buttonWithTitle:[NSString stringWithFormat:@"%@%@", marker, title] target:self action:@selector(selectTab:)];
        button.tag = (NSInteger)index;
        button.bezelStyle = [tab.identifier isEqual:self.selectedTabIdentifier] ? NSBezelStyleTexturedRounded : NSBezelStyleInline;
        button.font = [NSFont systemFontOfSize:12 weight:NSFontWeightRegular];
        NSMenu *tabMenu = [[NSMenu alloc] initWithTitle:@"Tab"];
        [tabMenu addItemWithTitle:tab.isPinned ? @"Unpin" : @"Pin" action:@selector(togglePin:) keyEquivalent:@""];
        for (NSMenuItem *item in tabMenu.itemArray) {
            item.target = self;
            item.representedObject = tab.identifier;
        }
        button.menu = tabMenu;
        [self.tabStack addArrangedSubview:button];
        index += 1;
    }
}

- (void)enqueueTabOperation:(WBTabOperation)operation {
    if (self.tabOperations.count >= WBTabOperationLimit) {
        NSBeep();
        return;
    }
    [self.tabOperations addObject:[operation copy]];
    [self startNextTabOperation];
}

- (void)startNextTabOperation {
    if (self.isProcessingTabOperation || self.tabOperations.count == 0) {
        return;
    }
    self.isProcessingTabOperation = YES;
    WBTabOperation operation = self.tabOperations.firstObject;
    [self.tabOperations removeObjectAtIndex:0];
    __weak typeof(self) weakSelf = self;
    operation(^{
        WBAppController *strongSelf = weakSelf;
        if (!strongSelf) {
            return;
        }
        strongSelf.isProcessingTabOperation = NO;
        [strongSelf startNextTabOperation];
    });
}

- (void)showTabWithIdentifierAfterScrollCapture:(NSUUID *)identifier {
    [self showTabWithIdentifierAfterScrollCapture:identifier initialRequest:nil];
}

- (void)showTabWithIdentifierAfterScrollCapture:(NSUUID *)identifier initialRequest:(nullable NSURLRequest *)initialRequest {
    if (![self tabWithIdentifier:identifier]) {
        return;
    }
    [self releaseLiveView];
    self.selectedTabIdentifier = identifier;
    [self refreshTabStrip];
    [self createLiveViewAndLoadSelectedTabWithRequest:initialRequest];
    [self saveSession];
}

- (void)enqueueNewTabWithURL:(NSURL *)URL focusAddress:(BOOL)isFocusAddress {
    [self enqueueNewTabWithURL:URL initialRequest:nil focusAddress:isFocusAddress];
}

- (void)enqueueNewTabWithURL:(NSURL *)URL initialRequest:(nullable NSURLRequest *)initialRequest focusAddress:(BOOL)isFocusAddress {
    __weak typeof(self) weakSelf = self;
    [self enqueueTabOperation:^(void (^completion)(void)) {
        WBAppController *strongSelf = weakSelf;
        if (!strongSelf) {
            return;
        }
        if (strongSelf.tabs.count >= WBOpenTabLimit) {
            NSBeep();
            completion();
            return;
        }
        [strongSelf captureCurrentScrollWithCompletion:^{
            WBAppController *innerSelf = weakSelf;
            if (!innerSelf) {
                return;
            }
            WBTab *tab = [[WBTab alloc] initWithURL:URL];
            [innerSelf.tabs addObject:tab];
            [innerSelf showTabWithIdentifierAfterScrollCapture:tab.identifier initialRequest:initialRequest];
            if (isFocusAddress) {
                [innerSelf focusAddress:nil];
            }
            completion();
        }];
    }];
}

- (void)newTab:(id)sender {
    [self enqueueNewTabWithURL:[NSURL URLWithString:@"https://www.google.com/"] focusAddress:YES];
}

- (void)recordClosedTab:(WBTab *)tab {
    [self.closedTabs insertObject:tab.dictionaryRepresentation atIndex:0];
    if (self.closedTabs.count > 25) {
        [self.closedTabs removeLastObject];
    }
}

- (void)removeInactiveTab:(WBTab *)tab {
    [self recordClosedTab:tab];
    [self.tabs removeObject:tab];
    [self refreshTabStrip];
    [self saveSession];
}

- (void)closeSelectedTab:(nullable id)sender {
    NSUUID *identifier = [self.selectedTabIdentifier copy];
    if (!identifier) {
        return;
    }
    __weak typeof(self) weakSelf = self;
    [self enqueueTabOperation:^(void (^completion)(void)) {
        WBAppController *strongSelf = weakSelf;
        WBTab *tab = [strongSelf tabWithIdentifier:identifier];
        if (!strongSelf || !tab) {
            completion();
            return;
        }
        if (!strongSelf.webView || ![strongSelf.selectedTabIdentifier isEqual:identifier]) {
            [strongSelf removeInactiveTab:tab];
            completion();
            return;
        }
        [strongSelf captureCurrentScrollWithCompletion:^{
            WBAppController *innerSelf = weakSelf;
            WBTab *capturedTab = [innerSelf tabWithIdentifier:identifier];
            if (!innerSelf || !capturedTab) {
                completion();
                return;
            }
            if (!innerSelf.webView || ![innerSelf.selectedTabIdentifier isEqual:identifier]) {
                [innerSelf removeInactiveTab:capturedTab];
                completion();
                return;
            }
            NSUInteger index = [innerSelf.tabs indexOfObject:capturedTab];
            [innerSelf releaseLiveView];
            [innerSelf recordClosedTab:capturedTab];
            [innerSelf.tabs removeObject:capturedTab];
            if (innerSelf.tabs.count == 0) {
                [innerSelf.tabs addObject:[[WBTab alloc] initWithURL:[NSURL URLWithString:@"https://www.google.com/"]]];
                index = 0;
            }
            NSUInteger selectedIndex = MIN(index, innerSelf.tabs.count - 1);
            [innerSelf showTabWithIdentifierAfterScrollCapture:innerSelf.tabs[selectedIndex].identifier];
            completion();
        }];
    }];
}

- (void)restoreClosedTab:(nullable id)sender {
    __weak typeof(self) weakSelf = self;
    [self enqueueTabOperation:^(void (^completion)(void)) {
        WBAppController *strongSelf = weakSelf;
        if (!strongSelf) {
            return;
        }
        if (strongSelf.tabs.count >= WBOpenTabLimit) {
            NSBeep();
            completion();
            return;
        }
        [strongSelf captureCurrentScrollWithCompletion:^{
            WBAppController *innerSelf = weakSelf;
            if (!innerSelf) {
                return;
            }
            NSDictionary *dictionary = innerSelf.closedTabs.firstObject;
            WBTab *tab = dictionary ? [[WBTab alloc] initWithDictionary:dictionary] : nil;
            if (!tab) {
                completion();
                return;
            }
            [innerSelf.closedTabs removeObjectAtIndex:0];
            [innerSelf.tabs addObject:tab];
            [innerSelf showTabWithIdentifierAfterScrollCapture:tab.identifier];
            completion();
        }];
    }];
}

- (void)selectTab:(NSButton *)sender {
    [self switchToTabAtIndex:(NSUInteger)sender.tag];
}

- (void)switchToTabAtIndex:(NSUInteger)index {
    if (index >= self.tabs.count) {
        return;
    }
    NSUUID *identifier = self.tabs[index].identifier;
    __weak typeof(self) weakSelf = self;
    [self enqueueTabOperation:^(void (^completion)(void)) {
        WBAppController *strongSelf = weakSelf;
        if (!strongSelf || ![strongSelf tabWithIdentifier:identifier]) {
            completion();
            return;
        }
        [strongSelf selectIdentifier:identifier completion:completion];
    }];
}

- (void)selectIdentifier:(NSUUID *)identifier completion:(void (^)(void))completion {
    WBTab *tab = [self tabWithIdentifier:identifier];
    if (!tab || ([self.selectedTabIdentifier isEqual:identifier] && self.webView)) {
        completion();
        return;
    }
    if (!self.webView) {
        self.selectedTabIdentifier = identifier;
        [self refreshTabStrip];
        [self createLiveViewAndLoadSelectedTab];
        [self saveSession];
        completion();
        return;
    }
    __weak typeof(self) weakSelf = self;
    [self captureCurrentScrollWithCompletion:^{
        WBAppController *strongSelf = weakSelf;
        if (!strongSelf) {
            return;
        }
        if ([strongSelf tabWithIdentifier:identifier]) {
            [strongSelf showTabWithIdentifierAfterScrollCapture:identifier];
        }
        completion();
    }];
}

- (void)captureCurrentScrollWithCompletion:(void (^)(void))completion {
    WBTab *tab = self.selectedTab;
    if (!self.webView || !tab) {
        completion();
        return;
    }
    __block BOOL isScrollCaptureComplete = NO;
    __block NSTimer *timeoutTimer = nil;
    void (^completeScrollCapture)(id, NSError *) = ^(id result, NSError *error) {
        if (isScrollCaptureComplete) {
            return;
        }
        isScrollCaptureComplete = YES;
        [timeoutTimer invalidate];
        timeoutTimer = nil;
        if ([result isKindOfClass:NSDictionary.class] && !error) {
            NSDictionary *state = result;
            id scrollX = state[@"x"];
            id scrollY = state[@"y"];
            if ([scrollX isKindOfClass:NSNumber.class] && CFGetTypeID((__bridge CFTypeRef)scrollX) != CFBooleanGetTypeID()) {
                tab.scrollX = [scrollX doubleValue];
            }
            if ([scrollY isKindOfClass:NSNumber.class] && CFGetTypeID((__bridge CFTypeRef)scrollY) != CFBooleanGetTypeID()) {
                tab.scrollY = [scrollY doubleValue];
            }
        }
        completion();
    };
    timeoutTimer = [NSTimer timerWithTimeInterval:WBScrollCaptureTimeout repeats:NO block:^(NSTimer *timer) {
        [timer invalidate];
        completeScrollCapture(nil, nil);
    }];
    [NSRunLoop.mainRunLoop addTimer:timeoutTimer forMode:NSRunLoopCommonModes];
    NSString *script = @"({x:window.scrollX||0,y:window.scrollY||0})";
    [self.webView evaluateJavaScript:script completionHandler:^(id result, NSError *error) {
        completeScrollCapture(result, error);
    }];
}

- (void)createLiveViewAndLoadSelectedTab {
    [self createLiveViewAndLoadSelectedTabWithRequest:nil];
}

- (void)createLiveViewAndLoadSelectedTabWithRequest:(nullable NSURLRequest *)initialRequest {
    if (self.webView) {
        return;
    }
    WBTab *tab = self.selectedTab;
    if (!tab) {
        return;
    }
    WKWebViewConfiguration *configuration = [[WKWebViewConfiguration alloc] init];
    configuration.websiteDataStore = WKWebsiteDataStore.defaultDataStore;
    WKWebView *view = [[WKWebView alloc] initWithFrame:self.contentView.bounds configuration:configuration];
    view.autoresizingMask = NSViewWidthSizable | NSViewHeightSizable;
    view.navigationDelegate = self;
    view.UIDelegate = self;
    self.webView = view;
    self.isScrollRestorationPending = YES;
    [self.contentView addSubview:view];
    [view loadRequest:initialRequest ?: [NSURLRequest requestWithURL:tab.URL]];
    self.addressField.stringValue = tab.URL.absoluteString;
}

- (void)releaseLiveView {
    [NSObject cancelPreviousPerformRequestsWithTarget:self selector:@selector(saveSession) object:nil];
    [self cancelHistoryRestoration];
    self.isScrollRestorationPending = NO;
    self.webView.navigationDelegate = nil;
    self.webView.UIDelegate = nil;
    [self.webView stopLoading];
    [self.webView removeFromSuperview];
    self.webView = nil;
}

- (BOOL)control:(NSControl *)control textView:(NSTextView *)textView doCommandBySelector:(SEL)commandSelector {
    if (control == self.addressField && commandSelector == @selector(insertNewline:)) {
        [self navigateAddress:control];
        return YES;
    }
    return NO;
}

- (void)navigateAddress:(id)sender {
    NSURL *URL = WBURLForInput(self.addressField.stringValue);
    if (!URL) {
        NSAlert *alert = [[NSAlert alloc] init];
        alert.messageText = @"Unsupported address";
        alert.informativeText = @"Use an http or https address, or enter search text.";
        [alert runModal];
        return;
    }
    [self cancelHistoryRestoration];
    [self resetScrollForSelectedTab];
    [self.webView loadRequest:[NSURLRequest requestWithURL:URL]];
}

- (void)focusAddress:(id)sender {
    [self.window makeFirstResponder:self.addressField];
    [self.addressField selectText:nil];
}

- (void)reloadSelected:(id)sender {
    [self cancelHistoryRestoration];
    self.isScrollRestorationPending = NO;
    WBTab *tab = self.selectedTab;
    if (tab.isContentProcessTerminated) {
        tab.isContentProcessTerminated = NO;
        [self.webView loadRequest:[NSURLRequest requestWithURL:tab.URL]];
        return;
    }
    [self.webView reload:nil];
}

- (void)navigateHistoryByOffset:(NSInteger)offset {
    if (self.isRestoringHistory) {
        return;
    }
    WBTab *tab = self.selectedTab;
    if ((offset < 0 && ![tab isAbleToGoBack]) || (offset > 0 && ![tab isAbleToGoForward])) {
        return;
    }
    NSURL *URL = [tab URLAtHistoryOffset:offset];
    self.isRestoringHistory = YES;
    self.isScrollRestorationPending = YES;
    self.historyNavigation = [self.webView loadRequest:[NSURLRequest requestWithURL:URL]];
    if (!self.historyNavigation) {
        [tab rollbackHistoryNavigation];
        self.isRestoringHistory = NO;
        self.isScrollRestorationPending = NO;
        self.addressField.stringValue = tab.URL.absoluteString;
        [self saveSession];
    }
}

- (void)goBack:(id)sender {
    [self navigateHistoryByOffset:-1];
}

- (void)goForward:(id)sender {
    [self navigateHistoryByOffset:1];
}

- (void)selectTabShortcut:(NSMenuItem *)sender {
    [self switchToTabAtIndex:(NSUInteger)sender.tag];
}

- (void)selectLastTab:(id)sender {
    [self switchToTabAtIndex:self.tabs.count - 1];
}

- (void)selectAdjacentTab:(NSMenuItem *)sender {
    NSInteger offset = sender.tag;
    __weak typeof(self) weakSelf = self;
    [self enqueueTabOperation:^(void (^completion)(void)) {
        WBAppController *strongSelf = weakSelf;
        if (strongSelf.tabs.count < 2) {
            completion();
            return;
        }
        WBTab *currentTab = strongSelf.selectedTab;
        NSUInteger currentIndex = [strongSelf.tabs indexOfObject:currentTab];
        NSInteger nextIndex = (NSInteger)currentIndex + offset;
        if (nextIndex < 0) {
            nextIndex = (NSInteger)strongSelf.tabs.count - 1;
        }
        if (nextIndex >= (NSInteger)strongSelf.tabs.count) {
            nextIndex = 0;
        }
        [strongSelf selectIdentifier:strongSelf.tabs[(NSUInteger)nextIndex].identifier completion:completion];
    }];
}

- (void)togglePin:(NSMenuItem *)sender {
    NSUUID *identifier = sender.representedObject;
    __weak typeof(self) weakSelf = self;
    [self enqueueTabOperation:^(void (^completion)(void)) {
        WBAppController *strongSelf = weakSelf;
        for (WBTab *tab in strongSelf.tabs) {
            if ([tab.identifier isEqual:identifier]) {
                tab.isPinned = !tab.isPinned;
                [strongSelf.tabs removeObject:tab];
                NSUInteger index = tab.isPinned ? 0 : strongSelf.tabs.count;
                [strongSelf.tabs insertObject:tab atIndex:index];
                [strongSelf refreshTabStrip];
                [strongSelf saveSession];
                break;
            }
        }
        completion();
    }];
}

- (void)showSessionSaveFailureAlert {
    if (self.isSessionSaveFailureReported) {
        return;
    }
    self.isSessionSaveFailureReported = YES;
    [self.window makeKeyAndOrderFront:nil];
    [NSApp activate];
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = @"Session could not be saved.";
    alert.informativeText = @"Recent browser state may not be available the next time Web opens.";
    [alert runModal];
}

- (BOOL)saveSession {
    if (self.tabs.count == 0 || ![self tabWithIdentifier:self.selectedTabIdentifier]) {
        return YES;
    }
    NSError *error = nil;
    BOOL isSaved = [self.sessionStore saveTabs:self.tabs selectedIdentifier:self.selectedTabIdentifier closedTabs:self.closedTabs error:&error];
    if (isSaved) {
        self.isSessionSaveFailureReported = NO;
        return YES;
    }
    [self showSessionSaveFailureAlert];
    return NO;
}

- (void)scheduleScrollSave {
    [NSObject cancelPreviousPerformRequestsWithTarget:self selector:@selector(saveSession) object:nil];
    [self performSelector:@selector(saveSession) withObject:nil afterDelay:1.0];
}

- (void)webView:(WKWebView *)webView didCommitNavigation:(WKNavigation *)navigation {
    WBTab *tab = self.selectedTab;
    if (!tab || webView != self.webView || !webView.URL) {
        return;
    }
    NSURL *URL = WBURLForPersistableURL(webView.URL, tab.URL);
    if ([self isHistoryNavigation:navigation]) {
        [tab commitHistoryNavigationToURL:URL];
        [tab completeHistoryNavigation];
        tab.scrollX = 0;
        tab.scrollY = 0;
    } else {
        [tab recordURL:URL];
    }
    tab.title = webView.title.length > 0 ? webView.title : tab.URL.host;
    self.addressField.stringValue = tab.URL.absoluteString;
    [self refreshTabStrip];
    [self saveSession];
}

- (void)webView:(WKWebView *)webView didFinishNavigation:(WKNavigation *)navigation {
    WBTab *tab = self.selectedTab;
    if (!tab || webView != self.webView) {
        return;
    }
    if ([self isHistoryNavigation:navigation]) {
        [tab completeHistoryNavigation];
        self.isRestoringHistory = NO;
        self.historyNavigation = nil;
    }
    tab.title = webView.title.length > 0 ? webView.title : tab.URL.host;
    if (self.isScrollRestorationPending) {
        NSString *script = [NSString stringWithFormat:@"window.scrollTo(%0.17g,%0.17g);", tab.scrollX, tab.scrollY];
        [webView evaluateJavaScript:script completionHandler:nil];
        self.isScrollRestorationPending = NO;
    }
    [self refreshTabStrip];
    [self saveSession];
    [self scheduleScrollSave];
}

- (void)reconcileFailedHistoryNavigation:(WKNavigation *)navigation inWebView:(WKWebView *)webView {
    if (webView != self.webView || ![self isHistoryNavigation:navigation]) {
        return;
    }
    WBTab *tab = self.selectedTab;
    [tab rollbackHistoryNavigation];
    self.isRestoringHistory = NO;
    self.historyNavigation = nil;
    self.isScrollRestorationPending = NO;
    self.addressField.stringValue = tab.URL.absoluteString;
    [self refreshTabStrip];
    [self saveSession];
}

- (void)webView:(WKWebView *)webView didFailProvisionalNavigation:(WKNavigation *)navigation withError:(NSError *)error {
    [self reconcileFailedHistoryNavigation:navigation inWebView:webView];
}

- (void)webView:(WKWebView *)webView didFailNavigation:(WKNavigation *)navigation withError:(NSError *)error {
    [self reconcileFailedHistoryNavigation:navigation inWebView:webView];
}

- (void)webView:(WKWebView *)webView didReceiveAuthenticationChallenge:(NSURLAuthenticationChallenge *)challenge completionHandler:(void (^)(NSURLSessionAuthChallengeDisposition, NSURLCredential * _Nullable))completionHandler {
    completionHandler(NSURLSessionAuthChallengePerformDefaultHandling, nil);
}

- (void)webViewWebContentProcessDidTerminate:(WKWebView *)webView {
    WBTab *tab = self.selectedTab;
    if (tab && webView == self.webView) {
        [self cancelHistoryRestoration];
        tab.isContentProcessTerminated = YES;
        self.addressField.stringValue = @"Content process stopped. Reload this tab.";
        [self saveSession];
    }
}

- (void)webView:(WKWebView *)webView decidePolicyForNavigationAction:(WKNavigationAction *)navigationAction decisionHandler:(void (^)(WKNavigationActionPolicy))decisionHandler {
    if (navigationAction.shouldPerformDownload) {
        decisionHandler(WKNavigationActionPolicyDownload);
        return;
    }
    if (webView == self.webView && navigationAction.targetFrame.isMainFrame && (navigationAction.navigationType == WKNavigationTypeLinkActivated || navigationAction.navigationType == WKNavigationTypeFormSubmitted)) {
        [self cancelHistoryRestoration];
        [self resetScrollForSelectedTab];
    }
    decisionHandler(WKNavigationActionPolicyAllow);
}

- (void)webView:(WKWebView *)webView decidePolicyForNavigationResponse:(WKNavigationResponse *)navigationResponse decisionHandler:(void (^)(WKNavigationResponsePolicy))decisionHandler {
    decisionHandler(navigationResponse.canShowMIMEType ? WKNavigationResponsePolicyAllow : WKNavigationResponsePolicyDownload);
}

- (void)webView:(WKWebView *)webView navigationAction:(WKNavigationAction *)navigationAction didBecomeDownload:(WKDownload *)download {
    [self beginDownload:download];
}

- (void)webView:(WKWebView *)webView navigationResponse:(WKNavigationResponse *)navigationResponse didBecomeDownload:(WKDownload *)download {
    [self beginDownload:download];
}

- (void)beginDownload:(WKDownload *)download {
    download.delegate = self;
    [self.downloads addObject:download];
}

- (nullable NSURL *)uniqueDownloadStagingURL {
    NSFileManager *manager = NSFileManager.defaultManager;
    for (NSUInteger index = 0; index < 10; index += 1) {
        NSURL *URL = [manager.temporaryDirectory URLByAppendingPathComponent:NSUUID.UUID.UUIDString];
        if (![manager fileExistsAtPath:URL.path]) {
            return URL;
        }
    }
    return nil;
}

- (void)showDownloadReplacementError:(NSError *)error {
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = @"Download could not replace the existing file.";
    alert.informativeText = error.localizedDescription ?: @"The original file was left unchanged.";
    [alert runModal];
}

- (BOOL)moveStagedDownloadAtURL:(NSURL *)stagedURL toFinalURL:(NSURL *)finalURL error:(NSError * _Nullable *)error {
    NSFileManager *manager = NSFileManager.defaultManager;
    BOOL isDirectory = NO;
    if (![manager fileExistsAtPath:finalURL.path isDirectory:&isDirectory]) {
        return [manager moveItemAtURL:stagedURL toURL:finalURL error:error];
    }
    if (isDirectory) {
        if (error) {
            *error = [NSError errorWithDomain:@"Web.Download" code:1 userInfo:@{NSLocalizedDescriptionKey: @"The selected destination is a directory."}];
        }
        return NO;
    }
    return [manager replaceItemAtURL:finalURL withItemAtURL:stagedURL backupItemName:NSUUID.UUID.UUIDString options:0 resultingItemURL:nil error:error];
}

- (void)download:(WKDownload *)download decideDestinationUsingResponse:(NSURLResponse *)response suggestedFilename:(NSString *)suggestedFilename completionHandler:(void (^)(NSURL * _Nullable))completionHandler {
    NSSavePanel *panel = [NSSavePanel savePanel];
    panel.nameFieldStringValue = suggestedFilename;
    [panel beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse result) {
        NSURL *finalURL = result == NSModalResponseOK ? panel.URL : nil;
        if (!finalURL) {
            completionHandler(nil);
            return;
        }
        BOOL isDirectory = NO;
        if ([NSFileManager.defaultManager fileExistsAtPath:finalURL.path isDirectory:&isDirectory] && isDirectory) {
            [self showDownloadReplacementError:[NSError errorWithDomain:@"Web.Download" code:1 userInfo:@{NSLocalizedDescriptionKey: @"The selected destination is a directory."}]];
            completionHandler(nil);
            return;
        }
        NSURL *stagedURL = [self uniqueDownloadStagingURL];
        if (!stagedURL) {
            [self showDownloadReplacementError:[NSError errorWithDomain:@"Web.Download" code:2 userInfo:@{NSLocalizedDescriptionKey: @"A temporary download location could not be created."}]];
            completionHandler(nil);
            return;
        }
        [self.downloadDestinations setObject:@{ @"stagedURL": stagedURL, @"finalURL": finalURL } forKey:download];
        completionHandler(stagedURL);
    }];
}

- (BOOL)finishDownload:(WKDownload *)download isSuccessful:(BOOL)isSuccessful {
    BOOL isTrackedDownload = [self.downloads containsObject:download];
    if (!isTrackedDownload) {
        return NO;
    }
    NSDictionary<NSString *, NSURL *> *destinations = [self.downloadDestinations objectForKey:download];
    NSURL *stagedURL = destinations[@"stagedURL"];
    NSURL *finalURL = destinations[@"finalURL"];
    if (isSuccessful && stagedURL && finalURL) {
        NSError *error = nil;
        if (![self moveStagedDownloadAtURL:stagedURL toFinalURL:finalURL error:&error]) {
            [[NSFileManager defaultManager] removeItemAtURL:stagedURL error:nil];
            [self showDownloadReplacementError:error];
        }
    } else if (stagedURL) {
        [[NSFileManager defaultManager] removeItemAtURL:stagedURL error:nil];
    }
    [self.downloadDestinations removeObjectForKey:download];
    [self.downloads removeObject:download];
    return YES;
}

- (void)downloadDidFinish:(WKDownload *)download {
    [self finishDownload:download isSuccessful:YES];
}

- (void)showDownloadFailureAlert {
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = @"Download failed";
    alert.informativeText = @"The download could not be completed.";
    [alert runModal];
}

- (void)download:(WKDownload *)download didFailWithError:(NSError *)error resumeData:(NSData *)resumeData {
    if ([self finishDownload:download isSuccessful:NO]) {
        [self showDownloadFailureAlert];
    }
}

- (nullable WKWebView *)webView:(WKWebView *)webView createWebViewWithConfiguration:(WKWebViewConfiguration *)configuration forNavigationAction:(WKNavigationAction *)navigationAction windowFeatures:(WKWindowFeatures *)windowFeatures {
    NSURL *sourceURL = WBURLForPersistableURL(navigationAction.sourceFrame.request.URL, webView.URL);
    NSURL *URL = WBURLForPersistableURL(navigationAction.request.URL, sourceURL);
    NSURLRequest *request = WBURLForInput(navigationAction.request.URL.absoluteString) ? [navigationAction.request copy] : nil;
    if (navigationAction.navigationType == WKNavigationTypeLinkActivated || navigationAction.navigationType == WKNavigationTypeFormSubmitted) {
        [self enqueueNewTabWithURL:URL initialRequest:request focusAddress:NO];
        return nil;
    }
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = @"Allow popup?";
    alert.informativeText = @"A script-created popup opens as a new tab and loses its script handle after unloading.";
    [alert addButtonWithTitle:@"Open"];
    [alert addButtonWithTitle:@"Cancel"];
    if ([alert runModal] == NSAlertFirstButtonReturn) {
        [self enqueueNewTabWithURL:URL initialRequest:request focusAddress:NO];
    }
    return nil;
}

- (void)webView:(WKWebView *)webView runOpenPanelWithParameters:(WKOpenPanelParameters *)parameters initiatedByFrame:(WKFrameInfo *)frame completionHandler:(void (^)(NSArray<NSURL *> * _Nullable))completionHandler {
    NSOpenPanel *panel = [NSOpenPanel openPanel];
    panel.allowsMultipleSelection = parameters.allowsMultipleSelection;
    panel.canChooseDirectories = parameters.allowsDirectories;
    panel.canChooseFiles = YES;
    [panel beginSheetModalForWindow:self.window completionHandler:^(NSModalResponse result) {
        completionHandler(result == NSModalResponseOK ? panel.URLs : nil);
    }];
}

- (void)webView:(WKWebView *)webView requestMediaCapturePermissionForOrigin:(WKSecurityOrigin *)origin initiatedByFrame:(WKFrameInfo *)frame type:(WKMediaCaptureType)type decisionHandler:(void (^)(WKPermissionDecision))decisionHandler {
    NSString *resource = type == WKMediaCaptureTypeCamera ? @"camera" : type == WKMediaCaptureTypeMicrophone ? @"microphone" : @"camera and microphone";
    NSAlert *alert = [[NSAlert alloc] init];
    alert.messageText = [NSString stringWithFormat:@"Allow %@?", resource];
    alert.informativeText = [NSString stringWithFormat:@"%@ requested access.", origin.host];
    [alert addButtonWithTitle:@"Allow"];
    [alert addButtonWithTitle:@"Don’t Allow"];
    decisionHandler([alert runModal] == NSAlertFirstButtonReturn ? WKPermissionDecisionGrant : WKPermissionDecisionDeny);
}

@end
