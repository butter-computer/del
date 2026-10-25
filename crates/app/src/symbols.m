#import <AppKit/AppKit.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>

// Public AppKit SF Symbols rendered at their configured native point sizes.
// Alpha cropping centers the artwork without enlarging compact symbols to fit
// a square. A common optical scale preserves SF's relative glyph proportions.
char *del_system_symbol_svg(const char *name) {
    @autoreleasepool {
        if (@available(macOS 11.0, *)) {
            NSImage *image = [NSImage imageWithSystemSymbolName:[NSString stringWithUTF8String:name]
                                    accessibilityDescription:nil];
            if (!image) return NULL;
            image = [image imageWithSymbolConfiguration:
                     [NSImageSymbolConfiguration configurationWithPointSize:16.0
                                                                    weight:NSFontWeightRegular
                                                                     scale:NSImageSymbolScaleMedium]];
            if (!image || image.size.width <= 0 || image.size.height <= 0) return NULL;
            const CGFloat rasterScale = 4.0;
            const CGFloat opticalScale = 0.75;
            const CGFloat inkCap = 14.0 * rasterScale;
            const NSInteger side = 64;
            NSInteger width = (NSInteger)ceil(image.size.width * rasterScale);
            NSInteger height = (NSInteger)ceil(image.size.height * rasterScale);
            NSBitmapImageRep *source = [[NSBitmapImageRep alloc]
                initWithBitmapDataPlanes:NULL pixelsWide:width pixelsHigh:height
                bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES isPlanar:NO
                colorSpaceName:NSDeviceRGBColorSpace bytesPerRow:0 bitsPerPixel:0];
            if (!source) return NULL;
            [NSGraphicsContext saveGraphicsState];
            NSGraphicsContext.currentContext = [NSGraphicsContext graphicsContextWithBitmapImageRep:source];
            NSGraphicsContext.currentContext.imageInterpolation = NSImageInterpolationHigh;
            NSRect sourceRect = NSMakeRect(0, 0, width, height);
            [[NSColor clearColor] setFill];
            NSRectFillUsingOperation(sourceRect, NSCompositingOperationCopy);
            [image drawInRect:NSMakeRect(0, 0, image.size.width * rasterScale,
                                         image.size.height * rasterScale)
                    fromRect:NSZeroRect operation:NSCompositingOperationSourceOver fraction:1.0
              respectFlipped:NO hints:nil];
            [[NSColor whiteColor] setFill];
            NSRectFillUsingOperation(sourceRect, NSCompositingOperationSourceIn);
            [NSGraphicsContext restoreGraphicsState];

            NSInteger minX = width, minY = height, maxX = -1, maxY = -1;
            unsigned char *pixels = source.bitmapData;
            for (NSInteger y = 0; y < height; y++) {
                for (NSInteger x = 0; x < width; x++) {
                    if (pixels[y * source.bytesPerRow + x * 4 + 3] < 8) continue;
                    minX = MIN(minX, x); minY = MIN(minY, y);
                    maxX = MAX(maxX, x); maxY = MAX(maxY, y);
                }
            }
            if (maxX < minX || maxY < minY) return NULL;
            CGFloat inkWidth = maxX - minX + 1;
            CGFloat inkHeight = maxY - minY + 1;
            CGImageRef cropped = CGImageCreateWithImageInRect(source.CGImage,
                CGRectMake(minX, minY, inkWidth, inkHeight));
            if (!cropped) return NULL;
            NSImage *ink = [[NSImage alloc] initWithCGImage:cropped
                size:NSMakeSize(inkWidth, inkHeight)];
            CGImageRelease(cropped);
            CGFloat scale = MIN(opticalScale, inkCap / MAX(inkWidth, inkHeight));
            NSSize fitted = NSMakeSize(inkWidth * scale, inkHeight * scale);
            NSBitmapImageRep *bitmap = [[NSBitmapImageRep alloc]
                initWithBitmapDataPlanes:NULL pixelsWide:side pixelsHigh:side
                bitsPerSample:8 samplesPerPixel:4 hasAlpha:YES isPlanar:NO
                colorSpaceName:NSDeviceRGBColorSpace bytesPerRow:0 bitsPerPixel:0];
            if (!bitmap) return NULL;
            [NSGraphicsContext saveGraphicsState];
            NSGraphicsContext.currentContext = [NSGraphicsContext graphicsContextWithBitmapImageRep:bitmap];
            NSGraphicsContext.currentContext.imageInterpolation = NSImageInterpolationHigh;
            NSRect canvas = NSMakeRect(0, 0, side, side);
            [[NSColor clearColor] setFill];
            NSRectFillUsingOperation(canvas, NSCompositingOperationCopy);
            [ink drawInRect:NSMakeRect((side-fitted.width)/2, (side-fitted.height)/2,
                                      fitted.width, fitted.height)
                  fromRect:NSZeroRect operation:NSCompositingOperationSourceOver fraction:1.0
            respectFlipped:NO hints:nil];
            [NSGraphicsContext restoreGraphicsState];
            NSData *png = [bitmap representationUsingType:NSBitmapImageFileTypePNG properties:@{}];
            if (!png) return NULL;
            NSString *svg = [NSString stringWithFormat:
                @"<svg xmlns='http://www.w3.org/2000/svg' width='16' height='16' viewBox='0 0 64 64'><image width='64' height='64' href='data:image/png;base64,%@'/></svg>",
                [png base64EncodedStringWithOptions:0]];
            return strdup(svg.UTF8String);
        }
        return NULL;
    }
}

void del_free_system_symbol_svg(char *bytes) { free(bytes); }
