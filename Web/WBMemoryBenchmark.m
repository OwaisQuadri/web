#import "WBMemoryBenchmark.h"
#import <AppKit/AppKit.h>
#import <WebKit/WebKit.h>
#import <libproc.h>
#import <sys/proc_info.h>
#import <sys/resource.h>
#import <unistd.h>
#import <math.h>
#import <string.h>

static const NSUInteger WBObservationCount = 11;
static const NSTimeInterval WBSampleInterval = 1.0;
static const NSUInteger WBSampleDurationMilliseconds = 10000;
static const NSUInteger WBRepetitionCount = 3;
static const uint64_t WBProductionCeilingBytes = 2000000000;

@interface WBMemoryLoadDelegate : NSObject <WKNavigationDelegate>
@property (nonatomic) NSUInteger finishedCount;
@property (nonatomic) NSUInteger failedCount;
@end

@implementation WBMemoryLoadDelegate
- (void)webView:(WKWebView *)webView didFinishNavigation:(WKNavigation *)navigation {
    self.finishedCount += 1;
}
- (void)webView:(WKWebView *)webView didFailNavigation:(WKNavigation *)navigation withError:(NSError *)error {
    self.failedCount += 1;
}
- (void)webView:(WKWebView *)webView didFailProvisionalNavigation:(WKNavigation *)navigation withError:(NSError *)error {
    self.failedCount += 1;
}
@end

@interface WBWeakView : NSObject
@property (nonatomic, weak) WKWebView *view;
@end

@implementation WBWeakView
@end

@implementation WBMemoryBenchmark

+ (int)runStrictBenchmark {
    @autoreleasepool {
        NSError *error = nil;
        NSDictionary *result = [self runBenchmarkWithError:&error];
        NSData *data = [NSJSONSerialization dataWithJSONObject:result options:NSJSONWritingPrettyPrinted error:nil];
        NSString *name = [NSString stringWithFormat:@"Web-memory-benchmark-%@.json", NSUUID.UUID.UUIDString];
        NSURL *outputURL = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:name]];
        BOOL isWritten = [data writeToURL:outputURL options:NSDataWritingAtomic error:&error];
        if (!isWritten) {
            fprintf(stderr, "memory benchmark could not write its receipt\n");
            return 1;
        }
        printf("WEB_MEMORY_RESULT %s\n", outputURL.path.UTF8String);
        if (error) {
            fprintf(stderr, "memory benchmark failed: %s\n", error.localizedDescription.UTF8String);
            return 1;
        }
        return [self isValidBenchmarkDocument:result] ? 0 : 1;
    }
}

+ (NSDictionary *)runBenchmarkWithError:(NSError * _Nullable *)error {
    NSApplication *application = NSApplication.sharedApplication;
    [application setActivationPolicy:NSApplicationActivationPolicyRegular];
    NSMutableArray<NSDictionary *> *results = [NSMutableArray array];
    if ([self isSameNamedApplicationRunningWithCurrentProcessIdentifierExcluded:error]) {
        NSError *baselineError = error ? *error : nil;
        return [self failedDocumentWithResults:results error:baselineError ?: [self errorWithDescription:@"Another benchmark application is running."]];
    }
    NSSet<NSString *> *serviceBaseline = [self webKitServiceIdentityStrings];
    NSArray<NSNumber *> *tabCounts = @[@7, @12, @20];
    for (NSNumber *tabCountValue in tabCounts) {
        NSUInteger tabCount = tabCountValue.unsignedIntegerValue;
        for (NSUInteger repetition = 1; repetition <= WBRepetitionCount; repetition += 1) {
            __strong NSError *caseError = nil;
            __strong NSArray<NSDictionary *> *caseResults = nil;
            @autoreleasepool {
                caseResults = [self runCaseWithTabCount:tabCount repetition:repetition isSplit:NO serviceBaseline:serviceBaseline error:&caseError];
                if (caseResults) {
                    [results addObjectsFromArray:caseResults];
                }
            }
            if (!caseResults) {
                if (error) {
                    *error = caseError;
                }
                return [self failedDocumentWithResults:results error:caseError];
            }
        }
    }
    for (NSUInteger repetition = 1; repetition <= WBRepetitionCount; repetition += 1) {
        __strong NSError *caseError = nil;
        __strong NSArray<NSDictionary *> *caseResults = nil;
        @autoreleasepool {
            caseResults = [self runCaseWithTabCount:7 repetition:repetition isSplit:YES serviceBaseline:serviceBaseline error:&caseError];
            if (caseResults) {
                [results addObjectsFromArray:caseResults];
            }
        }
        if (!caseResults) {
            if (error) {
                *error = caseError;
            }
            return [self failedDocumentWithResults:results error:caseError];
        }
    }
    NSDictionary *document = @{
        @"version": @1,
        @"outcome": @"complete",
        @"observationsPerPhase": @(WBObservationCount),
        @"sampleDurationMilliseconds": @(WBSampleDurationMilliseconds),
        @"repetitions": @(WBRepetitionCount),
        @"productionCeilingBytes": @(WBProductionCeilingBytes),
        @"results": results
    };
    if (![self isValidBenchmarkDocument:document]) {
        NSError *validationError = [self errorWithDescription:@"Final benchmark receipts did not retain run-wide service membership."];
        if (error) {
            *error = validationError;
        }
        return [self failedDocumentWithResults:results error:validationError];
    }
    return document;
}

