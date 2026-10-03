#import "KsonGameView.h"
#import <OpenGLES/ES3/gl.h>
#import <OpenGLES/ES3/glext.h>

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
    glBindFramebuffer(GL_FRAMEBUFFER, _framebuffer);
    glBindRenderbuffer(GL_RENDERBUFFER, _colorRenderbuffer);
    [_context renderbufferStorage:GL_RENDERBUFFER fromDrawable:layer];
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, _colorRenderbuffer);

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
    glBindRenderbuffer(GL_RENDERBUFFER, _colorRenderbuffer);
    [_context renderbufferStorage:GL_RENDERBUFFER fromDrawable:(CAEAGLLayer *)self.layer];
    glFramebufferRenderbuffer(GL_FRAMEBUFFER, GL_COLOR_ATTACHMENT0, GL_RENDERBUFFER, _colorRenderbuffer);

    CGFloat scale = self.contentScaleFactor;
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
        [self initializeRust];
        _initialized = YES;
        _lastTimestamp = link.timestamp;
        return;
    }

    double elapsedMs = (link.timestamp - _lastTimestamp) * 1000.0;
    _lastTimestamp = link.timestamp;
    kson_ios_frame(elapsedMs);
}

- (void)initializeRust {
    [EAGLContext setCurrentContext:_context];
    glBindFramebuffer(GL_FRAMEBUFFER, _framebuffer);

    NSString *container = NSHomeDirectory();
    NSString *resources = [[NSBundle mainBundle] resourcePath];

    GLint width = [self drawableWidth];
    GLint height = [self drawableHeight];

    kson_ios_init(container.fileSystemRepresentation,
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
