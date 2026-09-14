#import <XCTest/XCTest.h>
#import "WBAppController.h"
#import "WBMemoryBenchmark.h"
#import "WBSessionStore.h"
#import "WBTab.h"
#import <limits.h>
#import <float.h>
#import <math.h>

@interface WBMemoryBenchmark (Testing)
+ (nullable NSNumber *)validatedFootprintTotalFromOutput:(NSDictionary *)output members:(NSArray<NSDictionary *> *)members;
+ (BOOL)isValidBenchmarkDocument:(NSDictionary *)document;
+ (BOOL)isValidRunWideServiceMembershipForMembers:(NSArray<NSDictionary *> *)members serviceBaseline:(NSSet<NSString *> *)serviceBaseline currentServices:(NSSet<NSString *> *)currentServices;
@end

@interface WBSessionStore (Testing)
- (nullable NSData *)sessionDataAtURL:(NSURL *)URL;
@end

@interface WebTests : XCTestCase
@end

@implementation WebTests

- (void)testAddressOrSearchParsing {
    XCTAssertEqualObjects(WBURLForInput(@"example.com/path").absoluteString, @"https://example.com/path");
    XCTAssertEqualObjects(WBURLForInput(@"localhost:3000").absoluteString, @"https://localhost:3000");
    XCTAssertEqualObjects(WBURLForInput(@"example.com:8443/path").absoluteString, @"https://example.com:8443/path");
    XCTAssertEqualObjects(WBURLForInput(@"https://example.com/a").scheme, @"https");
    XCTAssertEqualObjects(WBURLForInput(@"words with spaces").host, @"www.google.com");
    XCTAssertNil(WBURLForInput(@"ftp://example.com"));
    XCTAssertEqualObjects(WBURLForPersistableURL([NSURL URLWithString:@"about:blank"], [NSURL URLWithString:@"https://source.example/"]).absoluteString, @"https://source.example/");
    XCTAssertEqualObjects(WBURLForPersistableURL([NSURL URLWithString:@"about:blank"], [NSURL URLWithString:@"about:blank"]).absoluteString, @"https://www.google.com/");
}

- (void)testSessionSavesAndLoadsOneHundredTabsWithOneHundredHistoryEntries {
    NSURL *directory = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString] isDirectory:YES];
    WBSessionStore *store = [[WBSessionStore alloc] initWithDirectoryURL:directory];
    NSMutableArray<WBTab *> *tabs = [NSMutableArray arrayWithCapacity:100];
    for (NSUInteger tabIndex = 0; tabIndex < 100; tabIndex += 1) {
        WBTab *tab = [[WBTab alloc] initWithURL:[NSURL URLWithString:[NSString stringWithFormat:@"https://example.com/%lu/0", (unsigned long)tabIndex]]];
        for (NSUInteger historyIndex = 1; historyIndex < 100; historyIndex += 1) {
            [tab recordURL:[NSURL URLWithString:[NSString stringWithFormat:@"https://example.com/%lu/%lu", (unsigned long)tabIndex, (unsigned long)historyIndex]]];
        }
        [tabs addObject:tab];
    }
    NSError *error = nil;
    XCTAssertTrue([store saveTabs:tabs selectedIdentifier:tabs.firstObject.identifier closedTabs:@[] error:&error]);
    XCTAssertNil(error);
    NSDictionary *session = [store loadSessionWithError:&error];
    XCTAssertNotNil(session);
    XCTAssertNil(error);
    NSArray<NSDictionary *> *savedTabs = session[@"tabs"];
    XCTAssertEqual(savedTabs.count, 100u);
    XCTAssertEqual([savedTabs.firstObject[@"history"] count], 100u);
    NSData *data = [store sessionDataAtURL:[directory URLByAppendingPathComponent:@"session.json"]];
    XCTAssertNotNil(data);
    XCTAssertLessThanOrEqual(data.length, 8u * 1024u * 1024u);
    [[NSFileManager defaultManager] removeItemAtURL:directory error:nil];
}

- (void)testSessionSaveRejectsSerializedDataLargerThanTheLoadLimit {
    NSURL *directory = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString] isDirectory:YES];
    WBSessionStore *store = [[WBSessionStore alloc] initWithDirectoryURL:directory];
    NSString *padding = [@"" stringByPaddingToLength:850 withString:@"a" startingAtIndex:0];
    NSMutableArray<WBTab *> *tabs = [NSMutableArray arrayWithCapacity:100];
    for (NSUInteger tabIndex = 0; tabIndex < 100; tabIndex += 1) {
        WBTab *tab = [[WBTab alloc] initWithURL:[NSURL URLWithString:[NSString stringWithFormat:@"https://example.com/%lu/0/%@", (unsigned long)tabIndex, padding]]];
        for (NSUInteger historyIndex = 1; historyIndex < 100; historyIndex += 1) {
            [tab recordURL:[NSURL URLWithString:[NSString stringWithFormat:@"https://example.com/%lu/%lu/%@", (unsigned long)tabIndex, (unsigned long)historyIndex, padding]]];
        }
        [tabs addObject:tab];
    }
    NSError *error = nil;
    XCTAssertFalse([store saveTabs:tabs selectedIdentifier:tabs.firstObject.identifier closedTabs:@[] error:&error]);
    XCTAssertEqualObjects(error.domain, @"Web.Session");
    XCTAssertEqual(error.code, 3);
    XCTAssertFalse([[NSFileManager defaultManager] fileExistsAtPath:[directory URLByAppendingPathComponent:@"session.json"].path]);
    [[NSFileManager defaultManager] removeItemAtURL:directory error:nil];
}