+ (NSDictionary *)failedDocumentWithResults:(NSArray<NSDictionary *> *)results error:(NSError *)error {
    return @{
        @"version": @1,
        @"outcome": @"failed",
        @"observationsPerPhase": @(WBObservationCount),
        @"sampleDurationMilliseconds": @(WBSampleDurationMilliseconds),
        @"repetitions": @(WBRepetitionCount),
        @"productionCeilingBytes": @(WBProductionCeilingBytes),
        @"failure": error.localizedDescription ?: @"Unknown benchmark failure",
        @"results": results
    };
}

+ (nullable NSArray<NSDictionary *> *)runCaseWithTabCount:(NSUInteger)tabCount repetition:(NSUInteger)repetition isSplit:(BOOL)isSplit serviceBaseline:(NSSet<NSString *> *)serviceBaseline error:(NSError * _Nullable *)error {
    NSRect frame = NSMakeRect(0, 0, 800, 600);
    NSWindow *window = [[NSWindow alloc] initWithContentRect:frame styleMask:(NSWindowStyleMaskTitled | NSWindowStyleMaskClosable) backing:NSBackingStoreBuffered defer:NO];
    window.releasedWhenClosed = NO;
    window.title = @"Web memory benchmark";
    [window makeKeyAndOrderFront:nil];
    WBMemoryLoadDelegate *delegate = [[WBMemoryLoadDelegate alloc] init];
    WKWebsiteDataStore *store = [WKWebsiteDataStore nonPersistentDataStore];
    NSMutableArray<WKWebView *> *views = [NSMutableArray arrayWithCapacity:tabCount];
    NSMutableArray<WBWeakView *> *weakViews = [NSMutableArray array];
    NSString *document = [self benchmarkDocumentForTabCount:tabCount];
    for (NSUInteger index = 0; index < tabCount; index += 1) {
        @autoreleasepool {
            WKWebViewConfiguration *configuration = [[WKWebViewConfiguration alloc] init];
            configuration.websiteDataStore = store;
            WKWebView *view = [[WKWebView alloc] initWithFrame:[self frameForIndex:index tabCount:tabCount isSplit:isSplit] configuration:configuration];
            view.navigationDelegate = delegate;
            if (index == 0 || (isSplit && index < 2)) {
                [window.contentView addSubview:view];
            }
            [view loadHTMLString:document baseURL:[NSURL URLWithString:@"https://benchmark.invalid/"]];
            [views addObject:view];
        }
    }
    BOOL isLoaded = [self waitForDelegate:delegate expectedCount:tabCount timeout:30.0];
    if (!isLoaded) {
        [self releaseViews:views weakViews:weakViews];
        [window close];
        if (error) {
            *error = [self errorWithDescription:@"In-memory WebKit workload did not finish loading."];
        }
        return nil;
    }
    [self runLoopFor:1.0];
    NSMutableArray<NSDictionary *> *phases = [NSMutableArray array];
    if (isSplit) {
        NSDictionary *splitStressMeasurement = [self samplePhaseMeasurementWithServiceBaseline:serviceBaseline error:error];
        if (!splitStressMeasurement) {
            [self releaseViews:views weakViews:weakViews];
            [window close];
            return nil;
        }
        @autoreleasepool {
            [self releaseViews:views weakViews:weakViews];
            window.contentView = [[NSView alloc] initWithFrame:window.frame];
            [views removeAllObjects];
        }
        BOOL isReleased = [self observeReleasedViews:weakViews timeout:5.0];
        [window close];
        if (!isReleased) {
            if (error && !*error) {
                *error = [self errorWithDescription:@"Split-stress native WebKit views did not release."];
            }
            return nil;
        }
        NSDictionary *splitStress = [self receiptForPhase:@"split-stress" tabCount:tabCount repetition:repetition isSplit:YES measurement:splitStressMeasurement isNativeReleaseObserved:YES error:error];
        if (!splitStress) {
            return nil;
        }
        [phases addObject:splitStress];
        return phases;
    }
    NSDictionary *loaded = [self samplePhase:@"loaded" tabCount:tabCount repetition:repetition isSplit:NO isNativeReleaseObserved:NO serviceBaseline:serviceBaseline error:error];
    if (!loaded) {
        [self releaseViews:views weakViews:weakViews];
        [window close];
        return nil;
    }
    [phases addObject:loaded];
    @autoreleasepool {
        WKWebView *selectedView = views.firstObject;
        NSMutableArray<WKWebView *> *unloaded = [NSMutableArray arrayWithArray:views];
        [unloaded removeObjectAtIndex:0];
        [self releaseViews:unloaded weakViews:weakViews];
        NSView *liveContent = [[NSView alloc] initWithFrame:window.frame];
        window.contentView = liveContent;
        selectedView.frame = liveContent.bounds;
        [liveContent addSubview:selectedView];
        [views removeAllObjects];
        [unloaded removeAllObjects];
        [views addObject:selectedView];
        selectedView = nil;
    }
    BOOL isNativeReleaseObserved = [self observeReleasedViews:weakViews timeout:5.0];
    if (!isNativeReleaseObserved) {
        [self releaseViews:views weakViews:weakViews];
        [window close];
        if (error) {
            *error = [self errorWithDescription:[NSString stringWithFormat:@"Unloaded native WebKit views did not release (%lu of %lu retained).", (unsigned long)[self unreleasedViewCount:weakViews], (unsigned long)weakViews.count]];
        }
        return nil;
    }
    [self runLoopFor:1.0];
    NSDictionary *oneLive = [self samplePhase:@"one-live" tabCount:tabCount repetition:repetition isSplit:isSplit isNativeReleaseObserved:YES serviceBaseline:serviceBaseline error:error];
    NSNumber *oneLivePeak = oneLive[@"peakDeduplicatedBytes"];
    if (!oneLive || oneLivePeak.unsignedLongLongValue >= WBProductionCeilingBytes) {
        [self releaseViews:views weakViews:weakViews];
        [window close];
        if (error && !*error) {
            *error = [self errorWithDescription:@"One-live production policy exceeded the memory ceiling."];
        }
        return nil;
    }
    [phases addObject:oneLive];
    @autoreleasepool {
        [self releaseViews:views weakViews:weakViews];
        window.contentView = [[NSView alloc] initWithFrame:window.frame];
        [views removeAllObjects];
    }
    BOOL isSelectedReleased = [self observeReleasedViews:weakViews timeout:5.0];
    if (!isSelectedReleased) {
        [window close];
        if (error) {
            *error = [self errorWithDescription:@"Selected native WebKit view did not release before reopening."];
        }
        return nil;
    }
    NSUInteger expectedCount = delegate.finishedCount + 1;
    WBWeakView *reopenedWeakView = [[WBWeakView alloc] init];
    __strong NSDictionary *reopenedMeasurement = nil;
    __strong NSError *reopenedError = nil;
    BOOL isReopened = NO;
    @autoreleasepool {
        WKWebViewConfiguration *reopenedConfiguration = [[WKWebViewConfiguration alloc] init];
        reopenedConfiguration.websiteDataStore = store;
        WKWebView *reopenedView = [[WKWebView alloc] initWithFrame:window.contentView.bounds configuration:reopenedConfiguration];
        reopenedView.navigationDelegate = delegate;
        [window.contentView addSubview:reopenedView];
        [reopenedView loadHTMLString:document baseURL:[NSURL URLWithString:@"https://benchmark.invalid/"]];
        isReopened = [self waitForDelegate:delegate expectedCount:expectedCount timeout:30.0];
        if (isReopened) {
            [self runLoopFor:1.0];
            reopenedMeasurement = [self samplePhaseMeasurementWithServiceBaseline:serviceBaseline error:&reopenedError];
        }
        reopenedWeakView.view = reopenedView;
        [reopenedView stopLoading];
        reopenedView.navigationDelegate = nil;
        [reopenedView removeFromSuperviewWithoutNeedingDisplay];
    }
    if (reopenedError && error) {
        *error = reopenedError;
    }
    if (!isReopened || !reopenedMeasurement) {
        [window close];
        if (error && !*error) {
            *error = [self errorWithDescription:@"Reopened in-memory WebKit workload did not finish loading."];
        }
        return nil;
    }
    BOOL isReopenedReleased = [self observeReleasedViews:@[reopenedWeakView] timeout:5.0];
    if (!isReopenedReleased) {
        [window close];
        if (error) {
            *error = [self errorWithDescription:@"Reopened native WebKit view did not release after sampling."];
        }
        return nil;
    }
    NSDictionary *reopened = [self receiptForPhase:@"reopened" tabCount:tabCount repetition:repetition isSplit:isSplit measurement:reopenedMeasurement isNativeReleaseObserved:YES error:error];
    [window close];
    NSNumber *reopenedPeak = reopened[@"peakDeduplicatedBytes"];
    if (!reopened || reopenedPeak.unsignedLongLongValue >= WBProductionCeilingBytes) {
        if (error && !*error) {
            *error = [self errorWithDescription:@"Reopened production policy exceeded the memory ceiling."];
        }
        return nil;
    }
    [phases addObject:reopened];
    return phases;
}

