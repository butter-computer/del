// Renders Del's real background renderer offscreen so motion and lighting can be
// inspected and costed without launching the app. It runs in real time because
// the renderer owns its clock, exactly as it does inside the app.
//
// clang -fobjc-arc -Wno-incompatible-pointer-types -Wno-deprecated-declarations \
//   -framework Foundation -framework AppKit -framework Metal -framework CoreVideo \
//   -framework QuartzCore -framework ImageIO -framework CoreServices \
//   crates/app/src/fluid.m scripts/render-backgrounds.m -o /tmp/del-render
// /tmp/del-render crates/app/src/fluid.metal <out-dir> [style] [dark|light] [seconds]
#import <AppKit/AppKit.h>
#import <CoreVideo/CoreVideo.h>
#import <ImageIO/ImageIO.h>
#import <Metal/Metal.h>
#import <QuartzCore/QuartzCore.h>
#import <simd/simd.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
void del_fluid_region(void *, unsigned int, double, double, double, double, double);
void *del_fluid_create(const unsigned char *, size_t, const char *, size_t);
void del_fluid_release(void *);
void del_fluid_running(void *, bool);
void *del_fluid_frame(void *, double, double, double, double, double, double, double, double, double, bool, unsigned int, float, float, float, float, bool);
void del_fluid_prepare(const void *, void *);
void del_fluid_blurred(void *, bool);
void del_fluid_force_press(void *, bool);

enum { LOGICAL_W = 1100, LOGICAL_H = 760, SCALE = 2 };
static const char *names[] = {"", "", "life", "liquid", "contours", "drift", "fluted", "curtains", "rain", "clouds", "leaves", "pudding", "windowcurtains", "seafloor"};

static void writePNG(CVPixelBufferRef buffer, bool light, simd_float4 tint, NSString *path) {
    CVPixelBufferLockBaseAddress(buffer, kCVPixelBufferLock_ReadOnly);
    unsigned char *bytes = CVPixelBufferGetBaseAddress(buffer);
    size_t stride = CVPixelBufferGetBytesPerRow(buffer), w = CVPixelBufferGetWidth(buffer), h = CVPixelBufferGetHeight(buffer);
    NSMutableData *rgba = [NSMutableData dataWithLength:w * h * 4];
    unsigned char *out = rgba.mutableBytes;
    float bg = light ? 1 : 0;
    for (size_t y = 0; y < h; y++) for (size_t x = 0; x < w; x++) {
        unsigned char *p = bytes + y * stride + x * 4;
        float a = p[3] / 255.0f;
        unsigned char *o = out + (y * w + x) * 4;
        o[0] = (bg * (1 - a) + p[2] / 255.0f * a) * 255;
        o[1] = (bg * (1 - a) + p[1] / 255.0f * a) * 255;
        o[2] = (bg * (1 - a) + p[0] / 255.0f * a) * 255;
        o[3] = 255;
    }
    CVPixelBufferUnlockBaseAddress(buffer, kCVPixelBufferLock_ReadOnly);
    CGColorSpaceRef space = CGColorSpaceCreateWithName(kCGColorSpaceSRGB);
    CGContextRef context = CGBitmapContextCreate(out, w, h, 8, w * 4, space, kCGImageAlphaNoneSkipLast);
    // Stand-ins for the UI drawn over the background, to judge the result in context.
    CGContextSetRGBFillColor(context, light ? 0.976 : 0.11, light ? 0.976 : 0.11, light ? 0.976 : 0.11, 1);
    CGRect bar = CGRectMake(150 * SCALE, 40 * SCALE, (LOGICAL_W - 300) * SCALE, 110 * SCALE);
    CGPathRef shape = CGPathCreateWithRoundedRect(bar, 22 * SCALE, 22 * SCALE, NULL);
    CGContextAddPath(context, shape); CGContextFillPath(context); CGPathRelease(shape);
    NSGraphicsContext *graphics = [NSGraphicsContext graphicsContextWithCGContext:context flipped:NO];
    [NSGraphicsContext setCurrentContext:graphics];
    NSDictionary *attributes = @{NSFontAttributeName: [NSFont systemFontOfSize:15 * SCALE], NSForegroundColorAttributeName: light ? [NSColor colorWithWhite:0.1 alpha:1] : [NSColor colorWithWhite:0.93 alpha:1]};
    NSArray *lines = @[@"The chat stays readable over the moving background, in every style.", @"I checked the renderer, then ran the full test suite before committing.", @"Everything here is drawn by the app on top of the animated surface."];
    for (NSUInteger i = 0; i < lines.count; i++) [lines[i] drawAtPoint:NSMakePoint(190 * SCALE, (LOGICAL_H - 120 - 30.0 * i) * SCALE) withAttributes:attributes];
    [NSGraphicsContext setCurrentContext:nil];
    CGImageRef image = CGBitmapContextCreateImage(context);
    CGImageDestinationRef destination = CGImageDestinationCreateWithURL((__bridge CFURLRef)[NSURL fileURLWithPath:path], CFSTR("public.png"), 1, NULL);
    CGImageDestinationAddImage(destination, image, NULL);
    CGImageDestinationFinalize(destination);
    CFRelease(destination); CGImageRelease(image); CGContextRelease(context); CGColorSpaceRelease(space);
}

