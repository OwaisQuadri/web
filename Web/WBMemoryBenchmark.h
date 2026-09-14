#import <Foundation/Foundation.h>

NS_ASSUME_NONNULL_BEGIN

@interface WBMemoryBenchmark : NSObject
+ (int)runStrictBenchmark;
+ (BOOL)isValidReceipt:(NSDictionary *)receipt;
+ (BOOL)isValidScenario:(NSDictionary *)scenario;
@end

NS_ASSUME_NONNULL_END
