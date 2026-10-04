#import <UIKit/UIKit.h>
#import "KsonGameView.h"
#import "KsonGamepad.h"

@interface KsonGameViewController : UIViewController
@property(nonatomic, strong) KsonGameView *gameView;
@end

@implementation KsonGameViewController

- (void)loadView {
    self.gameView = [[KsonGameView alloc] initWithFrame:[UIScreen mainScreen].bounds];
    self.view = self.gameView;
    self.view.backgroundColor = [UIColor blackColor];
}

- (BOOL)prefersStatusBarHidden {
    return YES;
}

- (BOOL)prefersHomeIndicatorAutoHidden {
    return YES;
}

- (UIInterfaceOrientationMask)supportedInterfaceOrientations {
    // SDVX is a landscape game; lock to landscape to avoid a resize storm
    // during play.
    return UIInterfaceOrientationMaskLandscape;
}

- (void)viewDidAppear:(BOOL)animated {
    [super viewDidAppear:animated];
    [self.gameView startAnimation];
}

- (void)viewWillDisappear:(BOOL)animated {
    [super viewWillDisappear:animated];
    [self.gameView stopAnimation];
}

@end

@interface KsonGameAppDelegate : UIResponder <UIApplicationDelegate>
@property(nonatomic, strong) UIWindow *window;
@end

@implementation KsonGameAppDelegate

- (BOOL)application:(UIApplication *)application
        didFinishLaunchingWithOptions:(NSDictionary<UIApplicationLaunchOptionsKey, id> *)launchOptions {
    // Controllers can connect at any time, including before the game starts.
    [KsonGamepad start];
    self.window = [[UIWindow alloc] initWithFrame:[UIScreen mainScreen].bounds];
    self.window.rootViewController = [[KsonGameViewController alloc] init];
    [self.window makeKeyAndVisible];
    return YES;
}

@end

int main(int argc, char *argv[]) {
    @autoreleasepool {
        return UIApplicationMain(argc, argv, nil, NSStringFromClass([KsonGameAppDelegate class]));
    }
}