- (void)testCredentialBearingURLsAreNeverPersisted {
    NSURL *fallbackURL = [NSURL URLWithString:@"https://safe.example/"];
    NSURL *credentialURL = [NSURL URLWithString:@"https://browser-user:browser-password@example.com/"];
    XCTAssertNil(WBURLForInput(@"https://browser-user:browser-password@example.com/"));
    XCTAssertNil(WBURLForInput(@"https://browser-user@example.com/"));
    XCTAssertNil(WBURLForInput(@"https://:browser-password@example.com/"));
    XCTAssertEqualObjects(WBURLForPersistableURL(credentialURL, fallbackURL).absoluteString, fallbackURL.absoluteString);

    WBTab *tab = [[WBTab alloc] initWithURL:fallbackURL];
    [tab recordURL:credentialURL];
    XCTAssertEqual(tab.history.count, 1u);
    XCTAssertEqualObjects(tab.URL.absoluteString, fallbackURL.absoluteString);
    NSData *data = [NSJSONSerialization dataWithJSONObject:tab.dictionaryRepresentation options:0 error:nil];
    NSString *serialized = [[NSString alloc] initWithData:data encoding:NSUTF8StringEncoding];
    XCTAssertFalse([serialized containsString:@"browser-user"]);
    XCTAssertFalse([serialized containsString:@"browser-password"]);

    WBTab *credentialTab = [[WBTab alloc] initWithURL:credentialURL];
    XCTAssertEqualObjects(credentialTab.URL.absoluteString, @"https://www.google.com/");
    XCTAssertFalse([credentialTab.dictionaryRepresentation.description containsString:@"browser-user"]);
}

- (void)testRuntimeURLLengthLimitRejectsInputAndPreservesThePriorURL {
    NSString *prefix = @"https://example.com/";
    NSString *maximumLengthURLString = [prefix stringByAppendingString:[@"" stringByPaddingToLength:8192 - prefix.length withString:@"a" startingAtIndex:0]];
    NSString *overlongURLString = [maximumLengthURLString stringByAppendingString:@"a"];
    XCTAssertEqual(maximumLengthURLString.length, 8192u);
    XCTAssertEqual(overlongURLString.length, 8193u);
    XCTAssertEqualObjects(WBURLForInput(maximumLengthURLString).absoluteString, maximumLengthURLString);
    XCTAssertNil(WBURLForInput(overlongURLString));
    XCTAssertNil(WBURLForInput([@"" stringByPaddingToLength:8192 withString:@"a" startingAtIndex:0]));

    NSURL *fallbackURL = [NSURL URLWithString:@"https://safe.example/"];
    NSURL *overlongURL = [NSURL URLWithString:overlongURLString];
    XCTAssertNotNil(overlongURL);
    XCTAssertEqualObjects(WBURLForPersistableURL(overlongURL, fallbackURL).absoluteString, fallbackURL.absoluteString);
    WBTab *tab = [[WBTab alloc] initWithURL:fallbackURL];
    [tab recordURL:overlongURL];
    XCTAssertEqual(tab.history.count, 1u);
    XCTAssertEqualObjects(tab.URL.absoluteString, fallbackURL.absoluteString);
}

- (void)testRuntimeTitleAndScrollValuesRemainPersistable {
    WBTab *tab = [[WBTab alloc] initWithURL:[NSURL URLWithString:@"https://example.com/"]];
    NSString *prefix = [@"" stringByPaddingToLength:1023 withString:@"a" startingAtIndex:0];
    tab.title = [prefix stringByAppendingString:@"😀z"];
    XCTAssertEqualObjects(tab.title, prefix);
    XCTAssertNotNil([NSJSONSerialization dataWithJSONObject:tab.dictionaryRepresentation options:0 error:nil]);

    tab.scrollX = DBL_MAX;
    tab.scrollY = -DBL_MAX;
    XCTAssertEqual(tab.scrollX, 10000000.0);
    XCTAssertEqual(tab.scrollY, -10000000.0);
    tab.scrollX = NAN;
    tab.scrollY = INFINITY;
    XCTAssertEqual(tab.scrollX, 0.0);
    XCTAssertEqual(tab.scrollY, 0.0);
    XCTAssertNotNil([[WBTab alloc] initWithDictionary:tab.dictionaryRepresentation]);
}

- (void)testSessionValidationAndBackupRecovery {
    NSURL *directory = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString] isDirectory:YES];
    WBSessionStore *store = [[WBSessionStore alloc] initWithDirectoryURL:directory];
    WBTab *tab = [[WBTab alloc] initWithURL:[NSURL URLWithString:@"https://example.com/a"]];
    XCTAssertTrue([store saveTabs:@[tab] selectedIdentifier:tab.identifier closedTabs:@[] error:nil]);
    [tab recordURL:[NSURL URLWithString:@"https://example.com/b"]];
    XCTAssertTrue([store saveTabs:@[tab] selectedIdentifier:tab.identifier closedTabs:@[] error:nil]);
    NSURL *current = [directory URLByAppendingPathComponent:@"session.json"];
    XCTAssertTrue([[@"{" dataUsingEncoding:NSUTF8StringEncoding] writeToURL:current options:NSDataWritingAtomic error:nil]);
    NSDictionary *recoveredBackup = [store loadSessionWithError:nil];
    XCTAssertEqual([recoveredBackup[@"generation"] integerValue], 1);
    [tab recordURL:[NSURL URLWithString:@"https://example.com/c"]];
    XCTAssertTrue([store saveTabs:@[tab] selectedIdentifier:tab.identifier closedTabs:@[] error:nil]);
    NSDictionary *recoveredCurrent = [store loadSessionWithError:nil];
    NSDictionary *recoveredTab = ((NSArray<NSDictionary *> *)recoveredCurrent[@"tabs"]).firstObject;
    XCTAssertEqualObjects(recoveredTab[@"url"], @"https://example.com/c");
    NSMutableDictionary *invalid = [recoveredCurrent mutableCopy];
    invalid[@"unexpected"] = @YES;
    XCTAssertNil([store validatedSessionFromDictionary:invalid error:nil]);
    [[NSFileManager defaultManager] removeItemAtURL:directory error:nil];
}

