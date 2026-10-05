#import "KsonGamepad.h"

// `fabsf`, used to measure how far a stick axis moved.
#import <math.h>

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
extern int32_t kson_ios_knob_axis(int32_t side);

/// Raw control indices shared with `game/src/platform/gamepad.rs`. The button
/// numbering matches `KsonGamepadButton`; the axes are the four stick axes used
/// by the binding UI.
typedef NS_ENUM(int32_t, KsonGamepadAxisRef) {
    KsonGamepadAxisLeftX = 0,
    KsonGamepadAxisLeftY = 1,
    KsonGamepadAxisRightX = 2,
    KsonGamepadAxisRightY = 3,
};

/// How much further the turned axis has to move than the idle one before the
/// idle one is dropped.
///
/// The firmware sends both axes of the stick in every packet, sampling the
/// encoder that did not turn along with the one that did, and that idle sample
/// wanders by a fraction of a detent. Forwarding it turns the other laser as
/// well, which the player sees as "one knob moves both lasers". A turned axis
/// moves several times further than the idle one, so a factor well above one
/// separates them while still letting both knobs be turned at once.
static const float KsonKnobAxisDominance = 2.5f;

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

    // By default only the left stick is used, and it drives both knobs: pushing
    // it up and down turns the left knob, pushing it left and right turns the
    // right one. The settings screen can rebind each knob to any of the four
    // stick axes, so ask the game which axis each knob is on before feeding it.
    pad.leftThumbstick.valueChangedHandler =
        ^(GCControllerDirectionPad *dpad, float x, float y) {
            (void)dpad;
            // No deadzone here: the PHAC knobs report an absolute position,
            // and squashing the middle of that range to zero would turn every
            // pass through it into a jump. The Rust side rejects noise on the
            // step instead.
            [self feedKnobsX:x y:y xRef:KsonGamepadAxisLeftX yRef:KsonGamepadAxisLeftY];
        };
    // The right stick is free by default, but it is still reported so it can be
    // bound from the settings screen.
    pad.rightThumbstick.valueChangedHandler =
        ^(GCControllerDirectionPad *dpad, float x, float y) {
            (void)dpad;
            [self feedKnobsX:x y:y xRef:KsonGamepadAxisRightX yRef:KsonGamepadAxisRightY];
        };
}

/// Feeds one stick's two axes into the knobs bound to them.
///
/// The PHAC firmware maps the left encoder onto the left stick's X and the
/// right encoder onto its Y, so a single stick carries both knobs. Which knob
/// each axis turns comes from the binding: `kson_ios_knob_axis` returns the
/// axis index for each knob, and the default puts X on the left knob and Y on
/// the right. `xRef`/`yRef` name the axes this stick reports; LeftX is 0,
/// LeftY 1, RightX 2 and RightY 3.
+ (void)feedKnobsX:(float)x y:(float)y xRef:(int32_t)xRef yRef:(int32_t)yRef {
    // How far each of the four stick axes moved since the previous report.
    // A binding is recorded from this, and a mirrored stick is spotted with
    // it as well.
    static float prevAxis[4];
    static BOOL haveAxis[4];
    float dx = haveAxis[xRef] ? fabsf(x - prevAxis[xRef]) : 0.0f;
    float dy = haveAxis[yRef] ? fabsf(y - prevAxis[yRef]) : 0.0f;
    prevAxis[xRef] = x;
    prevAxis[yRef] = y;
    haveAxis[xRef] = YES;
    haveAxis[yRef] = YES;

    // While a binding is being captured the axis that moved is recorded
    // instead of turning a knob.
    if ([self captureAxesX:xRef dx:dx yRef:yRef dy:dy]) {
        return;
    }

    // Some hosts report the left stick's position for the right stick as well
    // (the PHAC has no right stick). Feeding that mirrored movement to a knob
    // would turn a second laser with the same knob, so it is ignored.
    if (xRef != KsonGamepadAxisLeftX && haveAxis[KsonGamepadAxisLeftX] &&
        x == prevAxis[KsonGamepadAxisLeftX] && y == prevAxis[KsonGamepadAxisLeftY]) {
        return;
    }

    int32_t leftKnobAxis = kson_ios_knob_axis(0);
    int32_t rightKnobAxis = kson_ios_knob_axis(1);
    if (leftKnobAxis < 0) {
        leftKnobAxis = KsonGamepadAxisLeftX;
    }
    if (rightKnobAxis < 0) {
        rightKnobAxis = KsonGamepadAxisLeftY;
    }
    // A binding recorded while the wrong axis was moving can leave both knobs
    // on one axis, which makes a knob unreachable. Fall back to the layout the
    // firmware uses instead: the left knob on X, the right knob on Y.
    if (leftKnobAxis == rightKnobAxis) {
        leftKnobAxis = KsonGamepadAxisLeftX;
        rightKnobAxis = KsonGamepadAxisLeftY;
    }

    // Only the axis that actually turned is forwarded. Every packet carries
    // both, and the one that did not turn still wanders by a fraction of a
    // detent - enough to pass the Rust side's noise gate and drag the other
    // laser along. See `KsonKnobAxisDominance`.
    BOOL x_moved = dx > 0.0f;
    BOOL y_moved = dy > 0.0f;
    if (x_moved && y_moved) {
        if (dx > dy * KsonKnobAxisDominance) {
            y_moved = NO;
        } else if (dy > dx * KsonKnobAxisDominance) {
            x_moved = NO;
        }
    }

    // Send each axis to the knob that is bound to it, whichever stick the
    // axis came from.
    if (x_moved) {
        if (leftKnobAxis == xRef) {
            kson_ios_gamepad_axis(0, x);
        } else if (rightKnobAxis == xRef) {
            kson_ios_gamepad_axis(1, x);
        }
    }
    if (y_moved) {
        if (leftKnobAxis == yRef) {
            kson_ios_gamepad_axis(0, y);
        } else if (rightKnobAxis == yRef) {
            kson_ios_gamepad_axis(1, y);
        }
    }
}

/// Offers the axis that actually moved to the settings screen while it is
/// capturing a binding, so a stick can be bound instead of moving a knob.
///
/// Only the moved axis is offered. Reporting every axis of the stick on every
/// event is what recorded "axis 1" for every laser quadrant, and for the Back
/// key, no matter which axis the player moved.
///
/// Returns true when the movement completed a binding.
+ (BOOL)captureAxesX:(int32_t)xRef dx:(float)dx yRef:(int32_t)yRef dy:(float)dy {
    const float moved_enough = 0.02f;
    BOOL x_moved = dx > moved_enough;
    BOOL y_moved = dy > moved_enough;
    if (!x_moved && !y_moved) {
        return NO;
    }
    // A stick pushed diagonally moves both axes; the one that moved further is
    // the one the player meant.
    if (x_moved && (!y_moved || dx >= dy)) {
        return kson_ios_capture_gamepad_axis(xRef);
    }
    return kson_ios_capture_gamepad_axis(yRef);
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

@end
