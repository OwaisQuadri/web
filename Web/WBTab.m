#import "WBTab.h"
#import <math.h>
#import <string.h>

static const NSUInteger WBHistoryLimit = 100;
static const NSUInteger WBMaximumPersistedTitleLength = 1024;
static const NSUInteger WBMaximumPersistedURLStringLength = 8192;
static const double WBMaximumPersistedScrollCoordinate = 10000000.0;
static const NSInteger WBNoHistoryNavigation = -1;

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

static BOOL WBIsFiniteNonBooleanNumberInAbsoluteRange(id value, double maximum) {
    return [value isKindOfClass:NSNumber.class] && CFGetTypeID((__bridge CFTypeRef)value) != CFBooleanGetTypeID() && isfinite([value doubleValue]) && fabs([value doubleValue]) <= maximum;
}

@interface WBTab ()
@property (nonatomic, readwrite) NSUUID *identifier;
@property (nonatomic) NSMutableArray<NSURL *> *mutableHistory;
@property (nonatomic) NSInteger historyIndex;
@property (nonatomic) NSInteger historyNavigationOriginalIndex;
@property (nonatomic) NSInteger historyNavigationTargetIndex;
@property (nonatomic) NSURL *historyNavigationTargetURL;
@end

@implementation WBTab

- (instancetype)initWithURL:(NSURL *)URL {
    self = [super init];
    if (self) {
        NSURL *persistableURL = WBURLForPersistableURL(URL, nil);
        _identifier = [NSUUID UUID];
        _URL = [persistableURL copy];
        self.title = persistableURL.host.length > 0 ? persistableURL.host : @"New Tab";
        _mutableHistory = [NSMutableArray arrayWithObject:persistableURL];
        _historyIndex = 0;
        _historyNavigationOriginalIndex = WBNoHistoryNavigation;
        _historyNavigationTargetIndex = WBNoHistoryNavigation;
    }
    return self;
}

- (nullable instancetype)initWithDictionary:(NSDictionary *)dictionary {
    NSSet<NSString *> *expectedKeys = [NSSet setWithArray:@[@"identifier", @"title", @"url", @"history", @"historyIndex", @"isPinned", @"scrollX", @"scrollY"]];
    if (![dictionary isKindOfClass:NSDictionary.class] || dictionary.count != expectedKeys.count || ![expectedKeys isEqualToSet:[NSSet setWithArray:dictionary.allKeys]]) {
        return nil;
    }
    NSString *identifierString = dictionary[@"identifier"];
    NSString *urlString = dictionary[@"url"];
    NSString *title = dictionary[@"title"];
    NSArray *historyStrings = dictionary[@"history"];
    NSNumber *historyIndex = dictionary[@"historyIndex"];
    if (![identifierString isKindOfClass:NSString.class] || ![urlString isKindOfClass:NSString.class] || urlString.length > WBMaximumPersistedURLStringLength || ![title isKindOfClass:NSString.class] || title.length > WBMaximumPersistedTitleLength || ![historyStrings isKindOfClass:NSArray.class] || historyStrings.count == 0 || historyStrings.count > WBHistoryLimit || !WBIsExactNonBooleanIntegerInRange(historyIndex, 0, (NSInteger)historyStrings.count - 1)) {
        return nil;
    }
    for (id value in historyStrings) {
        if (![value isKindOfClass:NSString.class] || ((NSString *)value).length > WBMaximumPersistedURLStringLength) {
            return nil;
        }
    }
    NSUUID *identifier = [[NSUUID alloc] initWithUUIDString:identifierString];
    NSURL *URL = WBURLForInput(urlString);
    if (!identifier || !URL) {
        return nil;
    }
    NSMutableArray<NSURL *> *history = [NSMutableArray arrayWithCapacity:historyStrings.count];
    for (NSString *historyString in historyStrings) {
        NSURL *historyURL = WBURLForInput(historyString);
        if (!historyURL) {
            return nil;
        }
        [history addObject:historyURL];
    }
    if (![URL isEqual:history[(NSUInteger)historyIndex.integerValue]]) {
        return nil;
    }
    NSNumber *isPinned = dictionary[@"isPinned"];
    NSNumber *scrollX = dictionary[@"scrollX"];
    NSNumber *scrollY = dictionary[@"scrollY"];
    if (![isPinned isKindOfClass:NSNumber.class] || CFGetTypeID((__bridge CFTypeRef)isPinned) != CFBooleanGetTypeID() || !WBIsFiniteNonBooleanNumberInAbsoluteRange(scrollX, WBMaximumPersistedScrollCoordinate) || !WBIsFiniteNonBooleanNumberInAbsoluteRange(scrollY, WBMaximumPersistedScrollCoordinate)) {
        return nil;
    }
    self = [super init];
    if (self) {
        _identifier = identifier;
        _URL = URL;
        self.title = title;
        _mutableHistory = history;
        _historyIndex = historyIndex.integerValue;
        _historyNavigationOriginalIndex = WBNoHistoryNavigation;
        _historyNavigationTargetIndex = WBNoHistoryNavigation;
        _isPinned = isPinned.boolValue;
        _scrollX = scrollX.doubleValue;
        _scrollY = scrollY.doubleValue;
    }
    return self;
}