- (void)testSessionRejectsMalformedNumbersBeforeSelectingABackup {
    NSURL *directory = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString] isDirectory:YES];
    WBSessionStore *store = [[WBSessionStore alloc] initWithDirectoryURL:directory];
    WBTab *tab = [[WBTab alloc] initWithURL:[NSURL URLWithString:@"https://example.com/"]];
    NSDictionary *valid = @{@"version": @1, @"generation": @4, @"selectedIdentifier": tab.identifier.UUIDString, @"tabs": @[tab.dictionaryRepresentation], @"closedTabs": @[]};
    NSMutableDictionary *fractionalVersion = [valid mutableCopy];
    fractionalVersion[@"version"] = @1.5;
    NSMutableDictionary *booleanVersion = [valid mutableCopy];
    booleanVersion[@"version"] = @YES;
    NSMutableDictionary *fractionalGeneration = [valid mutableCopy];
    fractionalGeneration[@"generation"] = @4.5;
    NSMutableDictionary *negativeGeneration = [valid mutableCopy];
    negativeGeneration[@"generation"] = @-1;
    NSMutableDictionary *booleanGeneration = [valid mutableCopy];
    booleanGeneration[@"generation"] = @YES;
    NSMutableDictionary *infiniteGeneration = [valid mutableCopy];
    infiniteGeneration[@"generation"] = @(INFINITY);
    NSMutableDictionary *zeroGeneration = [valid mutableCopy];
    zeroGeneration[@"generation"] = @0;
    XCTAssertNil([store validatedSessionFromDictionary:fractionalVersion error:nil]);
    XCTAssertNil([store validatedSessionFromDictionary:booleanVersion error:nil]);
    XCTAssertNil([store validatedSessionFromDictionary:fractionalGeneration error:nil]);
    XCTAssertNil([store validatedSessionFromDictionary:negativeGeneration error:nil]);
    XCTAssertNil([store validatedSessionFromDictionary:booleanGeneration error:nil]);
    XCTAssertNil([store validatedSessionFromDictionary:infiniteGeneration error:nil]);
    XCTAssertNotNil([store validatedSessionFromDictionary:zeroGeneration error:nil]);

    XCTAssertTrue([[NSFileManager defaultManager] createDirectoryAtURL:directory withIntermediateDirectories:YES attributes:nil error:nil]);
    NSURL *backup = [directory URLByAppendingPathComponent:@"session.backup.json"];
    NSURL *current = [directory URLByAppendingPathComponent:@"session.json"];
    NSData *backupData = [NSJSONSerialization dataWithJSONObject:valid options:0 error:nil];
    XCTAssertTrue([backupData writeToURL:backup options:NSDataWritingAtomic error:nil]);
    NSData *malformedCurrentData = [NSJSONSerialization dataWithJSONObject:fractionalGeneration options:0 error:nil];
    XCTAssertTrue([malformedCurrentData writeToURL:current options:NSDataWritingAtomic error:nil]);
    NSDictionary *recovered = [store loadSessionWithError:nil];
    XCTAssertEqualObjects(recovered[@"generation"], @4);
    XCTAssertEqualObjects(((NSArray<NSDictionary *> *)recovered[@"tabs"]).firstObject[@"url"], @"https://example.com/");
    [[NSFileManager defaultManager] removeItemAtURL:directory error:nil];
}

