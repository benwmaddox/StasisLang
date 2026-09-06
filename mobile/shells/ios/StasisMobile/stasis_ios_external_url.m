#import <UIKit/UIKit.h>

#include <stdint.h>

int stasis_platform_open_external_url(const char *url, int32_t length) {
    if (url == NULL || length <= 0) return 0;
    NSString *value = [[NSString alloc]
        initWithBytes:url length:(NSUInteger)length encoding:NSUTF8StringEncoding];
    if (value == nil) return 0;
    NSURL *target = [NSURL URLWithString:value];
    if (target == nil) return 0;

    void (^completion)(BOOL) = ^(BOOL success) {
        if (!success) NSLog(@"Stasis external URL request was blocked");
    };
    if (NSThread.isMainThread) {
        UIApplication *application = UIApplication.sharedApplication;
        if (application.applicationState != UIApplicationStateActive ||
            ![application canOpenURL:target]) return 0;
        [application openURL:target options:@{} completionHandler:completion];
        return 1;
    }

    dispatch_async(dispatch_get_main_queue(), ^{
        UIApplication *mainApplication = UIApplication.sharedApplication;
        if (mainApplication.applicationState == UIApplicationStateActive &&
            [mainApplication canOpenURL:target]) {
            [mainApplication openURL:target options:@{} completionHandler:completion];
        }
    });
    return 1;
}