- (void)setTitle:(NSString *)title {
    NSUInteger length = title.length;
    if (length > WBMaximumPersistedTitleLength) {
        NSRange boundary = [title rangeOfComposedCharacterSequenceAtIndex:WBMaximumPersistedTitleLength];
        length = boundary.location < WBMaximumPersistedTitleLength ? boundary.location : WBMaximumPersistedTitleLength;
    }
    _title = [[title substringToIndex:length] copy];
}

- (void)setScrollX:(double)scrollX {
    _scrollX = isfinite(scrollX) ? fmax(-WBMaximumPersistedScrollCoordinate, fmin(scrollX, WBMaximumPersistedScrollCoordinate)) : 0;
}

- (void)setScrollY:(double)scrollY {
    _scrollY = isfinite(scrollY) ? fmax(-WBMaximumPersistedScrollCoordinate, fmin(scrollY, WBMaximumPersistedScrollCoordinate)) : 0;
}

- (NSArray<NSURL *> *)history {
    return [self.mutableHistory copy];
}

- (void)recordURL:(NSURL *)URL {
    NSURL *persistableURL = WBURLForPersistableURL(URL, self.URL);
    [self completeHistoryNavigation];
    if ([self.URL isEqual:persistableURL]) {
        return;
    }
    if (self.historyIndex + 1 < (NSInteger)self.mutableHistory.count) {
        NSRange range = NSMakeRange((NSUInteger)(self.historyIndex + 1), self.mutableHistory.count - (NSUInteger)(self.historyIndex + 1));
        [self.mutableHistory removeObjectsInRange:range];
    }
    [self.mutableHistory addObject:persistableURL];
    if (self.mutableHistory.count > WBHistoryLimit) {
        [self.mutableHistory removeObjectAtIndex:0];
    }
    self.historyIndex = (NSInteger)self.mutableHistory.count - 1;
    self.URL = persistableURL;
}

- (BOOL)isAbleToGoBack {
    return self.historyIndex > 0;
}

- (BOOL)isAbleToGoForward {
    return self.historyIndex + 1 < (NSInteger)self.mutableHistory.count;
}

- (NSURL *)URLAtHistoryOffset:(NSInteger)offset {
    NSInteger targetIndex = self.historyIndex + offset;
    if (targetIndex < 0 || targetIndex >= (NSInteger)self.mutableHistory.count) {
        return self.URL;
    }
    self.historyNavigationOriginalIndex = self.historyIndex;
    self.historyNavigationTargetIndex = targetIndex;
    self.historyNavigationTargetURL = self.mutableHistory[(NSUInteger)targetIndex];
    self.historyIndex = targetIndex;
    self.URL = self.mutableHistory[(NSUInteger)targetIndex];
    return self.URL;
}

- (void)commitHistoryNavigationToURL:(NSURL *)URL {
    NSURL *persistableURL = WBURLForPersistableURL(URL, self.URL);
    if (self.historyNavigationOriginalIndex == WBNoHistoryNavigation) {
        [self recordURL:persistableURL];
        return;
    }
    self.mutableHistory[(NSUInteger)self.historyIndex] = persistableURL;
    self.URL = persistableURL;
}