+ (NSRect)frameForIndex:(NSUInteger)index tabCount:(NSUInteger)tabCount isSplit:(BOOL)isSplit {
    if (isSplit && index < 2) {
        return NSMakeRect((CGFloat)index * 400.0, 0, 400, 600);
    }
    NSUInteger columns = 4;
    CGFloat width = 800.0 / (CGFloat)columns;
    CGFloat height = 600.0 / ceil((CGFloat)tabCount / (CGFloat)columns);
    return NSMakeRect((CGFloat)(index % columns) * width, (CGFloat)(index / columns) * height, width, height);
}

+ (NSString *)benchmarkDocumentForTabCount:(NSUInteger)tabCount {
    NSMutableString *cells = [NSMutableString string];
    for (NSUInteger index = 0; index < tabCount * 64; index += 1) {
        NSUInteger red = (index * 37) % 255;
        NSUInteger green = (index * 67) % 255;
        NSUInteger blue = (index * 97) % 255;
        [cells appendFormat:@"<div class='cell' style='background:linear-gradient(135deg,rgb(%lu,%lu,%lu),rgb(%lu,%lu,%lu))'></div>", (unsigned long)red, (unsigned long)green, (unsigned long)blue, (unsigned long)blue, (unsigned long)red, (unsigned long)green];
    }
    return [NSString stringWithFormat:@"<!doctype html><meta charset='utf-8'><title>Deterministic memory workload</title><style>body{margin:0;font:14px -apple-system;background:#111;color:white}.grid{display:grid;grid-template-columns:repeat(16,1fr);gap:2px;padding:8px}.cell{height:48px;border-radius:4px;box-shadow:inset 0 0 8px #fff8}</style><main><h1>Memory workload %lu</h1><div class='grid'>%@</div></main>", (unsigned long)tabCount, cells];
}

+ (void)releaseViews:(NSArray<WKWebView *> *)views weakViews:(NSMutableArray<WBWeakView *> *)weakViews {
    for (WKWebView *view in views) {
        WBWeakView *weakView = [[WBWeakView alloc] init];
        weakView.view = view;
        [weakViews addObject:weakView];
        view.navigationDelegate = nil;
        view.UIDelegate = nil;
        [view stopLoading];
        [view removeFromSuperviewWithoutNeedingDisplay];
    }
}