- (void)testWBTabRejectsMalformedNestedRecords {
    WBTab *tab = [[WBTab alloc] initWithURL:[NSURL URLWithString:@"https://example.com/"]];
    NSDictionary *valid = tab.dictionaryRepresentation;
    NSMutableDictionary *fractionalHistoryIndex = [valid mutableCopy];
    fractionalHistoryIndex[@"historyIndex"] = @0.0;
    NSMutableDictionary *booleanHistoryIndex = [valid mutableCopy];
    booleanHistoryIndex[@"historyIndex"] = @YES;
    NSMutableDictionary *negativeHistoryIndex = [valid mutableCopy];
    negativeHistoryIndex[@"historyIndex"] = @-1;
    NSMutableDictionary *nanHistoryIndex = [valid mutableCopy];
    nanHistoryIndex[@"historyIndex"] = @(NAN);
    NSMutableDictionary *infiniteHistoryIndex = [valid mutableCopy];
    infiniteHistoryIndex[@"historyIndex"] = @(INFINITY);
    NSMutableDictionary *outOfBoundsHistoryIndex = [valid mutableCopy];
    outOfBoundsHistoryIndex[@"historyIndex"] = @1;
    NSMutableDictionary *oversizedHistoryIndex = [valid mutableCopy];
    oversizedHistoryIndex[@"historyIndex"] = [NSNumber numberWithUnsignedLongLong:ULLONG_MAX];
    NSMutableDictionary *numericPinned = [valid mutableCopy];
    numericPinned[@"isPinned"] = @1;
    NSMutableDictionary *booleanScroll = [valid mutableCopy];
    booleanScroll[@"scrollX"] = @YES;
    NSMutableDictionary *nanScroll = [valid mutableCopy];
    nanScroll[@"scrollY"] = @(NAN);
    NSMutableDictionary *infiniteScroll = [valid mutableCopy];
    infiniteScroll[@"scrollY"] = @(INFINITY);
    NSMutableDictionary *oversizedScroll = [valid mutableCopy];
    oversizedScroll[@"scrollX"] = @(DBL_MAX);
    NSMutableDictionary *malformedScroll = [valid mutableCopy];
    malformedScroll[@"scrollX"] = @"1.5";
    NSMutableDictionary *fractionalScroll = [valid mutableCopy];
    fractionalScroll[@"scrollX"] = @1.5;
    fractionalScroll[@"scrollY"] = @-2.25;
    NSMutableDictionary *extraKey = [valid mutableCopy];
    extraKey[@"unexpected"] = @YES;
    NSMutableDictionary *oversizedTitle = [valid mutableCopy];
    oversizedTitle[@"title"] = [@"" stringByPaddingToLength:10000 withString:@"a" startingAtIndex:0];
    NSString *oversizedURLString = [@"https://example.com/" stringByAppendingString:[@"" stringByPaddingToLength:10000 withString:@"a" startingAtIndex:0]];
    NSMutableDictionary *oversizedURL = [valid mutableCopy];
    oversizedURL[@"url"] = oversizedURLString;
    oversizedURL[@"history"] = @[oversizedURLString];
    NSMutableDictionary *inconsistentCurrentURL = [valid mutableCopy];
    inconsistentCurrentURL[@"history"] = @[@"https://history.example/"];

    XCTAssertNotNil([[WBTab alloc] initWithDictionary:valid]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:fractionalHistoryIndex]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:booleanHistoryIndex]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:negativeHistoryIndex]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:nanHistoryIndex]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:infiniteHistoryIndex]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:outOfBoundsHistoryIndex]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:oversizedHistoryIndex]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:numericPinned]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:booleanScroll]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:nanScroll]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:infiniteScroll]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:oversizedScroll]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:malformedScroll]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:extraKey]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:oversizedTitle]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:oversizedURL]);
    XCTAssertNil([[WBTab alloc] initWithDictionary:inconsistentCurrentURL]);
    WBTab *restoredFractionalScroll = [[WBTab alloc] initWithDictionary:fractionalScroll];
    XCTAssertNotNil(restoredFractionalScroll);
    XCTAssertEqualWithAccuracy(restoredFractionalScroll.scrollX, 1.5, DBL_EPSILON);
    XCTAssertEqualWithAccuracy(restoredFractionalScroll.scrollY, -2.25, DBL_EPSILON);
}

- (void)testSessionRejectsMalformedNestedRecordNumbers {
    NSURL *directory = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString] isDirectory:YES];
    WBSessionStore *store = [[WBSessionStore alloc] initWithDirectoryURL:directory];
    WBTab *tab = [[WBTab alloc] initWithURL:[NSURL URLWithString:@"https://example.com/"]];
    NSDictionary *valid = @{@"version": @1, @"generation": @1, @"selectedIdentifier": tab.identifier.UUIDString, @"tabs": @[tab.dictionaryRepresentation], @"closedTabs": @[]};
    NSMutableDictionary *invalidTab = [tab.dictionaryRepresentation mutableCopy];
    invalidTab[@"isPinned"] = @1;
    NSMutableDictionary *invalidDocument = [valid mutableCopy];
    invalidDocument[@"tabs"] = @[invalidTab];
    NSMutableDictionary *invalidClosedTab = [tab.dictionaryRepresentation mutableCopy];
    invalidClosedTab[@"scrollX"] = @YES;
    NSMutableDictionary *invalidClosedDocument = [valid mutableCopy];
    invalidClosedDocument[@"closedTabs"] = @[invalidClosedTab];

    XCTAssertNotNil([store validatedSessionFromDictionary:valid error:nil]);
    XCTAssertNil([store validatedSessionFromDictionary:invalidDocument error:nil]);
    XCTAssertNil([store validatedSessionFromDictionary:invalidClosedDocument error:nil]);
}

- (void)testSessionRejectsGenerationAtOrBeyondIncrementBoundaryAndRecovers {
    NSURL *directory = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString] isDirectory:YES];
    WBSessionStore *store = [[WBSessionStore alloc] initWithDirectoryURL:directory];
    WBTab *tab = [[WBTab alloc] initWithURL:[NSURL URLWithString:@"https://example.com/"]];
    NSDictionary *valid = @{@"version": @1, @"generation": @4, @"selectedIdentifier": tab.identifier.UUIDString, @"tabs": @[tab.dictionaryRepresentation], @"closedTabs": @[]};
    NSArray<NSNumber *> *invalidGenerations = @[@(NSIntegerMax), [NSNumber numberWithUnsignedLongLong:ULLONG_MAX]];
    for (NSNumber *generation in invalidGenerations) {
        NSMutableDictionary *invalid = [valid mutableCopy];
        invalid[@"generation"] = generation;
        XCTAssertNil([store validatedSessionFromDictionary:invalid error:nil]);
    }

    XCTAssertTrue([[NSFileManager defaultManager] createDirectoryAtURL:directory withIntermediateDirectories:YES attributes:nil error:nil]);
    NSURL *backup = [directory URLByAppendingPathComponent:@"session.backup.json"];
    NSURL *current = [directory URLByAppendingPathComponent:@"session.json"];
    NSData *validData = [NSJSONSerialization dataWithJSONObject:valid options:0 error:nil];
    XCTAssertTrue([validData writeToURL:backup options:NSDataWritingAtomic error:nil]);
    for (NSNumber *generation in invalidGenerations) {
        NSMutableDictionary *invalid = [valid mutableCopy];
        invalid[@"generation"] = generation;
        NSData *invalidData = [NSJSONSerialization dataWithJSONObject:invalid options:0 error:nil];
        XCTAssertTrue([invalidData writeToURL:current options:NSDataWritingAtomic error:nil]);
        XCTAssertTrue([store saveTabs:@[tab] selectedIdentifier:tab.identifier closedTabs:@[] error:nil]);
        NSDictionary *recovered = [store loadSessionWithError:nil];
        XCTAssertEqualObjects(recovered[@"generation"], @5);
        XCTAssertEqualObjects(((NSArray<NSDictionary *> *)recovered[@"tabs"]).firstObject[@"url"], @"https://example.com/");
    }
    [[NSFileManager defaultManager] removeItemAtURL:directory error:nil];
}