int main(int argc, const char **argv) { @autoreleasepool {
    if (argc < 3) { fprintf(stderr, "Usage: render path/to/fluid.metal out-dir [style] [dark|light] [seconds]\n"); return 1; }
    NSString *source = [NSString stringWithContentsOfFile:@(argv[1]) encoding:NSUTF8StringEncoding error:nil];
    NSString *directory = @(argv[2]);
    [[NSFileManager defaultManager] createDirectoryAtPath:directory withIntermediateDirectories:YES attributes:nil error:nil];
    unsigned first = 2, last = 13;
    if (argc > 3 && atoi(argv[3]) > 0) first = last = atoi(argv[3]);
    bool light = argc > 4 && strcmp(argv[4], "light") == 0;
    double seconds = argc > 5 ? atof(argv[5]) : 4;
    id<MTLDevice> device = MTLCreateSystemDefaultDevice();
    id<MTLCommandQueue> queue = [device newCommandQueue];
    // Neutral Graphite-like accent, matching the app's dark/light tint strengths.
    simd_float4 tint = light ? (simd_float4){0.10, 0.10, 0.12, 0.16} : (simd_float4){0.92, 0.92, 0.95, 0.14};
    for (unsigned style = first; style <= last; style++) {
        void *renderer = del_fluid_create(NULL, 0, source.UTF8String, strlen(source.UTF8String));
        del_fluid_running(renderer, true);
        // Same shape as the real composer and footer: a rounded bar near the bottom.
        del_fluid_region(renderer, 0, 150, LOGICAL_H - 150, LOGICAL_W - 300, 110, 22);
        // DEL_RENDER_MOUSE=sweep moves a cursor; =pull grabs the middle and drags it up and aside;
        // =fast whips it across the window; =click clicks once, shortly before the first shot.
        const char *mode = getenv("DEL_RENDER_MOUSE");
        bool sweep = mode && strcmp(mode, "sweep") == 0, pull = mode && strcmp(mode, "pull") == 0;
        bool fast = mode && strcmp(mode, "fast") == 0, click = mode && strcmp(mode, "click") == 0;
        if (getenv("DEL_RENDER_BLUR")) del_fluid_blurred(renderer, true);
        double start = CACurrentMediaTime(), gpu = 0, worst = 0;
        int frames = 0, shot = 0;
        double next = seconds;
        while (shot < 3) {
            double now = CACurrentMediaTime() - start;
            double mx = -1, my = -1;
            if (sweep) { mx = LOGICAL_W * (0.5 + 0.34 * sin(now * 0.9)); my = LOGICAL_H * (0.42 + 0.20 * sin(now * 1.5)); }
            if (pull) {
                double t = fmin(fmax((now - seconds * 0.45) / 1.0, 0), 1);
                mx = LOGICAL_W * 0.5 + t * 190; my = LOGICAL_H - 300 - t * 200;
                del_fluid_force_press(renderer, now > seconds * 0.4);
            }
            if (fast) { mx = LOGICAL_W * (0.5 + 0.42 * sin(now * 24)); my = LOGICAL_H * (0.45 + 0.30 * sin(now * 17)); }
            if (click) { mx = LOGICAL_W * 0.5; my = LOGICAL_H * 0.32; del_fluid_force_press(renderer, now > seconds - 0.55 && now < seconds - 0.45); }
            CVPixelBufferRef buffer = del_fluid_frame(renderer, LOGICAL_W, LOGICAL_H, SCALE, 1, 12, mx, my, 0, 0, light, style, tint.x, tint.y, tint.z, tint.w, sweep || pull || fast || click);
            id<MTLCommandBuffer> command = [queue commandBuffer];
            del_fluid_prepare(buffer, (__bridge void *)command);
            [command commit];
            [command waitUntilCompleted];
            if (command.status == MTLCommandBufferStatusError) { fprintf(stderr, "GPU error: %s\n", command.error.description.UTF8String); return 2; }
            double cost = command.GPUEndTime - command.GPUStartTime;
            if (frames > 5) { gpu += cost; worst = fmax(worst, cost); }
            frames++;
            if (CACurrentMediaTime() - start >= next) {
                writePNG(buffer, light, tint, [directory stringByAppendingFormat:@"/%s-%s-%d.png", names[style], light ? "light" : "dark", shot]);
                shot++; next += 1.2;
            }
            CFRelease(buffer);
            usleep(14000);
        }
        printf("%-9s %5.2f ms avg  %5.2f ms worst  (%d frames, %dx%d)\n", names[style], gpu / fmax(frames - 6, 1) * 1000, worst * 1000, frames, LOGICAL_W * SCALE, LOGICAL_H * SCALE);
        fflush(stdout);
        del_fluid_release(renderer);
    }
    return 0;
}}