+ (BOOL)observeReleasedViews:(NSArray<WBWeakView *> *)weakViews timeout:(NSTimeInterval)timeout {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:timeout];
    while ([deadline timeIntervalSinceNow] > 0) {
        BOOL isReleased = YES;
        for (WBWeakView *weakView in weakViews) {
            if (weakView.view) {
                isReleased = NO;
                break;
            }
        }
        if (isReleased) {
            return YES;
        }
        [self runLoopFor:0.05];
    }
    return NO;
}

+ (NSUInteger)unreleasedViewCount:(NSArray<WBWeakView *> *)weakViews {
    NSUInteger count = 0;
    for (WBWeakView *weakView in weakViews) {
        if (weakView.view) {
            count += 1;
        }
    }
    return count;
}

+ (BOOL)waitForDelegate:(WBMemoryLoadDelegate *)delegate expectedCount:(NSUInteger)expectedCount timeout:(NSTimeInterval)timeout {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:timeout];
    while ([deadline timeIntervalSinceNow] > 0) {
        if (delegate.failedCount > 0) {
            return NO;
        }
        if (delegate.finishedCount >= expectedCount) {
            return YES;
        }
        [self runLoopFor:0.05];
    }
    return NO;
}

+ (void)runLoopFor:(NSTimeInterval)interval {
    NSDate *until = [NSDate dateWithTimeIntervalSinceNow:interval];
    while ([until timeIntervalSinceNow] > 0) {
        @autoreleasepool {
            [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode beforeDate:until];
        }
    }
}

+ (nullable NSDictionary *)samplePhaseMeasurementWithServiceBaseline:(NSSet<NSString *> *)serviceBaseline error:(NSError * _Nullable *)error {
    if ([self isSameNamedApplicationRunningWithCurrentProcessIdentifierExcluded:error]) {
        return nil;
    }
    NSArray<NSDictionary *> *initialMembers = [self stableCorrelatedMembersFromServiceBaseline:serviceBaseline error:error];
    if (!initialMembers) {
        return nil;
    }
    NSSet<NSString *> *initialMembership = [self membershipForMembers:initialMembers];
    NSMutableArray<NSDictionary *> *observations = [NSMutableArray arrayWithCapacity:WBObservationCount];
    NSDate *started = NSDate.date;
    for (NSUInteger index = 0; index < WBObservationCount; index += 1) {
        if ([self isSameNamedApplicationRunningWithCurrentProcessIdentifierExcluded:error]) {
            return nil;
        }
        NSArray<NSDictionary *> *before = [self correlatedMembersFromServiceBaseline:serviceBaseline error:error];
        if (!before || ![[self membershipForMembers:before] isEqualToSet:initialMembership]) {
            if (error && !*error) {
                *error = [self errorWithDescription:@"Process membership changed before footprint collection."];
            }
            return nil;
        }
        uint64_t total = [self footprintForMembers:before error:error];
        if (total == 0) {
            return nil;
        }
        if ([self isSameNamedApplicationRunningWithCurrentProcessIdentifierExcluded:error]) {
            return nil;
        }
        NSArray<NSDictionary *> *after = [self correlatedMembersFromServiceBaseline:serviceBaseline error:error];
        if (!after || ![[self membershipForMembers:after] isEqualToSet:initialMembership]) {
            if (error && !*error) {
                *error = [self errorWithDescription:@"Process membership changed during footprint collection."];
            }
            return nil;
        }
        NSTimeInterval elapsed = -[started timeIntervalSinceNow];
        [observations addObject:@{
            @"index": @(index),
            @"elapsedMilliseconds": @(llround(elapsed * 1000.0)),
            @"deduplicatedBytes": @(total),
            @"members": before
        }];
        if (index + 1 < WBObservationCount) {
            NSTimeInterval target = (NSTimeInterval)(index + 1) * WBSampleInterval;
            NSTimeInterval wait = target - (-[started timeIntervalSinceNow]);
            if (wait > 0) {
                [self runLoopFor:wait];
            }
        }
    }
    uint64_t peak = 0;
    for (NSDictionary *observation in observations) {
        peak = MAX(peak, [observation[@"deduplicatedBytes"] unsignedLongLongValue]);
    }
    return @{
        @"observations": observations,
        @"peakDeduplicatedBytes": @(peak)
    };
}

+ (nullable NSDictionary *)samplePhase:(NSString *)phase tabCount:(NSUInteger)tabCount repetition:(NSUInteger)repetition isSplit:(BOOL)isSplit isNativeReleaseObserved:(BOOL)isNativeReleaseObserved serviceBaseline:(NSSet<NSString *> *)serviceBaseline error:(NSError * _Nullable *)error {
    NSDictionary *measurement = [self samplePhaseMeasurementWithServiceBaseline:serviceBaseline error:error];
    if (!measurement) {
        return nil;
    }
    return [self receiptForPhase:phase tabCount:tabCount repetition:repetition isSplit:isSplit measurement:measurement isNativeReleaseObserved:isNativeReleaseObserved error:error];
}

+ (nullable NSDictionary *)receiptForPhase:(NSString *)phase tabCount:(NSUInteger)tabCount repetition:(NSUInteger)repetition isSplit:(BOOL)isSplit measurement:(NSDictionary *)measurement isNativeReleaseObserved:(BOOL)isNativeReleaseObserved error:(NSError * _Nullable *)error {
    NSDictionary *scenario = @{
        @"tabCount": @(tabCount),
        @"phase": phase,
        @"isSplit": @(isSplit),
        @"repetition": @(repetition)
    };
    NSDictionary *receipt = @{
        @"scenario": scenario,
        @"observations": measurement[@"observations"],
        @"peakDeduplicatedBytes": measurement[@"peakDeduplicatedBytes"],
        @"isNativeReleaseObserved": @(isNativeReleaseObserved),
        @"isProcessMembershipStable": @YES
    };
    if (![self isValidReceipt:receipt]) {
        if (error) {
            *error = [self errorWithDescription:@"Benchmark receipt did not meet the required scenario contract."];
        }
        return nil;
    }
    return receipt;
}

