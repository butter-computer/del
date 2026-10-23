#import <AppKit/AppKit.h>

// Development runs have no bundle metadata for the Dock to read.
void del_set_app_icon(const unsigned char *bytes, size_t length) {
    if (![NSThread isMainThread] || !bytes || !length) return;
    NSData *data = [NSData dataWithBytes:bytes length:length];
    NSImage *image = [[NSImage alloc] initWithData:data];
    if (image) NSApplication.sharedApplication.applicationIconImage = image;
}
