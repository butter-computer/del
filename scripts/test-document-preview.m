// Native integration check: compile with document_preview.m, AppKit, and Quartz.
#import <AppKit/AppKit.h>
#import <Quartz/Quartz.h>
extern void *del_preview_create(void *, const char *);
extern void del_preview_frame(void *, double, double, double, double);
extern void del_preview_hide(void *);
extern void del_preview_release(void *);

int main(void) {
    @autoreleasepool {
        [NSApplication sharedApplication];
        NSString *folder = [NSTemporaryDirectory() stringByAppendingPathComponent:NSUUID.UUID.UUIDString];
        [[NSFileManager defaultManager] createDirectoryAtPath:folder withIntermediateDirectories:YES attributes:nil error:nil];
        NSString *pdfPath = [folder stringByAppendingPathComponent:@"two pages.pdf"];
        CGRect paper = CGRectMake(0, 0, 612, 792);
        CGContextRef pdf = CGPDFContextCreateWithURL((__bridge CFURLRef)[NSURL fileURLWithPath:pdfPath], &paper, NULL);
        for (int page = 0; page < 2; page++) {
            CGPDFContextBeginPage(pdf, NULL);
            CGContextSetRGBFillColor(pdf, 0.2, 0.4, 0.8, 1);
            CGContextFillRect(pdf, CGRectMake(40, 40, 200 + page * 60, 200));
            CGPDFContextEndPage(pdf);
        }
        CGPDFContextClose(pdf);
        CGContextRelease(pdf);
        PDFDocument *document = [[PDFDocument alloc] initWithURL:[NSURL fileURLWithPath:pdfPath]];
        NSCAssert(document.pageCount == 2, @"multipage fixture");
        NSString *docxPath = [folder stringByAppendingPathComponent:@"formatted document.docx"];
        NSAttributedString *text = [[NSAttributedString alloc] initWithString:@"Document preview\nA formatted Word document.\nSecond paragraph."
            attributes:@{NSFontAttributeName: [NSFont boldSystemFontOfSize:24], NSForegroundColorAttributeName:NSColor.blueColor}];
        NSError *error = nil;
        NSData *docx = [text dataFromRange:NSMakeRange(0, text.length) documentAttributes:@{NSDocumentTypeDocumentAttribute:NSOfficeOpenXMLTextDocumentType} error:&error];
        NSCAssert(docx && !error, @"DOCX fixture");
        [docx writeToFile:docxPath atomically:YES];

        NSWindow *window = [[NSWindow alloc] initWithContentRect:NSMakeRect(0, 0, 900, 700)
            styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
        window.releasedWhenClosed = NO;
        NSView *parent = window.contentView;
        for (NSString *path in @[pdfPath, docxPath]) {
            void *pointer = del_preview_create((__bridge void *)parent, path.fileSystemRepresentation);
            NSCAssert(pointer != NULL, @"creates native view");
            NSView *preview = (__bridge NSView *)pointer;
            NSCAssert(preview.superview == parent, @"embedded in app content view");
            if ([path.pathExtension isEqualToString:@"pdf"]) {
                NSCAssert([preview isKindOfClass:PDFView.class], @"PDF uses scrolling renderer without Quick Look navigation arrows");
                PDFView *pdfView = (PDFView *)preview;
                NSCAssert([pdfView.document.documentURL.path isEqualToString:path], @"loads exact PDF");
                NSCAssert(pdfView.document.pageCount == 2, @"loads every PDF page");
                NSCAssert(pdfView.displayMode == kPDFDisplaySinglePageContinuous, @"continuous page scrolling");
                NSCAssert(pdfView.displayDirection == kPDFDisplayDirectionVertical, @"vertical scrolling");
            } else {
                QLPreviewView *quickLook = (QLPreviewView *)preview;
                NSCAssert([quickLook.previewItem.previewItemURL.path isEqualToString:path], @"loads exact local document");
            }
            del_preview_frame(pointer, 500, 100, 360, 500);
            NSCAssert(!preview.hidden && preview.frame.size.width == 360, @"visible at sidebar bounds");
            NSCAssert(preview.frame.origin.y == 100, @"top-left coordinates converted correctly");
            NSDate *deadline = [NSDate dateWithTimeIntervalSinceNow:1];
            while ([deadline timeIntervalSinceNow] > 0) {
                [[NSRunLoop currentRunLoop] runMode:NSDefaultRunLoopMode beforeDate:[NSDate dateWithTimeIntervalSinceNow:0.02]];
            }
            NSCAssert(preview.subviews.count > 0, @"created document renderer");
            del_preview_hide(pointer);
            NSCAssert(preview.hidden, @"hidden when sidebar is closed or settings opens");
            del_preview_frame(pointer, 500, 100, 300, 450);
            NSCAssert(!preview.hidden && preview.frame.size.width == 300, @"reopen and resize");
            del_preview_release(pointer);
            NSCAssert(![parent.subviews containsObject:preview], @"removes closed view");
        }
        [window close];
        [[NSFileManager defaultManager] removeItemAtPath:folder error:nil];
        puts("PDF and DOCX native preview lifecycle passed");
    }
    return 0;
}
