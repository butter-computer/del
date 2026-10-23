// PDFKit and Quick Look own document decoding and interactive scrolling. All calls originate
// from GPUI's UI thread. The bridge retains one view until Rust releases it.
#import <AppKit/AppKit.h>
#import <Quartz/Quartz.h>

void *del_preview_create(void *parentPointer, const char *path) {
    if (![NSThread isMainThread] || !parentPointer || !path) return NULL;
    NSView *parent = (__bridge NSView *)parentPointer;
    NSString *filename = [[NSFileManager defaultManager]
        stringWithFileSystemRepresentation:path length:strlen(path)];
    if (!filename) return NULL;
    NSURL *url = [NSURL fileURLWithPath:filename];
    NSView *preview;
    if ([filename.pathExtension.lowercaseString isEqualToString:@"pdf"]) {
        // PDFKit's continuous layout scrolls through every page without Quick
        // Look's floating previous/next-page controls.
        PDFView *pdf = [[PDFView alloc] initWithFrame:NSZeroRect];
        pdf.displayMode = kPDFDisplaySinglePageContinuous;
        pdf.displayDirection = kPDFDisplayDirectionVertical;
        pdf.backgroundColor = NSColor.clearColor;
        pdf.autoScales = YES;
        pdf.document = [[PDFDocument alloc] initWithURL:url];
        preview = pdf;
    } else {
        QLPreviewView *quickLook = [[QLPreviewView alloc] initWithFrame:NSZeroRect
                                                               style:QLPreviewViewStyleCompact];
        quickLook.shouldCloseWithWindow = NO;
        quickLook.autostarts = NO;
        quickLook.previewItem = (id<QLPreviewItem>)url;
        preview = quickLook;
    }
    preview.hidden = YES;
    [parent addSubview:preview];
    return (__bridge_retained void *)preview;
}

void del_preview_frame(void *pointer, double x, double y, double width, double height) {
    if (![NSThread isMainThread] || !pointer) return;
    NSView *preview = (__bridge NSView *)pointer;
    NSView *parent = preview.superview;
    if (!parent || width <= 0 || height <= 0) { preview.hidden = YES; return; }
    if (!parent.isFlipped) y = NSHeight(parent.bounds) - y - height;
    preview.frame = NSMakeRect(x, y, width, height);
    preview.hidden = NO;
}

void del_preview_hide(void *pointer) {
    if (![NSThread isMainThread] || !pointer) return;
    ((__bridge NSView *)pointer).hidden = YES;
}

void del_preview_release(void *pointer) {
    if (![NSThread isMainThread] || !pointer) return;
    NSView *preview = (__bridge_transfer NSView *)pointer;
    if ([preview isKindOfClass:QLPreviewView.class]) [(QLPreviewView *)preview close];
    [preview removeFromSuperview];
}
