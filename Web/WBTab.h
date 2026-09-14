#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

@interface WBTab : NSObject
@property (nonatomic, readonly) NSUUID *identifier;
@property (nonatomic, copy) NSString *title;
@property (nonatomic, copy) NSURL *URL;
@property (nonatomic) BOOL isPinned;
@property (nonatomic) double scrollX;
@property (nonatomic) double scrollY;
@property (nonatomic) BOOL isContentProcessTerminated;
@property (nonatomic, readonly) NSArray<NSURL *> *history;
@property (nonatomic, readonly) NSInteger historyIndex;
- (instancetype)initWithURL:(NSURL *)URL;
- (nullable instancetype)initWithDictionary:(NSDictionary *)dictionary;
- (void)recordURL:(NSURL *)URL;
- (BOOL)isAbleToGoBack;
- (BOOL)isAbleToGoForward;
- (NSURL *)URLAtHistoryOffset:(NSInteger)offset;
- (void)commitHistoryNavigationToURL:(NSURL *)URL;
- (void)completeHistoryNavigation;
- (void)rollbackHistoryNavigation;
- (NSDictionary *)dictionaryRepresentation;
@end

FOUNDATION_EXPORT NSURL * _Nullable WBURLForInput(NSString *input);
FOUNDATION_EXPORT NSURL *WBURLForPersistableURL(NSURL * _Nullable URL, NSURL * _Nullable fallbackURL);

NS_ASSUME_NONNULL_END
