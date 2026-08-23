#import <UIKit/UIKit.h>

#include <stddef.h>
#include <stdint.h>
#include <string.h>

#include "stasis_mobile_aot_runtime.h"

/*
 * The pairing URL is native-only. It is copied out of the network host into
 * bounded native storage and presented through UIKit; it never enters Stasis
 * globals, snapshots, logs, analytics, or semantic frames.
 */
#if defined(STASIS_NETWORK_ENABLED)
static UIWindow *stasis_mobile_active_window(void) {
    for (UIScene *scene in UIApplication.sharedApplication.connectedScenes) {
        if (scene.activationState != UISceneActivationStateForegroundActive) {
            continue;
        }
        if (![scene isKindOfClass:[UIWindowScene class]]) {
            continue;
        }
        for (UIWindow *window in ((UIWindowScene *)scene).windows) {
            if (window.isKeyWindow || window.rootViewController != nil) {
                return window;
            }
        }
    }
    return nil;
}

void stasis_mobile_network_present_join_url(void) {
    char url[2048] = {0};
    int32_t length = stasis_mobile_network_copy_join_url(url, sizeof(url));
    if (length <= 0 || (size_t)length >= sizeof(url)) {
        memset(url, 0, sizeof(url));
        return;
    }
    NSString *joinURL = [[NSString alloc] initWithBytes:url
                                                  length:(NSUInteger)length
                                                encoding:NSUTF8StringEncoding];
    memset(url, 0, sizeof(url));
    if (joinURL == nil) {
        return;
    }
    dispatch_async(dispatch_get_main_queue(), ^{
        UIWindow *window = stasis_mobile_active_window();
        UIViewController *presenter = window.rootViewController;
        while (presenter.presentedViewController != nil) {
            presenter = presenter.presentedViewController;
        }
        if (presenter == nil) {
            return;
        }
        UIAlertController *alert = [UIAlertController
            alertControllerWithTitle:@"@STASIS_APP_NAME@"
                             message:joinURL
                      preferredStyle:UIAlertControllerStyleAlert];
        [alert addAction:[UIAlertAction actionWithTitle:@"Copy URL"
                                                  style:UIAlertActionStyleDefault
                                                handler:^(__unused UIAlertAction *action) {
            [UIPasteboard generalPasteboard].string = joinURL;
        }]];
        [alert addAction:[UIAlertAction actionWithTitle:@"Dismiss"
                                                  style:UIAlertActionStyleCancel
                                                handler:nil]];
        [presenter presentViewController:alert animated:YES completion:nil];
    });
}
#endif
