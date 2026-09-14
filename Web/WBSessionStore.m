#import "WBSessionStore.h"
#import "WBTab.h"
#import <string.h>

static NSString *const WBSessionErrorDomain = @"Web.Session";
static const NSInteger WBSessionSchemaVersion = 1;
static const NSUInteger WBOpenTabLimit = 100;
static const NSUInteger WBClosedTabLimit = 25;
static const NSUInteger WBMaximumSessionByteSize = 8 * 1024 * 1024;

static NSError *WBSessionSizeError(void) {
    return [NSError errorWithDomain:WBSessionErrorDomain code:3 userInfo:@{NSLocalizedDescriptionKey: @"Session data is too large to save."}];
}

static BOOL WBIsExactIntegerType(NSNumber *number) {
    const char *type = number.objCType;
    return strcmp(type, @encode(char)) == 0 || strcmp(type, @encode(unsigned char)) == 0 || strcmp(type, @encode(short)) == 0 || strcmp(type, @encode(unsigned short)) == 0 || strcmp(type, @encode(int)) == 0 || strcmp(type, @encode(unsigned int)) == 0 || strcmp(type, @encode(long)) == 0 || strcmp(type, @encode(unsigned long)) == 0 || strcmp(type, @encode(long long)) == 0 || strcmp(type, @encode(unsigned long long)) == 0;
}

static BOOL WBIsExactNonBooleanIntegerInRange(id value, NSInteger minimum, NSInteger maximum) {
    if (![value isKindOfClass:NSNumber.class] || CFGetTypeID((__bridge CFTypeRef)value) == CFBooleanGetTypeID() || !WBIsExactIntegerType(value)) {
        return NO;
    }
    NSNumber *number = value;
    const char *type = number.objCType;
    BOOL isUnsigned = strcmp(type, @encode(unsigned char)) == 0 || strcmp(type, @encode(unsigned short)) == 0 || strcmp(type, @encode(unsigned int)) == 0 || strcmp(type, @encode(unsigned long)) == 0 || strcmp(type, @encode(unsigned long long)) == 0;
    if (isUnsigned) {
        return maximum >= 0 && number.unsignedLongLongValue >= (unsigned long long)minimum && number.unsignedLongLongValue <= (unsigned long long)maximum;
    }
    return number.longLongValue >= (long long)minimum && number.longLongValue <= (long long)maximum;
}

@interface WBSessionStore ()
@property (nonatomic, readwrite) NSURL *directoryURL;
@property (nonatomic) NSURL *currentURL;
@property (nonatomic) NSURL *backupURL;
@end

@implementation WBSessionStore

- (instancetype)initWithDirectoryURL:(NSURL *)directoryURL {
    self = [super init];
    if (self) {
        _directoryURL = directoryURL;
        _currentURL = [directoryURL URLByAppendingPathComponent:@"session.json"];
        _backupURL = [directoryURL URLByAppendingPathComponent:@"session.backup.json"];
    }
    return self;
}

- (BOOL)saveTabs:(NSArray<WBTab *> *)tabs selectedIdentifier:(NSUUID *)selectedIdentifier closedTabs:(NSArray<NSDictionary *> *)closedTabs error:(NSError * _Nullable * _Nullable)error {
    NSMutableArray<NSDictionary *> *tabDictionaries = [NSMutableArray arrayWithCapacity:tabs.count];
    for (WBTab *tab in tabs) {
        [tabDictionaries addObject:tab.dictionaryRepresentation];
    }
    NSDictionary *latest = [self loadSessionWithError:nil];
    NSInteger generation = [latest[@"generation"] integerValue] + 1;
    NSDictionary *document = @{
        @"version": @(WBSessionSchemaVersion),
        @"generation": @(generation),
        @"selectedIdentifier": selectedIdentifier.UUIDString,
        @"tabs": tabDictionaries,
        @"closedTabs": closedTabs
    };
    if (![self validatedSessionFromDictionary:document error:error]) {
        return NO;
    }
    NSData *data = [NSJSONSerialization dataWithJSONObject:document options:0 error:error];
    if (!data) {
        return NO;
    }
    if (data.length > WBMaximumSessionByteSize) {
        if (error) {
            *error = WBSessionSizeError();
        }
        return NO;
    }
    NSFileManager *manager = NSFileManager.defaultManager;
    if (![manager createDirectoryAtURL:self.directoryURL withIntermediateDirectories:YES attributes:nil error:error]) {
        return NO;
    }
    if ([manager fileExistsAtPath:self.currentURL.path]) {
        NSData *currentData = [self sessionDataAtURL:self.currentURL];
        id currentObject = currentData ? [NSJSONSerialization JSONObjectWithData:currentData options:0 error:nil] : nil;
        NSDictionary *currentSession = [currentObject isKindOfClass:NSDictionary.class] ? [self validatedSessionFromDictionary:currentObject error:nil] : nil;
        if (currentSession && ![currentData writeToURL:self.backupURL options:NSDataWritingAtomic error:error]) {
            return NO;
        }
    }
    return [data writeToURL:self.currentURL options:NSDataWritingAtomic error:error];
}

