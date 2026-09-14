#import <AppKit/AppKit.h>
#import "WBAppController.h"
#import "WBMemoryBenchmark.h"

int main(int argc, const char * argv[]) {
    @autoreleasepool {
        NSArray<NSString *> *arguments = NSProcessInfo.processInfo.arguments;
        if (arguments.count == 2 && [arguments[1] isEqualToString:@"--memory-benchmark"]) {
            return [WBMemoryBenchmark runStrictBenchmark];
        }
        NSApplication *application = NSApplication.sharedApplication;
        WBAppController *controller = [[WBAppController alloc] init];
        application.delegate = controller;
        [application run];
    }
    return 0;
}
