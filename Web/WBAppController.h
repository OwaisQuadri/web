#import <AppKit/AppKit.h>

NS_ASSUME_NONNULL_BEGIN

@interface WBAppController : NSObject <NSApplicationDelegate>
@property (nonatomic, readonly) NSUInteger liveViewCount;
@end

NS_ASSUME_NONNULL_END