- (nullable NSDictionary *)loadSessionWithError:(NSError * _Nullable * _Nullable)error {
    NSArray<NSURL *> *URLs = @[self.currentURL, self.backupURL];
    NSDictionary *bestSession;
    for (NSURL *URL in URLs) {
        NSData *data = [self sessionDataAtURL:URL];
        if (!data) {
            continue;
        }
        id object = [NSJSONSerialization JSONObjectWithData:data options:0 error:nil];
        if (![object isKindOfClass:NSDictionary.class]) {
            continue;
        }
        NSDictionary *session = [self validatedSessionFromDictionary:object error:nil];
        if (session && (!bestSession || [session[@"generation"] unsignedLongLongValue] > [bestSession[@"generation"] unsignedLongLongValue])) {
            bestSession = session;
        }
    }
    if (!bestSession && error) {
        *error = [NSError errorWithDomain:WBSessionErrorDomain code:1 userInfo:@{NSLocalizedDescriptionKey: @"No valid session is available."}];
    }
    return bestSession;
}

- (nullable NSDictionary *)validatedSessionFromDictionary:(NSDictionary *)dictionary error:(NSError * _Nullable * _Nullable)error {
    NSSet<NSString *> *expectedKeys = [NSSet setWithArray:@[@"version", @"generation", @"selectedIdentifier", @"tabs", @"closedTabs"]];
    if (![dictionary isKindOfClass:NSDictionary.class] || ![expectedKeys isEqualToSet:[NSSet setWithArray:dictionary.allKeys]]) {
        return [self invalidSessionWithError:error];
    }
    NSNumber *version = dictionary[@"version"];
    NSNumber *generation = dictionary[@"generation"];
    NSString *selectedIdentifier = dictionary[@"selectedIdentifier"];
    NSArray *tabs = dictionary[@"tabs"];
    NSArray *closedTabs = dictionary[@"closedTabs"];
    if (!WBIsExactNonBooleanIntegerInRange(version, WBSessionSchemaVersion, WBSessionSchemaVersion) || !WBIsExactNonBooleanIntegerInRange(generation, 0, NSIntegerMax - 1) || ![selectedIdentifier isKindOfClass:NSString.class] || ![tabs isKindOfClass:NSArray.class] || tabs.count == 0 || tabs.count > WBOpenTabLimit || ![closedTabs isKindOfClass:NSArray.class] || closedTabs.count > WBClosedTabLimit) {
        return [self invalidSessionWithError:error];
    }
    NSMutableSet<NSUUID *> *identifiers = [NSMutableSet set];
    BOOL isSelected = NO;
    for (id tabDictionary in tabs) {
        if (![tabDictionary isKindOfClass:NSDictionary.class]) {
            return [self invalidSessionWithError:error];
        }
        WBTab *tab = [[WBTab alloc] initWithDictionary:tabDictionary];
        if (!tab || [identifiers containsObject:tab.identifier]) {
            return [self invalidSessionWithError:error];
        }
        [identifiers addObject:tab.identifier];
        if ([tab.identifier.UUIDString isEqualToString:selectedIdentifier]) {
            isSelected = YES;
        }
    }
    if (!isSelected) {
        return [self invalidSessionWithError:error];
    }
    for (id tabDictionary in closedTabs) {
        if (![tabDictionary isKindOfClass:NSDictionary.class]) {
            return [self invalidSessionWithError:error];
        }
        WBTab *tab = [[WBTab alloc] initWithDictionary:tabDictionary];
        if (!tab || [identifiers containsObject:tab.identifier]) {
            return [self invalidSessionWithError:error];
        }
        [identifiers addObject:tab.identifier];
    }
    return [dictionary copy];
}

- (nullable NSData *)sessionDataAtURL:(NSURL *)URL {
    NSDictionary<NSFileAttributeKey, id> *attributes = [NSFileManager.defaultManager attributesOfItemAtPath:URL.path error:nil];
    NSNumber *fileSize = attributes[NSFileSize];
    if (![fileSize isKindOfClass:NSNumber.class] || fileSize.unsignedLongLongValue > WBMaximumSessionByteSize) {
        return nil;
    }
    NSData *data = [NSData dataWithContentsOfURL:URL options:0 error:nil];
    return data.length <= WBMaximumSessionByteSize ? data : nil;
}

- (nullable NSDictionary *)invalidSessionWithError:(NSError * _Nullable * _Nullable)error {
    if (error) {
        *error = [NSError errorWithDomain:WBSessionErrorDomain code:2 userInfo:@{NSLocalizedDescriptionKey: @"Session data is invalid."}];
    }
    return nil;
}

@end