+ (nullable NSArray<NSDictionary *> *)stableCorrelatedMembersFromServiceBaseline:(NSSet<NSString *> *)serviceBaseline error:(NSError * _Nullable *)error {
    NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:5.0];
    while ([deadline timeIntervalSinceNow] > 0) {
        NSArray<NSDictionary *> *members = [self correlatedMembersFromServiceBaseline:serviceBaseline error:nil];
        if (members) {
            NSSet<NSString *> *membership = [self membershipForMembers:members];
            NSDate *settled = [NSDate dateWithTimeIntervalSinceNow:0.5];
            while ([settled timeIntervalSinceNow] > 0) {
                [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode beforeDate:settled];
            }
            NSArray<NSDictionary *> *after = [self snapshotsForPIDs:[members valueForKey:@"pid"] error:nil];
            if (after && [[self membershipForMembers:after] isEqualToSet:membership]) {
                return members;
            }
        }
    }
    if (error) {
        *error = [self errorWithDescription:@"Process membership did not settle before footprint collection."];
    }
    return nil;
}

+ (nullable NSArray<NSDictionary *> *)correlatedMembersFromServiceBaseline:(NSSet<NSString *> *)serviceBaseline error:(NSError * _Nullable *)error {
    NSMutableSet<NSNumber *> *pids = [NSMutableSet setWithObject:@(getpid())];
    for (NSNumber *pid in [self descendantPIDsForPID:getpid()]) {
        [pids addObject:pid];
    }
    NSSet<NSString *> *currentServices = [self webKitServiceIdentityStrings];
    for (NSString *identity in currentServices) {
        if (![serviceBaseline containsObject:identity]) {
            NSArray<NSString *> *parts = [identity componentsSeparatedByString:@":"];
            pid_t servicePID = parts.count == 2 ? parts.firstObject.intValue : 0;
            if (servicePID <= 0) {
                if (error) {
                    *error = [self errorWithDescription:@"A run-correlated WebKit service did not retain a valid process identity."];
                }
                return nil;
            }
            [pids addObject:@(servicePID)];
        }
    }
    NSArray<NSDictionary *> *members = [self snapshotsForPIDs:pids.allObjects error:error];
    if (!members || members.count < 2) {
        if (error && !*error) {
            *error = [self errorWithDescription:@"No WebKit service process could be correlated with this workload."];
        }
        return nil;
    }
    if (![self isValidRunWideServiceMembershipForMembers:members serviceBaseline:serviceBaseline currentServices:currentServices]) {
        if (error) {
            *error = [self errorWithDescription:@"A run-correlated WebKit service was omitted from the final receipt membership."];
        }
        return nil;
    }
    return members;
}

+ (NSArray<NSNumber *> *)descendantPIDsForPID:(pid_t)rootPID {
    NSMutableSet<NSNumber *> *found = [NSMutableSet set];
    NSMutableArray<NSNumber *> *pending = [NSMutableArray arrayWithObject:@(rootPID)];
    while (pending.count > 0) {
        pid_t parent = pending.lastObject.intValue;
        [pending removeLastObject];
        int capacity = 32;
        while (capacity <= 4096) {
            NSMutableData *buffer = [NSMutableData dataWithLength:(NSUInteger)capacity * sizeof(pid_t)];
            int count = proc_listchildpids(parent, buffer.mutableBytes, (int)buffer.length);
            if (count < 0) {
                break;
            }
            if (count < capacity) {
                pid_t *childPIDs = buffer.mutableBytes;
                for (int index = 0; index < count; index += 1) {
                    if (childPIDs[index] > 0 && ![found containsObject:@(childPIDs[index])]) {
                        NSNumber *child = @(childPIDs[index]);
                        [found addObject:child];
                        [pending addObject:child];
                    }
                }
                break;
            }
            capacity *= 2;
        }
    }
    return found.allObjects;
}

+ (BOOL)isSameNamedApplicationRunningWithCurrentProcessIdentifierExcluded:(NSError * _Nullable *)error {
    NSRunningApplication *currentApplication = NSRunningApplication.currentApplication;
    NSString *applicationName = currentApplication.localizedName;
    if (applicationName.length == 0) {
        if (error) {
            *error = [self errorWithDescription:@"The benchmark application has no localized name for service correlation."];
        }
        return YES;
    }
    pid_t currentPID = getpid();
    for (NSRunningApplication *application in NSWorkspace.sharedWorkspace.runningApplications) {
        if ([application.localizedName isEqualToString:applicationName] && application.processIdentifier != currentPID) {
            if (error) {
                *error = [self errorWithDescription:@"Another running application has the same localized name as the benchmark."];
            }
            return YES;
        }
    }
    return NO;
}

+ (NSSet<NSString *> *)webKitServiceIdentityStrings {
    NSString *applicationName = NSRunningApplication.currentApplication.localizedName;
    if (applicationName.length == 0) {
        return [NSSet set];
    }
    NSString *serviceNamePrefix = [applicationName stringByAppendingString:@" "];
    NSMutableSet<NSString *> *identities = [NSMutableSet set];
    for (NSRunningApplication *application in NSWorkspace.sharedWorkspace.runningApplications) {
        if (![application.bundleIdentifier hasPrefix:@"com.apple.WebKit."] || ![application.localizedName hasPrefix:serviceNamePrefix]) {
            continue;
        }
        NSDictionary *snapshot = [self snapshotForPID:application.processIdentifier error:nil];
        if (snapshot) {
            [identities addObject:[NSString stringWithFormat:@"%@:%@", snapshot[@"pid"], snapshot[@"start"]]];
        }
    }
    return identities;
}