- (void)completeHistoryNavigation {
    self.historyNavigationOriginalIndex = WBNoHistoryNavigation;
    self.historyNavigationTargetIndex = WBNoHistoryNavigation;
    self.historyNavigationTargetURL = nil;
}

- (void)rollbackHistoryNavigation {
    if (self.historyNavigationOriginalIndex != WBNoHistoryNavigation) {
        if (self.historyNavigationTargetURL && self.historyNavigationTargetIndex >= 0 && self.historyNavigationTargetIndex < (NSInteger)self.mutableHistory.count) {
            self.mutableHistory[(NSUInteger)self.historyNavigationTargetIndex] = self.historyNavigationTargetURL;
        }
        self.historyIndex = self.historyNavigationOriginalIndex;
        self.URL = self.mutableHistory[(NSUInteger)self.historyIndex];
    }
    [self completeHistoryNavigation];
}

- (NSDictionary *)dictionaryRepresentation {
    NSMutableArray<NSString *> *historyStrings = [NSMutableArray arrayWithCapacity:self.mutableHistory.count];
    for (NSURL *historyURL in self.mutableHistory) {
        [historyStrings addObject:historyURL.absoluteString];
    }
    return @{
        @"identifier": self.identifier.UUIDString,
        @"title": self.title,
        @"url": self.URL.absoluteString,
        @"history": historyStrings,
        @"historyIndex": @(self.historyIndex),
        @"isPinned": @(self.isPinned),
        @"scrollX": @(self.scrollX),
        @"scrollY": @(self.scrollY)
    };
}

@end

static NSURL *WBURLFromPersistableComponents(NSURLComponents *components) {
    if (components.user != nil || components.password != nil) {
        return nil;
    }
    NSURL *URL = components.URL;
    return URL.absoluteString.length > 0 && URL.absoluteString.length <= WBMaximumPersistedURLStringLength ? URL : nil;
}

NSURL * WBURLForPersistableURL(NSURL * _Nullable URL, NSURL * _Nullable fallbackURL) {
    NSURL *persistableURL = URL.absoluteString.length > 0 ? WBURLForInput(URL.absoluteString) : nil;
    if (persistableURL) {
        return persistableURL;
    }
    persistableURL = fallbackURL.absoluteString.length > 0 ? WBURLForInput(fallbackURL.absoluteString) : nil;
    if (persistableURL) {
        return persistableURL;
    }
    return [NSURL URLWithString:@"https://www.google.com/"];
}

NSURL * _Nullable WBURLForInput(NSString *input) {
    NSString *trimmed = [input stringByTrimmingCharactersInSet:NSCharacterSet.whitespaceAndNewlineCharacterSet];
    if (trimmed.length == 0 || trimmed.length > WBMaximumPersistedURLStringLength) {
        return nil;
    }
    BOOL isWhitespacePresent = [trimmed rangeOfCharacterFromSet:NSCharacterSet.whitespaceAndNewlineCharacterSet].location != NSNotFound;
    NSURLComponents *hostComponents = isWhitespacePresent ? nil : [NSURLComponents componentsWithString:[@"https://" stringByAppendingString:trimmed]];
    if (![trimmed containsString:@"://"] && hostComponents.host.length > 0 && hostComponents.port) {
        return WBURLFromPersistableComponents(hostComponents);
    }
    NSURLComponents *components = [NSURLComponents componentsWithString:trimmed];
    if (components.scheme.length > 0) {
        NSString *scheme = components.scheme.lowercaseString;
        if (!([scheme isEqualToString:@"http"] || [scheme isEqualToString:@"https"]) || components.host.length == 0) {
            return nil;
        }
        return WBURLFromPersistableComponents(components);
    }
    BOOL isHostLike = ([trimmed containsString:@"."] || [trimmed isEqualToString:@"localhost"] || [trimmed containsString:@":"]);
    if (isHostLike && !isWhitespacePresent) {
        return hostComponents.host.length > 0 ? WBURLFromPersistableComponents(hostComponents) : nil;
    }
    NSURLComponents *searchComponents = [[NSURLComponents alloc] init];
    searchComponents.scheme = @"https";
    searchComponents.host = @"www.google.com";
    searchComponents.path = @"/search";
    searchComponents.queryItems = @[[NSURLQueryItem queryItemWithName:@"q" value:trimmed]];
    return WBURLFromPersistableComponents(searchComponents);
}
