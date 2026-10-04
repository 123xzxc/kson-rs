#import "KsonGamepad.h"

/// Must match `GamepadButton::from_raw` in `game/src/platform/gamepad.rs`.
typedef NS_ENUM(int32_t, KsonGamepadButton) {
    KsonGamepadButtonSouth = 0,
    KsonGamepadButtonEast = 1,
    KsonGamepadButtonNorth = 2,
    KsonGamepadButtonWest = 3,
    KsonGamepadButtonLeftShoulder = 4,
    KsonGamepadButtonRightShoulder = 5,
    KsonGamepadButtonStart = 6,
    KsonGamepadButtonBack = 7,
    KsonGamepadButtonDPadUp = 8,
    KsonGamepadButtonDPadDown = 9,
    KsonGamepadButtonDPadLeft = 10,
    KsonGamepadButtonDPadRight = 11,
    KsonGamepadButtonLeftThumb = 12,
    KsonGamepadButtonRightThumb = 13,
};

extern void kson_ios_gamepad_button(int32_t button, bool pressed);
extern void kson_ios_gamepad_axis(int32_t side, float value);

/// Ignore tiny stick deflections so a resting controller reports a stable
/// centre position instead of drifting the laser.
static const float KsonStickDeadzone = 0.12f;

@implementation KsonGamepad

+ (void)start {
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        NSNotificationCenter *center = [NSNotificationCenter defaultCenter];
        [center addObserver:self
                   selector:@selector(controllerConnected:)
                       name:GCControllerDidConnectNotification
                     object:nil];
        [center addObserver:self
                   selector:@selector(controllerDisconnected:)
                       name:GCControllerDidDisconnectNotification
                     object:nil];

        // `startWirelessControllerDiscovery` would be needed only for
        // unpaired controllers; paired ones arrive through the notifications.
        [GCController startWirelessControllerDiscoveryWithCompletionHandler:nil];

        for (GCController *controller in GCController.controllers) {
            [self attachHandlersTo:controller];
        }
    });
}

+ (NSUInteger)connectedCount {
    return GCController.controllers.count;
}

+ (void)controllerConnected:(NSNotification *)note {
    GCController *controller = note.object;
    if ([controller isKindOfClass:GCController.class]) {
        [self attachHandlersTo:controller];
    }
}

+ (void)controllerDisconnected:(NSNotification *)note {
    // Releasing the handlers happens with the controller object itself; the
    // game keeps running with whatever input remains.
}

+ (void)attachHandlersTo:(GCController *)controller {
    GCExtendedGamepad *pad = controller.extendedGamepad;
    if (pad != nil) {
        [self attachExtendedGamepad:pad];
        return;
    }

    // Fall back to the simpler profile so older/limited controllers still work.
    GCGamepad *basic = controller.gamepad;
    if (basic != nil) {
        [self attachBasicGamepad:basic];
    }
    if (controller.microGamepad != nil) {
        [self attachMicroGamepad:controller.microGamepad];
    }
}

+ (void)attachExtendedGamepad:(GCExtendedGamepad *)pad {
    pad.buttonA.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonSouth];
    pad.buttonB.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonEast];
    pad.buttonX.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonNorth];
    pad.buttonY.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonWest];
    pad.leftShoulder.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonLeftShoulder];
    pad.rightShoulder.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonRightShoulder];

    if (@available(iOS 12.1, *)) {
        pad.leftThumbstickButton.pressedChangedHandler =
            [self pressHandlerFor:KsonGamepadButtonLeftThumb];
        pad.rightThumbstickButton.pressedChangedHandler =
            [self pressHandlerFor:KsonGamepadButtonRightThumb];
    }
    if (@available(iOS 13.0, *)) {
        pad.buttonMenu.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonStart];
        pad.buttonOptions.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonBack];
    }

    pad.dpad.up.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonDPadUp];
    pad.dpad.down.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonDPadDown];
    pad.dpad.left.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonDPadLeft];
    pad.dpad.right.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonDPadRight];

    // The sticks drive the two lasers: left stick -> left laser, right stick ->
    // right laser, matching the desktop axis handling.
    pad.leftThumbstick.valueChangedHandler =
        ^(GCControllerDirectionPad *dpad, float x, float y) {
            (void)dpad;
            (void)y;
            kson_ios_gamepad_axis(0, [self applyDeadzone:x]);
        };
    pad.rightThumbstick.valueChangedHandler =
        ^(GCControllerDirectionPad *dpad, float x, float y) {
            (void)dpad;
            (void)y;
            kson_ios_gamepad_axis(1, [self applyDeadzone:x]);
        };
}

+ (void)attachBasicGamepad:(GCGamepad *)pad {
    pad.buttonA.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonSouth];
    pad.buttonB.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonEast];
    pad.buttonX.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonNorth];
    pad.buttonY.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonWest];
    pad.leftShoulder.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonLeftShoulder];
    pad.rightShoulder.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonRightShoulder];

    pad.dpad.up.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonDPadUp];
    pad.dpad.down.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonDPadDown];
    pad.dpad.left.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonDPadLeft];
    pad.dpad.right.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonDPadRight];
}

+ (void)attachMicroGamepad:(GCMicroGamepad *)pad {
    pad.buttonA.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonSouth];
    pad.buttonX.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonNorth];
    pad.dpad.up.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonDPadUp];
    pad.dpad.down.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonDPadDown];
    pad.dpad.left.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonDPadLeft];
    pad.dpad.right.pressedChangedHandler = [self pressHandlerFor:KsonGamepadButtonDPadRight];
}

+ (GCControllerButtonValueChangedHandler)pressHandlerFor:(KsonGamepadButton)button {
    return ^(GCControllerButtonInput *input, float value, BOOL pressed) {
        (void)input;
        (void)value;
        kson_ios_gamepad_button((int32_t)button, pressed);
    };
}

+ (float)applyDeadzone:(float)value {
    if (fabsf(value) < KsonStickDeadzone) {
        return 0.0f;
    }
    return value;
}

@end