- (void)testSessionRejectsOversizedFilesBeforeParsing {
    NSURL *directory = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString] isDirectory:YES];
    WBSessionStore *store = [[WBSessionStore alloc] initWithDirectoryURL:directory];
    XCTAssertTrue([[NSFileManager defaultManager] createDirectoryAtURL:directory withIntermediateDirectories:YES attributes:nil error:nil]);
    NSURL *current = [directory URLByAppendingPathComponent:@"session.json"];
    NSData *oversizedData = [[@"{" stringByPaddingToLength:(8 * 1024 * 1024) + 1 withString:@" " startingAtIndex:0] dataUsingEncoding:NSUTF8StringEncoding];
    XCTAssertTrue([oversizedData writeToURL:current options:NSDataWritingAtomic error:nil]);
    XCTAssertNil([store sessionDataAtURL:current]);
    XCTAssertNil([store loadSessionWithError:nil]);
    [[NSFileManager defaultManager] removeItemAtURL:directory error:nil];
}

- (void)testSessionRejectsTooManyOpenTabs {
    NSURL *directory = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString] isDirectory:YES];
    WBSessionStore *store = [[WBSessionStore alloc] initWithDirectoryURL:directory];
    NSMutableArray<NSDictionary *> *tabDictionaries = [NSMutableArray array];
    for (NSUInteger index = 0; index < 101; index += 1) {
        WBTab *tab = [[WBTab alloc] initWithURL:[NSURL URLWithString:[NSString stringWithFormat:@"https://example.com/%lu", (unsigned long)index]]];
        [tabDictionaries addObject:tab.dictionaryRepresentation];
    }
    NSDictionary *validDocument = @{@"version": @1, @"generation": @1, @"selectedIdentifier": tabDictionaries.firstObject[@"identifier"], @"tabs": [tabDictionaries subarrayWithRange:NSMakeRange(0, 100)], @"closedTabs": @[]};
    NSDictionary *oversizedDocument = @{@"version": @1, @"generation": @1, @"selectedIdentifier": tabDictionaries.firstObject[@"identifier"], @"tabs": tabDictionaries, @"closedTabs": @[]};
    XCTAssertNotNil([store validatedSessionFromDictionary:validDocument error:nil]);
    XCTAssertNil([store validatedSessionFromDictionary:oversizedDocument error:nil]);
}

- (void)testRuntimeTitlesAreTruncatedBeforePersistence {
    WBTab *tab = [[WBTab alloc] initWithURL:[NSURL URLWithString:@"https://example.com/"]];
    tab.title = [@"" stringByPaddingToLength:1025 withString:@"a" startingAtIndex:0];
    XCTAssertEqual(tab.title.length, 1024u);
    XCTAssertEqualObjects(tab.dictionaryRepresentation[@"title"], tab.title);
    NSMutableDictionary *restored = [tab.dictionaryRepresentation mutableCopy];
    restored[@"title"] = [@"" stringByPaddingToLength:1025 withString:@"a" startingAtIndex:0];
    XCTAssertNil([[WBTab alloc] initWithDictionary:restored]);
}

- (void)testHistoryAndClosedTabBounds {
    WBTab *tab = [[WBTab alloc] initWithURL:[NSURL URLWithString:@"https://example.com/"]];
    for (NSUInteger index = 0; index < 110; index += 1) {
        [tab recordURL:[NSURL URLWithString:[NSString stringWithFormat:@"https://example.com/%lu", (unsigned long)index]]];
    }
    XCTAssertEqual(tab.history.count, 100);
    NSURL *directory = [NSURL fileURLWithPath:[NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString] isDirectory:YES];
    WBSessionStore *store = [[WBSessionStore alloc] initWithDirectoryURL:directory];
    NSMutableArray<NSDictionary *> *closedTabs = [NSMutableArray array];
    for (NSUInteger index = 0; index < 26; index += 1) {
        WBTab *closed = [[WBTab alloc] initWithURL:[NSURL URLWithString:[NSString stringWithFormat:@"https://closed.example/%lu", (unsigned long)index]]];
        [closedTabs addObject:closed.dictionaryRepresentation];
    }
    NSDictionary *document = @{@"version": @1, @"generation": @1, @"selectedIdentifier": tab.identifier.UUIDString, @"tabs": @[tab.dictionaryRepresentation], @"closedTabs": closedTabs};
    XCTAssertNil([store validatedSessionFromDictionary:document error:nil]);
}

- (void)testHistoryRollbackRestoresThePriorCurrentURL {
    WBTab *tab = [[WBTab alloc] initWithURL:[NSURL URLWithString:@"https://example.com/a"]];
    [tab recordURL:[NSURL URLWithString:@"https://example.com/b"]];
    [tab recordURL:[NSURL URLWithString:@"https://example.com/c"]];
    XCTAssertEqualObjects([tab URLAtHistoryOffset:-1].absoluteString, @"https://example.com/b");
    [tab commitHistoryNavigationToURL:[NSURL URLWithString:@"https://redirect.example/d"]];
    [tab rollbackHistoryNavigation];
    XCTAssertEqual(tab.historyIndex, 2);
    XCTAssertEqualObjects(tab.URL.absoluteString, @"https://example.com/c");
    XCTAssertEqualObjects(tab.history[1].absoluteString, @"https://example.com/b");
}

