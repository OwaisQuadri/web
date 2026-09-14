#import <Foundation/Foundation.h>

@class WBTab;

NS_ASSUME_NONNULL_BEGIN

@interface WBSessionStore : NSObject
- (instancetype)initWithDirectoryURL:(NSURL *)directoryURL;
- (BOOL)saveTabs:(NSArray<WBTab *> *)tabs selectedIdentifier:(NSUUID *)selectedIdentifier closedTabs:(NSArray<NSDictionary *> *)closedTabs error:(NSError * _Nullable * _Nullable)error;
- (nullable NSDictionary *)loadSessionWithError:(NSError * _Nullable * _Nullable)error;
- (nullable NSDictionary *)validatedSessionFromDictionary:(NSDictionary *)dictionary error:(NSError * _Nullable * _Nullable)error;
@end

NS_ASSUME_NONNULL_END
