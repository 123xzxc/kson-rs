#import "KsonGameView.h"
#import <dispatch/dispatch.h>
#import <OpenGLES/ES3/gl.h>
#import <OpenGLES/ES3/glext.h>

/// GL entry-point lookup / presentation shims used by the Rust renderer.
///
/// The Rust side (`game/src/platform/render.rs`) cannot link against
/// OpenGLES.framework directly, so it declares these two functions and the
/// Objective-C layer implements them against the live `EAGLContext`.
void *eagl_get_proc_address(EAGLContext *context, const char *name) {
    // `EAGLContext` has no per-context symbol lookup: OpenGL ES on iOS exposes
    // one global entry table, so the context argument only documents intent.
    //
    // `dlsym(RTLD_DEFAULT, ...)` does *not* find the OpenGL ES entry points:
    // they are weak-imported from OpenGLES.framework and are not part of the
    // default symbol search scope. Resolving through the framework handle
    // returns the real function pointers; without this glow and femtovg
    // silently end up with NULL for every entry point, which renders nothing
    // while audio keeps playing.
    (void)context;
    static void *handle = NULL;
    static dispatch_once_t once;
    dispatch_once(&once, ^{
        handle = dlopen("/System/Library/Frameworks/OpenGLES.framework/OpenGLES",
                        RTLD_LAZY | RTLD_LOCAL);
    });
    if (handle == NULL) {
        return NULL;
    }
    return dlsym(handle, name);
}

void eagl_present_renderbuffer(EAGLContext *context) {
    [EAGLContext setCurrentContext:context];
    [context presentRenderbuffer:GL_RENDERBUFFER];
}

/// Declarations of the Rust exports (see game/src/platform/app.rs).
extern bool kson_ios_init(const char *container_path,
                          const char *bundle_path,
                          void *eagl_context,
                          uint32_t framebuffer,
                          uint32_t width,
                          uint32_t height,
                          float scale);
extern void kson_ios_frame(double elapsed_ms);
extern void kson_ios_resize(double w, double h, float scale);
extern void kson_ios_touch(uint64_t touch_id, double x, double y, int32_t phase);

@interface KsonGameView () {
    EAGLContext *_context;
    GLuint _framebuffer;
    GLuint _colorRenderbuffer;
    GLuint _depthStencilRenderbuffer;
    CADisplayLink *_displayLink;
    CFTimeInterval _lastTimestamp;
    BOOL _initialized;
}
@end

@implementation KsonGameView

+ (Class)layerClass {
    return [CAEAGLLayer class];
}

- (instancetype)initWithFrame:(CGRect)frame {
    self = [super initWithFrame:frame];
    if (self) {
        [self commonInit];
    }
    return self;
}

- (instancetype)initWithCoder:(NSCoder *)coder {
    self = [super initWithCoder:coder];
    if (self) {
        [self commonInit];
    }
    return self;
}

- (void)commonInit {
    self.multipleTouchEnabled = YES;
    self.captureTouches = YES;

    CAEAGLLayer *layer = (CAEAGLLayer *)self.layer;
    layer.opaque = YES;
    // Without this the backbuffer is allocated in *logical* points (e.g.
    // 1373x954 on a 2x iPad), so the game renders into a quarter-size surface
    // while the screen is 2x. The Rust side reads this size back from the
    // renderbuffer, so the viewport and the presented surface disagreed.
    layer.contentsScale = self.contentScaleFactor;
    layer.drawableProperties = @{
        kEAGLDrawablePropertyRetainedBacking: @NO,
        kEAGLDrawablePropertyColorFormat: kEAGLColorFormatRGBA8,
    };

    _context = [[EAGLContext alloc] initWithAPI:kEAGLRenderingAPIOpenGLES3];
    if (_context == nil) {
        // Fall back to ES2 on very old hardware; femtovg will still work for
        // the subset of features the game uses.
        _context = [[EAGLContext alloc] initWithAPI:kEAGLRenderingAPIOpenGLES2];
    }
    [EAGLContext setCurrentContext:_context];

    glGenFramebuffers(1, &_framebuffer);
    glGenRenderbuffers(1, &_colorRenderbuffer);
    glGenRenderbuffers(1, &_depthStencilRenderbuffer);

    // `commonInit` runs from `initWithFrame:`/`initWithCoder:`, before the view
    // has been laid out, so `bounds` is frequently 0x0. Attaching a 0x0
    // drawable produces an incomplete framebuffer that never becomes complete
    // after `layoutSubviews` re-attaches unless we retry; create the storage
    // lazily from `layoutSubviews` instead.
    [self resizeDrawable];
}