- (void)testCommittedHistoryNavigationDoesNotRollback {
    WBTab *tab = [[WBTab alloc] initWithURL:[NSURL URLWithString:@"https://example.com/a"]];
    [tab recordURL:[NSURL URLWithString:@"https://example.com/b"]];
    [tab URLAtHistoryOffset:-1];
    [tab commitHistoryNavigationToURL:[NSURL URLWithString:@"https://redirect.example/c"]];
    [tab completeHistoryNavigation];
    [tab rollbackHistoryNavigation];
    XCTAssertEqual(tab.historyIndex, 0);
    XCTAssertEqualObjects(tab.URL.absoluteString, @"https://redirect.example/c");
}

- (void)testOneLiveViewOwnershipStartsEmpty {
    WBAppController *controller = [[WBAppController alloc] init];
    XCTAssertEqual(controller.liveViewCount, 0u);
}

- (void)testFootprintOutputRejectsNonIntegerTotalsAndUnits {
    NSArray<NSDictionary *> *members = @[@{@"pid": @101}, @{@"pid": @202}];
    NSDictionary *validOutput = @{@"unit": @"byte", @"bytes per unit": @1, @"processes": @[@{@"pid": @101}, @{@"pid": @202}], @"errors": @[], @"warnings": @[], @"total footprint": @4096};
    NSMutableDictionary *booleanUnit = [validOutput mutableCopy];
    booleanUnit[@"bytes per unit"] = @YES;
    NSMutableDictionary *fractionalUnit = [validOutput mutableCopy];
    fractionalUnit[@"bytes per unit"] = @1.5;
    NSMutableDictionary *wrongUnit = [validOutput mutableCopy];
    wrongUnit[@"bytes per unit"] = @2;
    NSMutableDictionary *booleanTotal = [validOutput mutableCopy];
    booleanTotal[@"total footprint"] = @YES;
    NSMutableDictionary *fractionalTotal = [validOutput mutableCopy];
    fractionalTotal[@"total footprint"] = @4096.5;
    NSMutableDictionary *zeroTotal = [validOutput mutableCopy];
    zeroTotal[@"total footprint"] = @0;
    XCTAssertEqualObjects([WBMemoryBenchmark validatedFootprintTotalFromOutput:validOutput members:members], @4096);
    XCTAssertNil([WBMemoryBenchmark validatedFootprintTotalFromOutput:booleanUnit members:members]);
    XCTAssertNil([WBMemoryBenchmark validatedFootprintTotalFromOutput:fractionalUnit members:members]);
    XCTAssertNil([WBMemoryBenchmark validatedFootprintTotalFromOutput:wrongUnit members:members]);
    XCTAssertNil([WBMemoryBenchmark validatedFootprintTotalFromOutput:booleanTotal members:members]);
    XCTAssertNil([WBMemoryBenchmark validatedFootprintTotalFromOutput:fractionalTotal members:members]);
    XCTAssertNil([WBMemoryBenchmark validatedFootprintTotalFromOutput:zeroTotal members:members]);
}

- (void)testBenchmarkDocumentRequiresExactEnvelopeIntegerTypes {
    NSDictionary *firstMember = @{@"pid": @1, @"start": @10, @"physicalFootprintBytes": @20, @"residentBytes": @30, @"lifetimePeakBytes": @40, @"diskReadBytes": @50, @"diskWriteBytes": @60};
    NSDictionary *secondMember = @{@"pid": @2, @"start": @20, @"physicalFootprintBytes": @21, @"residentBytes": @31, @"lifetimePeakBytes": @41, @"diskReadBytes": @51, @"diskWriteBytes": @61};
    NSArray *members = @[firstMember, secondMember];
    NSMutableArray<NSDictionary *> *observations = [NSMutableArray array];
    for (NSUInteger index = 0; index < 11; index += 1) {
        [observations addObject:@{@"index": @(index), @"elapsedMilliseconds": @(index * 1000), @"deduplicatedBytes": @(100 + index), @"members": members}];
    }
    NSMutableArray<NSDictionary *> *results = [NSMutableArray array];
    for (NSNumber *tabCount in @[@7, @12, @20]) {
        for (NSUInteger repetition = 1; repetition <= 3; repetition += 1) {
            for (NSString *phase in @[@"loaded", @"one-live", @"reopened"]) {
                BOOL isNativeReleaseObserved = ![phase isEqualToString:@"loaded"];
                NSDictionary *scenario = @{@"tabCount": tabCount, @"phase": phase, @"isSplit": @NO, @"repetition": @(repetition)};
                [results addObject:@{@"scenario": scenario, @"observations": observations, @"peakDeduplicatedBytes": @110, @"isNativeReleaseObserved": @(isNativeReleaseObserved), @"isProcessMembershipStable": @YES}];
            }
        }
    }
    for (NSUInteger repetition = 1; repetition <= 3; repetition += 1) {
        NSDictionary *scenario = @{@"tabCount": @7, @"phase": @"split-stress", @"isSplit": @YES, @"repetition": @(repetition)};
        [results addObject:@{@"scenario": scenario, @"observations": observations, @"peakDeduplicatedBytes": @110, @"isNativeReleaseObserved": @YES, @"isProcessMembershipStable": @YES}];
    }
    NSDictionary *document = @{@"version": @1, @"outcome": @"complete", @"observationsPerPhase": @11, @"sampleDurationMilliseconds": @10000, @"repetitions": @3, @"productionCeilingBytes": @2000000000, @"results": results};
    NSDictionary *floatingValues = @{@"version": @1.0, @"observationsPerPhase": @11.0, @"sampleDurationMilliseconds": @10000.0, @"repetitions": @3.0, @"productionCeilingBytes": @2000000000.0};
    NSDictionary *incorrectValues = @{@"version": @2, @"observationsPerPhase": @12, @"sampleDurationMilliseconds": @9999, @"repetitions": @4, @"productionCeilingBytes": @1999999999};

    XCTAssertTrue([WBMemoryBenchmark isValidBenchmarkDocument:document]);
    NSSet<NSString *> *serviceBaseline = [NSSet setWithObject:@"1:10"];
    NSSet<NSString *> *currentServices = [NSSet setWithObjects:@"1:10", @"2:20", nil];
    XCTAssertTrue([WBMemoryBenchmark isValidRunWideServiceMembershipForMembers:members serviceBaseline:serviceBaseline currentServices:currentServices]);
    XCTAssertFalse([WBMemoryBenchmark isValidRunWideServiceMembershipForMembers:@[firstMember] serviceBaseline:serviceBaseline currentServices:currentServices]);
    for (NSString *key in floatingValues) {
        NSMutableDictionary *invalid = [document mutableCopy];
        invalid[key] = floatingValues[key];
        XCTAssertFalse([WBMemoryBenchmark isValidBenchmarkDocument:invalid]);
    }
    for (NSString *key in incorrectValues) {
        NSMutableDictionary *invalid = [document mutableCopy];
        invalid[key] = incorrectValues[key];
        XCTAssertFalse([WBMemoryBenchmark isValidBenchmarkDocument:invalid]);
    }
    NSMutableDictionary *booleanVersion = [document mutableCopy];
    booleanVersion[@"version"] = @YES;
    XCTAssertFalse([WBMemoryBenchmark isValidBenchmarkDocument:booleanVersion]);
}

