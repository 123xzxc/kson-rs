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
extern void kson_ios_set_controllers(const uint32_t *indices,
                                     const char *const *names,
                                     size_t count);
extern bool kson_ios_capture_gamepad_button(int32_t index);
extern bool kson_ios_capture_gamepad_axis(int32_t index);
extern int32_t kson_ios_axis_binding(int32_t kind, int32_t raw_button);

/// Raw control indices shared with `game/src/platform/gamepad.rs`. The button
/// numbering matches `KsonGamepadButton`; the axes are the four stick axes used
/// by the binding UI.
typedef NS_ENUM(int32_t, KsonGamepadAxisRef) {
    KsonGamepadAxisLeftX = 0,
    KsonGamepadAxisLeftY = 1,
    KsonGamepadAxisRightX = 2,
    KsonGamepadAxisRightY = 3,
};

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
        [self publishControllers];
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
    [self publishControllers];
}

+ (void)controllerDisconnected:(NSNotification *)note {
    // Releasing the handlers happens with the controller object itself; the
    // game keeps running with whatever input remains.
    [self publishControllers];
}

/// Hands the connected controllers to the Rust settings screen.
///
/// `gilrs` cannot enumerate pads on iPadOS, so the settings dropdown is fed
/// from here; the index is `playerIndex`, which is what the bindings are keyed
/// by.
+ (void)publishControllers {
    NSArray<GCController *> *controllers = GCController.controllers;
    NSUInteger count = controllers.count;
    if (count == 0) {
        kson_ios_set_controllers(NULL, NULL, 0);
        return;
    }

    uint32_t *indices = calloc(count, sizeof(uint32_t));
    const char **names = calloc(count, sizeof(char *));
    if (indices == NULL || names == NULL) {
        free(indices);
        free(names);
        return;
    }
    for (NSUInteger i = 0; i < count; i++) {
        GCController *controller = controllers[i];
        indices[i] = (uint32_t)controller.playerIndex;
        names[i] = controller.vendorName.UTF8String;
        if (names[i] == NULL) {
            names[i] = "Controller";
        }
    }
    kson_ios_set_controllers(indices, names, count);
    free(names);
    free(indices);
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

    // Only the left stick is used, and it drives both knobs: pushing it up and
    // down turns the left knob, pushing it left and right turns the right one.
    // The right stick is deliberately left unbound so it stays free.
    pad.leftThumbstick.valueChangedHandler =
        ^(GCControllerDirectionPad *dpad, float x, float y) {
            (void)dpad;
            float vertical = [self applyDeadzone:y];
            float horizontal = [self applyDeadzone:x];
            // While a binding is being captured any axis that moves is
            // recorded instead of turning a knob.
            if ([self captureAxes]) {
                return;
            }
            if (vertical != 0.0f
                && kson_ios_axis_binding(0, KsonGamepadButtonDPadUp) == -1) {
                kson_ios_gamepad_axis(0, vertical);
            }
            if (horizontal != 0.0f
                && kson_ios_axis_binding(0, KsonGamepadButtonDPadLeft) == -1) {
                kson_ios_gamepad_axis(1, horizontal);
            }
        };
    // The right stick stays free, but it is still reported so it can be bound
    // from the settings screen.
    pad.rightThumbstick.valueChangedHandler =
        ^(GCControllerDirectionPad *dpad, float x, float y) {
            (void)dpad;
            float vertical = [self applyDeadzone:y];
            float horizontal = [self applyDeadzone:x];
            if ([self captureAxes]) {
                return;
            }
            if (vertical != 0.0f
                && kson_ios_axis_binding(0, KsonGamepadButtonDPadUp) == -1) {
                kson_ios_gamepad_axis(1, vertical);
            }
            if (horizontal != 0.0f
                && kson_ios_axis_binding(0, KsonGamepadButtonDPadRight) == -1) {
                kson_ios_gamepad_axis(0, horizontal);
            }
        };
}

/// Reports all four stick axes while the settings screen is capturing a
/// binding, so a stick can be bound instead of moving a knob.
///
/// Returns true when any of them completed a binding.
+ (BOOL)captureAxes {
    BOOL captured = kson_ios_capture_gamepad_axis(KsonGamepadAxisLeftY);
    captured = kson_ios_capture_gamepad_axis(KsonGamepadAxisLeftX) || captured;
    captured = kson_ios_capture_gamepad_axis(KsonGamepadAxisRightY) || captured;
    captured = kson_ios_capture_gamepad_axis(KsonGamepadAxisRightX) || captured;
    return captured;
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
        // While the settings screen is capturing a binding the press is
        // recorded instead of being forwarded, so it never reaches the game.
        if (pressed && kson_ios_capture_gamepad_button((int32_t)button)) {
            return;
        }
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
