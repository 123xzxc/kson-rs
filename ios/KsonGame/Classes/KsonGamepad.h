#import <Foundation/Foundation.h>
#import <GameController/GameController.h>

NS_ASSUME_NONNULL_BEGIN

/// Bridges `GCController` input into the Rust renderer.
///
/// The Rust side cannot use `gilrs` on iPadOS (it depends on IOKit), so this
/// class observes connected controllers and forwards normalised button and
/// stick events through the `kson_ios_gamepad_*` C entry points. The numbering
/// below is a private protocol shared with `game/src/platform/gamepad.rs`.
@interface KsonGamepad : NSObject

/// Starts observing controller connect/disconnect notifications. Safe to call
/// more than once.
+ (void)start;

/// Number of currently connected controllers, for diagnostics.
+ (NSUInteger)connectedCount;

@end

NS_ASSUME_NONNULL_END