+ (nullable NSArray<NSDictionary *> *)snapshotsForPIDs:(NSArray<NSNumber *> *)pids error:(NSError * _Nullable *)error {
    NSMutableArray<NSDictionary *> *snapshots = [NSMutableArray arrayWithCapacity:pids.count];
    for (NSNumber *pidNumber in pids) {
        NSDictionary *snapshot = [self snapshotForPID:pidNumber.intValue error:error];
        if (!snapshot) {
            return nil;
        }
        [snapshots addObject:snapshot];
    }
    return snapshots;
}

+ (nullable NSDictionary *)snapshotForPID:(pid_t)pid error:(NSError * _Nullable *)error {
    struct proc_bsdinfo process = {0};
    int processSize = proc_pidinfo(pid, PROC_PIDTBSDINFO, 0, &process, sizeof(process));
    uint64_t start = process.pbi_start_tvsec * 1000000ULL + process.pbi_start_tvusec;
    if (processSize != sizeof(process) || process.pbi_pid != (uint32_t)pid || start == 0) {
        if (error) {
            *error = [self errorWithDescription:@"proc_pidinfo could not establish a stable process identity."];
        }
        return nil;
    }
    struct rusage_info_v4 usage = {0};
    int usageResult = proc_pid_rusage(pid, RUSAGE_INFO_V4, (rusage_info_t *)&usage);
    if (usageResult != 0 || usage.ri_proc_start_abstime == 0 || usage.ri_phys_footprint == 0) {
        if (error) {
            *error = [self errorWithDescription:@"proc_pid_rusage could not establish complete process counters."];
        }
        return nil;
    }
    return @{
        @"pid": @(pid),
        @"start": @(start),
        @"physicalFootprintBytes": @(usage.ri_phys_footprint),
        @"residentBytes": @(usage.ri_resident_size),
        @"lifetimePeakBytes": @(usage.ri_lifetime_max_phys_footprint),
        @"diskReadBytes": @(usage.ri_diskio_bytesread),
        @"diskWriteBytes": @(usage.ri_diskio_byteswritten)
    };
}

+ (NSSet<NSString *> *)membershipForMembers:(NSArray<NSDictionary *> *)members {
    NSMutableSet<NSString *> *membership = [NSMutableSet setWithCapacity:members.count];
    for (NSDictionary *member in members) {
        [membership addObject:[NSString stringWithFormat:@"%@:%@", member[@"pid"], member[@"start"]]];
    }
    return membership;
}

+ (uint64_t)footprintForMembers:(NSArray<NSDictionary *> *)members error:(NSError * _Nullable *)error {
    NSString *name = [NSString stringWithFormat:@"Web-memory-footprint-%@.json", NSUUID.UUID.UUIDString];
    NSURL *URL = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:name]];
    NSTask *task = [[NSTask alloc] init];
    task.executableURL = [NSURL fileURLWithPath:@"/usr/bin/footprint"];
    NSMutableArray<NSString *> *arguments = [NSMutableArray arrayWithObjects:@"-j", URL.path, @"--noCategories", nil];
    for (NSDictionary *member in members) {
        [arguments addObject:@"--pid"];
        [arguments addObject:[member[@"pid"] stringValue]];
    }
    task.arguments = arguments;
    task.standardOutput = NSFileHandle.fileHandleWithNullDevice;
    task.standardError = NSFileHandle.fileHandleWithNullDevice;
    if (![task launchAndReturnError:error]) {
        return 0;
    }
    [task waitUntilExit];
    if (task.terminationStatus != 0) {
        if (error) {
            *error = [self errorWithDescription:@"/usr/bin/footprint exited unsuccessfully."];
        }
        return 0;
    }
    NSData *data = [NSData dataWithContentsOfURL:URL options:0 error:error];
    [[NSFileManager defaultManager] removeItemAtURL:URL error:nil];
    id object = data ? [NSJSONSerialization JSONObjectWithData:data options:0 error:error] : nil;
    if (![object isKindOfClass:NSDictionary.class]) {
        if (error && !*error) {
            *error = [self errorWithDescription:@"/usr/bin/footprint did not produce JSON."];
        }
        return 0;
    }
    NSNumber *total = [self validatedFootprintTotalFromOutput:object members:members];
    if (!total) {
        if (error) {
            *error = [self errorWithDescription:@"/usr/bin/footprint did not produce a complete deduplicated total."];
        }
        return 0;
    }
    return total.unsignedLongLongValue;
}