- (void)dealloc {
    [self stopAnimation];
    if (_framebuffer) {
        glDeleteFramebuffers(1, &_framebuffer);
    }
    if (_colorRenderbuffer) {
        glDeleteRenderbuffers(1, &_colorRenderbuffer);
    }
    if ([EAGLContext currentContext] == _context) {
        [EAGLContext setCurrentContext:nil];
    }
}

- (GLint)drawableWidth {
    GLint width = 0;
    glGetRenderbufferParameteriv(GL_RENDERBUFFER, GL_RENDERBUFFER_WIDTH, &width);
    return width;
}

- (GLint)drawableHeight {
    GLint height = 0;
    glGetRenderbufferParameteriv(GL_RENDERBUFFER, GL_RENDERBUFFER_HEIGHT, &height);
    return height;
}

- (void)resizeDrawable {
    [EAGLContext setCurrentContext:_context];
    if (_context == nil || self.bounds.size.width <= 0.0 || self.bounds.size.height <= 0.0) {
        return;
    }
    // `contentScaleFactor` is still 1.0 inside `commonInit` because the view is
    // not on a screen yet, and under LiveContainer the window can report 1.0 as
    // well. Prefer the native screen scale, then the view's own value.
    CGFloat scale = self.contentScaleFactor;
    CGFloat screenScale = [UIScreen mainScreen].scale;
    if (screenScale > scale) {
        scale = screenScale;
        self.contentScaleFactor = screenScale;
    }
    CAEAGLLayer *eaglLayer = (CAEAGLLayer *)self.layer;
    if (eaglLayer.contentsScale != scale) {
        eaglLayer.contentsScale = scale;
    }

    glBindRenderbuffer(GL_RENDERBUFFER, _colorRenderbuffer);
    [_context renderbufferStorage:GL_RENDERBUFFER fromDrawable:eaglLayer];
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, _colorRenderbuffer);

    // femtovg's `set_screen_target` refuses a framebuffer without a depth and
    // stencil attachment, and three-d's depth-tested draws need one too. Size
    // it from the color renderbuffer so it always matches the drawable.
    GLint drawableW = 0;
    GLint drawableH = 0;
    glGetRenderbufferParameteriv(GL_RENDERBUFFER, GL_RENDERBUFFER_WIDTH, &drawableW);
    glGetRenderbufferParameteriv(GL_RENDERBUFFER, GL_RENDERBUFFER_HEIGHT, &drawableH);
    glBindRenderbuffer(GL_RENDERBUFFER, _depthStencilRenderbuffer);
    glRenderbufferStorage(GL_RENDERBUFFER, GL_DEPTH24_STENCIL8_OES, drawableW, drawableH);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_DEPTH_ATTACHMENT, GL_RENDERBUFFER, _depthStencilRenderbuffer);
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_STENCIL_ATTACHMENT, GL_RENDERBUFFER, _depthStencilRenderbuffer);

    GLenum status = glCheckFramebufferStatus(GL_FRAMEBUFFER);
    if (status != GL_FRAMEBUFFER_COMPLETE) {
        NSLog(@"[KsonGame] framebuffer incomplete: 0x%x (%.0fx%.0f @%.1fx)",
              status, self.bounds.size.width, self.bounds.size.height,
              scale);
    }

    CGFloat w = self.bounds.size.width;
    CGFloat h = self.bounds.size.height;
    if (_initialized) {
        kson_ios_resize(w, h, (float)scale);
    }
}

- (void)layoutSubviews {
    [super layoutSubviews];
    [self resizeDrawable];
}