- (void)testBenchmarkReceiptAndScenarioValidation {
    NSDictionary *scenario = @{@"tabCount": @7, @"phase": @"one-live", @"isSplit": @NO, @"repetition": @1};
    NSDictionary *firstMember = @{@"pid": @1, @"start": @10, @"physicalFootprintBytes": @20, @"residentBytes": @30, @"lifetimePeakBytes": @40, @"diskReadBytes": @50, @"diskWriteBytes": @60};
    NSDictionary *secondMember = @{@"pid": @2, @"start": @20, @"physicalFootprintBytes": @21, @"residentBytes": @31, @"lifetimePeakBytes": @41, @"diskReadBytes": @51, @"diskWriteBytes": @61};
    NSArray *members = @[firstMember, secondMember];
    NSMutableArray<NSDictionary *> *observations = [NSMutableArray array];
    for (NSUInteger index = 0; index < 11; index += 1) {
        [observations addObject:@{@"index": @(index), @"elapsedMilliseconds": @(index * 1000), @"deduplicatedBytes": @(100 + index), @"members": members}];
    }
    NSDictionary *receipt = @{@"scenario": scenario, @"observations": observations, @"peakDeduplicatedBytes": @110, @"isNativeReleaseObserved": @YES, @"isProcessMembershipStable": @YES};
    NSDictionary *splitStressScenario = @{@"tabCount": @7, @"phase": @"split-stress", @"isSplit": @YES, @"repetition": @1};
    NSDictionary *splitStressReceipt = @{@"scenario": splitStressScenario, @"observations": observations, @"peakDeduplicatedBytes": @110, @"isNativeReleaseObserved": @YES, @"isProcessMembershipStable": @YES};
    NSDictionary *splitStressWithoutNativeReleaseReceipt = @{@"scenario": splitStressScenario, @"observations": observations, @"peakDeduplicatedBytes": @110, @"isNativeReleaseObserved": @NO, @"isProcessMembershipStable": @YES};
    NSDictionary *loadedScenario = @{@"tabCount": @7, @"phase": @"loaded", @"isSplit": @NO, @"repetition": @1};
    NSDictionary *loadedReceiptWithoutNativeRelease = @{@"scenario": loadedScenario, @"observations": observations, @"peakDeduplicatedBytes": @110, @"isNativeReleaseObserved": @NO, @"isProcessMembershipStable": @YES};
    NSDictionary *invalidScenario = @{@"tabCount": @12, @"phase": @"split-stress", @"isSplit": @YES, @"repetition": @1};
    NSDictionary *fractionalTabCountScenario = @{@"tabCount": @7.5, @"phase": @"one-live", @"isSplit": @NO, @"repetition": @1};
    NSDictionary *fractionalRepetitionScenario = @{@"tabCount": @7, @"phase": @"one-live", @"isSplit": @NO, @"repetition": @1.5};
    NSDictionary *numericSplitScenario = @{@"tabCount": @7, @"phase": @"one-live", @"isSplit": @2, @"repetition": @1};
    NSDictionary *fractionalSplitScenario = @{@"tabCount": @7, @"phase": @"one-live", @"isSplit": @1.5, @"repetition": @1};
    NSMutableDictionary *numericNativeReleaseReceipt = [receipt mutableCopy];
    numericNativeReleaseReceipt[@"isNativeReleaseObserved"] = @2;
    NSMutableDictionary *fractionalMembershipReceipt = [receipt mutableCopy];
    fractionalMembershipReceipt[@"isProcessMembershipStable"] = @1.5;
    NSMutableArray<NSDictionary *> *reusedIdentifierObservations = [observations mutableCopy];
    NSMutableDictionary *reusedIdentifierMember = [secondMember mutableCopy];
    reusedIdentifierMember[@"start"] = @21;
    reusedIdentifierObservations[5] = @{@"index": @5, @"elapsedMilliseconds": @5000, @"deduplicatedBytes": @105, @"members": @[firstMember, reusedIdentifierMember]};
    NSDictionary *reusedIdentifierReceipt = @{@"scenario": scenario, @"observations": reusedIdentifierObservations, @"peakDeduplicatedBytes": @110, @"isNativeReleaseObserved": @YES, @"isProcessMembershipStable": @YES};
    NSMutableArray<NSDictionary *> *joinedServiceObservations = [observations mutableCopy];
    NSDictionary *joinedMember = @{@"pid": @3, @"start": @30, @"physicalFootprintBytes": @22, @"residentBytes": @32, @"lifetimePeakBytes": @42, @"diskReadBytes": @52, @"diskWriteBytes": @62};
    joinedServiceObservations[5] = @{@"index": @5, @"elapsedMilliseconds": @5000, @"deduplicatedBytes": @105, @"members": @[firstMember, secondMember, joinedMember]};
    NSDictionary *joinedServiceReceipt = @{@"scenario": scenario, @"observations": joinedServiceObservations, @"peakDeduplicatedBytes": @110, @"isNativeReleaseObserved": @YES, @"isProcessMembershipStable": @YES};
    NSMutableArray<NSDictionary *> *missingObservationKeyObservations = [observations mutableCopy];
    NSMutableDictionary *missingObservationKey = [observations[0] mutableCopy];
    [missingObservationKey removeObjectForKey:@"index"];
    missingObservationKeyObservations[0] = missingObservationKey;
    NSDictionary *missingObservationKeyReceipt = @{@"scenario": scenario, @"observations": missingObservationKeyObservations, @"peakDeduplicatedBytes": @110, @"isNativeReleaseObserved": @YES, @"isProcessMembershipStable": @YES};
    NSMutableArray<NSDictionary *> *extraObservationKeyObservations = [observations mutableCopy];
    NSMutableDictionary *extraObservationKey = [observations[0] mutableCopy];
    extraObservationKey[@"unexpected"] = @1;
    extraObservationKeyObservations[0] = extraObservationKey;
    NSDictionary *extraObservationKeyReceipt = @{@"scenario": scenario, @"observations": extraObservationKeyObservations, @"peakDeduplicatedBytes": @110, @"isNativeReleaseObserved": @YES, @"isProcessMembershipStable": @YES};
    NSMutableArray<NSDictionary *> *extraMemberKeyObservations = [observations mutableCopy];
    NSMutableDictionary *extraMemberKey = [firstMember mutableCopy];
    extraMemberKey[@"unexpected"] = @1;
    extraMemberKeyObservations[0] = @{@"index": @0, @"elapsedMilliseconds": @0, @"deduplicatedBytes": @100, @"members": @[extraMemberKey, secondMember]};
    NSDictionary *extraMemberKeyReceipt = @{@"scenario": scenario, @"observations": extraMemberKeyObservations, @"peakDeduplicatedBytes": @110, @"isNativeReleaseObserved": @YES, @"isProcessMembershipStable": @YES};
    NSMutableArray<NSDictionary *> *missingMemberKeyObservations = [observations mutableCopy];
    NSMutableDictionary *missingMemberKey = [firstMember mutableCopy];
    [missingMemberKey removeObjectForKey:@"start"];
    missingMemberKeyObservations[0] = @{@"index": @0, @"elapsedMilliseconds": @0, @"deduplicatedBytes": @100, @"members": @[missingMemberKey, secondMember]};
    NSDictionary *missingMemberKeyReceipt = @{@"scenario": scenario, @"observations": missingMemberKeyObservations, @"peakDeduplicatedBytes": @110, @"isNativeReleaseObserved": @YES, @"isProcessMembershipStable": @YES};
    XCTAssertTrue([WBMemoryBenchmark isValidScenario:scenario]);
    XCTAssertTrue([WBMemoryBenchmark isValidReceipt:receipt]);
    XCTAssertTrue([WBMemoryBenchmark isValidReceipt:splitStressReceipt]);
    XCTAssertTrue([WBMemoryBenchmark isValidReceipt:loadedReceiptWithoutNativeRelease]);
    XCTAssertFalse([WBMemoryBenchmark isValidReceipt:splitStressWithoutNativeReleaseReceipt]);
    XCTAssertFalse([WBMemoryBenchmark isValidScenario:invalidScenario]);
    XCTAssertFalse([WBMemoryBenchmark isValidScenario:fractionalTabCountScenario]);
    XCTAssertFalse([WBMemoryBenchmark isValidScenario:fractionalRepetitionScenario]);
    XCTAssertFalse([WBMemoryBenchmark isValidScenario:numericSplitScenario]);
    XCTAssertFalse([WBMemoryBenchmark isValidScenario:fractionalSplitScenario]);
    XCTAssertFalse([WBMemoryBenchmark isValidReceipt:numericNativeReleaseReceipt]);
    XCTAssertFalse([WBMemoryBenchmark isValidReceipt:fractionalMembershipReceipt]);
    XCTAssertFalse([WBMemoryBenchmark isValidReceipt:reusedIdentifierReceipt]);
    XCTAssertFalse([WBMemoryBenchmark isValidReceipt:joinedServiceReceipt]);
    XCTAssertFalse([WBMemoryBenchmark isValidReceipt:missingObservationKeyReceipt]);
    XCTAssertFalse([WBMemoryBenchmark isValidReceipt:extraObservationKeyReceipt]);
    XCTAssertFalse([WBMemoryBenchmark isValidReceipt:extraMemberKeyReceipt]);
    XCTAssertFalse([WBMemoryBenchmark isValidReceipt:missingMemberKeyReceipt]);
}

@end
