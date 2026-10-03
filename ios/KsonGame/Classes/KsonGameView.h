#import <UIKit/UIKit.h>
#import <GLKit/GLKit.h>
#import <QuartzCore/QuartzCore.h>
#import <OpenGLES/EAGL.h>
#import <dlfcn.h>

NS_ASSUME_NONNULL_BEGIN

/// Hosts the EAGL-backed OpenGL ES context that the Rust renderer draws into,
/// forwards touches to the Rust input handlers and drives the frame loop with a
/// CADisplayLink.
@interface KsonGameView : UIView

/// When YES the view forwards touches as virtual controller buttons instead of
/// letting UIKit gesture recognizers consume them. Always YES for the game.
@property(nonatomic, assign) BOOL captureTouches;

- (void)startAnimation;
- (void)stopAnimation;

@end

NS_ASSUME_NONNULL_END