- (void)startAnimation {
    if (_displayLink != nil) {
        return;
    }
    _lastTimestamp = 0;
    _displayLink = [CADisplayLink displayLinkWithTarget:self selector:@selector(tick:)];
    // Run at native refresh rate (120Hz on ProMotion iPads); the Rust side
    // applies its own frame limiter.
    _displayLink.preferredFrameRateRange = CAFrameRateRangeMake(60, 120, 120);
    [_displayLink addToRunLoop:[NSRunLoop mainRunLoop] forMode:NSRunLoopCommonModes];
}

- (void)stopAnimation {
    [_displayLink invalidate];
    _displayLink = nil;
}

- (void)tick:(CADisplayLink *)link {
    if (!_initialized) {
        if (![self initializeRust]) {
            // Initialization failed (missing assets, audio device, GL setup).
            // Stop the display link so the failure is not retried every frame,
            // and leave `_initialized` NO so no frame/touch call can reach
            // half-built Rust state.
            [self stopAnimation];
            return;
        }
        _initialized = YES;
        _lastTimestamp = link.timestamp;
        return;
    }

    double elapsedMs = (link.timestamp - _lastTimestamp) * 1000.0;
    _lastTimestamp = link.timestamp;
    kson_ios_frame(elapsedMs);
}

- (BOOL)initializeRust {
    [EAGLContext setCurrentContext:_context];
    glBindFramebuffer(GL_FRAMEBUFFER, _framebuffer);

    NSString *resources = [[NSBundle mainBundle] resourcePath];

    // `NSHomeDirectory()` is not trustworthy under sideloading tools such as
    // LiveContainer: it can return a path inside the *host* app's container
    // (`.../Documents/Data/Application/<uuid>/...`), which is both unwritable
    // in the expected way and different from the bundle location. Deriving the
    // container from the bundle keeps config, skins and the log file next to
    // the app that actually owns them.
    NSString *container = NSHomeDirectory();
    NSString *bundlePath = [[NSBundle mainBundle] bundlePath];
    NSRange appRange = [bundlePath rangeOfString:@".app" options:NSBackwardsSearch];
    if (appRange.location != NSNotFound) {
        NSRange slashRange = [bundlePath rangeOfString:@"/"
                                                options:NSBackwardsSearch
                                                  range:NSMakeRange(0, appRange.location)];
        if (slashRange.location != NSNotFound && slashRange.location > 0) {
            NSString *derived = [bundlePath substringToIndex:slashRange.location];
            // Only trust the derived path when it is actually writable; some
            // hosts launch the app from a read-only staging directory.
            if ([[NSFileManager defaultManager] isWritableFileAtPath:derived]) {
                container = derived;
            }
        }
    }

    GLint width = [self drawableWidth];
    GLint height = [self drawableHeight];

    NSLog(@"[KsonGame] init container=%@ bundle=%@ fb=%u size=%dx%d scale=%.2f",
          container, resources, _framebuffer, (int)width, (int)height,
          (double)self.contentScaleFactor);

    return kson_ios_init(container.fileSystemRepresentation,
                         resources.fileSystemRepresentation,
                         (__bridge void *)_context,
                         _framebuffer,
                         (uint32_t)width,
                         (uint32_t)height,
                         (float)self.contentScaleFactor);
}

#pragma mark - Touch handling

- (void)touchesBegan:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    [self forwardTouches:touches phase:0];
}

- (void)touchesMoved:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    [self forwardTouches:touches phase:1];
}

- (void)touchesEnded:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    [self forwardTouches:touches phase:2];
}

- (void)touchesCancelled:(NSSet<UITouch *> *)touches withEvent:(UIEvent *)event {
    [self forwardTouches:touches phase:3];
}

- (void)forwardTouches:(NSSet<UITouch *> *)touches phase:(int32_t)phase {
    if (!_initialized) {
        return;
    }
    for (UITouch *touch in touches) {
        CGPoint location = [touch locationInView:self];
        // `hash` is stable for the lifetime of a touch, which is all the Rust
        // side needs to track multi-touch.
        kson_ios_touch((uint64_t)touch.hash, location.x, location.y, phase);
    }
}

@end