+ (nullable NSNumber *)validatedFootprintTotalFromOutput:(NSDictionary *)output members:(NSArray<NSDictionary *> *)members {
    if (![output isKindOfClass:NSDictionary.class]) {
        return nil;
    }
    NSString *unit = output[@"unit"];
    NSNumber *bytesPerUnit = output[@"bytes per unit"];
    NSArray *processes = output[@"processes"];
    NSArray *errors = output[@"errors"];
    NSArray *warnings = output[@"warnings"];
    NSNumber *total = output[@"total footprint"];
    if (![unit isKindOfClass:NSString.class] || ![unit isEqualToString:@"byte"] || ![self isPositiveIntegerValue:bytesPerUnit] || bytesPerUnit.unsignedLongLongValue != 1 || ![processes isKindOfClass:NSArray.class] || ![errors isKindOfClass:NSArray.class] || errors.count > 0 || ![warnings isKindOfClass:NSArray.class] || warnings.count > 0 || ![self isPositiveIntegerValue:total] || processes.count != members.count) {
        return nil;
    }
    NSMutableSet<NSNumber *> *expectedPIDs = [NSMutableSet setWithCapacity:members.count];
    for (NSDictionary *member in members) {
        NSNumber *pid = member[@"pid"];
        if (![self isPositiveIntegerValue:pid]) {
            return nil;
        }
        [expectedPIDs addObject:pid];
    }
    NSMutableSet<NSNumber *> *actualPIDs = [NSMutableSet setWithCapacity:processes.count];
    for (id process in processes) {
        if (![process isKindOfClass:NSDictionary.class]) {
            return nil;
        }
        NSNumber *pid = process[@"pid"];
        if (![self isPositiveIntegerValue:pid]) {
            return nil;
        }
        [actualPIDs addObject:pid];
    }
    return [expectedPIDs isEqualToSet:actualPIDs] ? total : nil;
}

+ (NSError *)errorWithDescription:(NSString *)description {
    return [NSError errorWithDomain:@"Web.Memory" code:1 userInfo:@{NSLocalizedDescriptionKey: description}];
}

+ (BOOL)isValidScenario:(NSDictionary *)scenario {
    if (![scenario isKindOfClass:NSDictionary.class] || scenario.count != 4) {
        return NO;
    }
    NSNumber *tabCount = scenario[@"tabCount"];
    NSString *phase = scenario[@"phase"];
    NSNumber *isSplit = scenario[@"isSplit"];
    NSNumber *repetition = scenario[@"repetition"];
    if (![self isPositiveIntegerValue:tabCount] || ![phase isKindOfClass:NSString.class] || ![self isExactBooleanValue:isSplit] || ![self isPositiveIntegerValue:repetition] || repetition.unsignedIntegerValue > WBRepetitionCount) {
        return NO;
    }
    if (isSplit.boolValue) {
        return tabCount.unsignedIntegerValue == 7 && [phase isEqualToString:@"split-stress"];
    }
    return (tabCount.unsignedIntegerValue == 7 || tabCount.unsignedIntegerValue == 12 || tabCount.unsignedIntegerValue == 20) && ([phase isEqualToString:@"loaded"] || [phase isEqualToString:@"one-live"] || [phase isEqualToString:@"reopened"]);
}

+ (BOOL)isValidBenchmarkDocument:(NSDictionary *)document {
    if (![document isKindOfClass:NSDictionary.class] || document.count != 7 || ![self isExactNonBooleanIntegerValue:document[@"version"] equalTo:1] || ![document[@"outcome"] isEqual:@"complete"] || ![self isExactNonBooleanIntegerValue:document[@"observationsPerPhase"] equalTo:WBObservationCount] || ![self isExactNonBooleanIntegerValue:document[@"sampleDurationMilliseconds"] equalTo:WBSampleDurationMilliseconds] || ![self isExactNonBooleanIntegerValue:document[@"repetitions"] equalTo:WBRepetitionCount] || ![self isExactNonBooleanIntegerValue:document[@"productionCeilingBytes"] equalTo:WBProductionCeilingBytes] || ![document[@"results"] isKindOfClass:NSArray.class]) {
        return NO;
    }
    NSMutableSet<NSString *> *expected = [NSMutableSet set];
    for (NSNumber *tabCount in @[@7, @12, @20]) {
        for (NSUInteger repetition = 1; repetition <= WBRepetitionCount; repetition += 1) {
            for (NSString *phase in @[@"loaded", @"one-live", @"reopened"]) {
                [expected addObject:[NSString stringWithFormat:@"%@:%@:%lu", tabCount, phase, (unsigned long)repetition]];
            }
        }
    }
    for (NSUInteger repetition = 1; repetition <= WBRepetitionCount; repetition += 1) {
        [expected addObject:[NSString stringWithFormat:@"7:split-stress:%lu", (unsigned long)repetition]];
    }
    NSArray<NSDictionary *> *results = document[@"results"];
    if (results.count != expected.count) {
        return NO;
    }
    NSMutableSet<NSString *> *actual = [NSMutableSet setWithCapacity:results.count];
    for (NSDictionary *receipt in results) {
        if (![self isValidReceipt:receipt]) {
            return NO;
        }
        NSDictionary *scenario = receipt[@"scenario"];
        [actual addObject:[NSString stringWithFormat:@"%@:%@:%@", scenario[@"tabCount"], scenario[@"phase"], scenario[@"repetition"]]];
    }
    return [actual isEqualToSet:expected];
}

+ (BOOL)isValidRunWideServiceMembershipForMembers:(NSArray<NSDictionary *> *)members serviceBaseline:(NSSet<NSString *> *)serviceBaseline currentServices:(NSSet<NSString *> *)currentServices {
    NSSet<NSString *> *membership = [self membershipForMembers:members];
    for (NSString *identity in currentServices) {
        if (![serviceBaseline containsObject:identity] && ![membership containsObject:identity]) {
            return NO;
        }
    }
    return YES;
}

+ (BOOL)isNumericValue:(id)value {
    return [value isKindOfClass:NSNumber.class] && CFGetTypeID((__bridge CFTypeRef)value) != CFBooleanGetTypeID();
}

+ (BOOL)isExactBooleanValue:(id)value {
    return [value isKindOfClass:NSNumber.class] && CFGetTypeID((__bridge CFTypeRef)value) == CFBooleanGetTypeID();
}

+ (BOOL)isExactNonBooleanIntegerValue:(id)value equalTo:(uint64_t)expectedValue {
    if (![self isNumericValue:value]) {
        return NO;
    }
    NSNumber *number = value;
    const char *type = number.objCType;
    BOOL isSigned = strcmp(type, @encode(char)) == 0 || strcmp(type, @encode(short)) == 0 || strcmp(type, @encode(int)) == 0 || strcmp(type, @encode(long)) == 0 || strcmp(type, @encode(long long)) == 0;
    BOOL isUnsigned = strcmp(type, @encode(unsigned char)) == 0 || strcmp(type, @encode(unsigned short)) == 0 || strcmp(type, @encode(unsigned int)) == 0 || strcmp(type, @encode(unsigned long)) == 0 || strcmp(type, @encode(unsigned long long)) == 0;
    if (isSigned) {
        return number.longLongValue >= 0 && (uint64_t)number.longLongValue == expectedValue;
    }
    return isUnsigned && number.unsignedLongLongValue == expectedValue;
}

+ (BOOL)isNonnegativeIntegerValue:(id)value {
    if (![self isNumericValue:value]) {
        return NO;
    }
    NSNumber *number = value;
    const char *type = number.objCType;
    BOOL isSigned = strcmp(type, @encode(char)) == 0 || strcmp(type, @encode(short)) == 0 || strcmp(type, @encode(int)) == 0 || strcmp(type, @encode(long)) == 0 || strcmp(type, @encode(long long)) == 0;
    BOOL isUnsigned = strcmp(type, @encode(unsigned char)) == 0 || strcmp(type, @encode(unsigned short)) == 0 || strcmp(type, @encode(unsigned int)) == 0 || strcmp(type, @encode(unsigned long)) == 0 || strcmp(type, @encode(unsigned long long)) == 0;
    return (isSigned && number.longLongValue >= 0) || isUnsigned;
}

+ (BOOL)isPositiveIntegerValue:(id)value {
    return [self isNonnegativeIntegerValue:value] && [value unsignedLongLongValue] > 0;
}

+ (BOOL)isValidMember:(NSDictionary *)member {
    if (![member isKindOfClass:NSDictionary.class] || member.count != 7) {
        return NO;
    }
    return [self isPositiveIntegerValue:member[@"pid"]] && [self isPositiveIntegerValue:member[@"start"]] && [self isNonnegativeIntegerValue:member[@"physicalFootprintBytes"]] && [self isNonnegativeIntegerValue:member[@"residentBytes"]] && [self isNonnegativeIntegerValue:member[@"lifetimePeakBytes"]] && [self isNonnegativeIntegerValue:member[@"diskReadBytes"]] && [self isNonnegativeIntegerValue:member[@"diskWriteBytes"]];
}

+ (BOOL)isValidReceipt:(NSDictionary *)receipt {
    if (![receipt isKindOfClass:NSDictionary.class] || receipt.count != 5) {
        return NO;
    }
    NSDictionary *scenario = receipt[@"scenario"];
    NSArray *observations = receipt[@"observations"];
    NSNumber *peak = receipt[@"peakDeduplicatedBytes"];
    NSNumber *isNativeReleaseObserved = receipt[@"isNativeReleaseObserved"];
    NSNumber *isProcessMembershipStable = receipt[@"isProcessMembershipStable"];
    if (![self isValidScenario:scenario] || ![observations isKindOfClass:NSArray.class] || observations.count != WBObservationCount || ![self isPositiveIntegerValue:peak] || ![self isExactBooleanValue:isNativeReleaseObserved] || ![self isExactBooleanValue:isProcessMembershipStable] || !isProcessMembershipStable.boolValue) {
        return NO;
    }
    uint64_t calculatedPeak = 0;
    NSSet<NSString *> *initialMembership;
    uint64_t lastElapsed = 0;
    for (NSUInteger index = 0; index < observations.count; index += 1) {
        NSDictionary *observation = observations[index];
        if (![observation isKindOfClass:NSDictionary.class] || observation.count != 4) {
            return NO;
        }
        NSNumber *observationIndex = observation[@"index"];
        NSNumber *elapsed = observation[@"elapsedMilliseconds"];
        NSNumber *bytes = observation[@"deduplicatedBytes"];
        NSArray *members = observation[@"members"];
        if (![self isNonnegativeIntegerValue:observationIndex] || observationIndex.unsignedIntegerValue != index || observationIndex.doubleValue != (double)index || ![self isNonnegativeIntegerValue:elapsed] || (index > 0 && elapsed.unsignedLongLongValue < lastElapsed) || ![self isPositiveIntegerValue:bytes] || ![members isKindOfClass:NSArray.class] || members.count < 2) {
            return NO;
        }
        for (NSDictionary *member in members) {
            if (![self isValidMember:member]) {
                return NO;
            }
        }
        NSSet<NSString *> *membership = [self membershipForMembers:members];
        if (membership.count != members.count || (initialMembership && ![initialMembership isEqualToSet:membership])) {
            return NO;
        }
        initialMembership = membership;
        lastElapsed = elapsed.unsignedLongLongValue;
        calculatedPeak = MAX(calculatedPeak, bytes.unsignedLongLongValue);
    }
    BOOL isProduction = [scenario[@"phase"] isEqualToString:@"one-live"] || [scenario[@"phase"] isEqualToString:@"reopened"];
    BOOL isReleaseProofRequired = isProduction || [scenario[@"phase"] isEqualToString:@"split-stress"];
    return lastElapsed >= WBSampleDurationMilliseconds && calculatedPeak == peak.unsignedLongLongValue && (!isReleaseProofRequired || isNativeReleaseObserved.boolValue) && (!isProduction || calculatedPeak < WBProductionCeilingBytes);
}

@end
