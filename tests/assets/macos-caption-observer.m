// Independent WindowServer pixels; no Electron IPC or private NSWindow pointers.
#import <Foundation/Foundation.h>
#import <ApplicationServices/ApplicationServices.h>
#include <dlfcn.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>

int main(int argc, const char **argv) {
  @autoreleasepool {
    if (argc != 4 || !CGPreflightScreenCaptureAccess()) return 77;
    pid_t pid = (pid_t)strtol(argv[1], NULL, 10);
    CGWindowID number = (CGWindowID)strtoul(argv[2], NULL, 10);
    double caption = strtod(argv[3], NULL);
    if (pid <= 0 || !number || caption < 16 || caption > 64) return 77;
    NSArray *windows = CFBridgingRelease(CGWindowListCopyWindowInfo(kCGWindowListOptionIncludingWindow, number));
    if (windows.count != 1 || [windows[0][(__bridge NSString *)kCGWindowOwnerPID] intValue] != pid) return 77;
    typedef CGImageRef (*Capture)(CGRect, CGWindowListOption, CGWindowID, CGWindowImageOption);
    Capture capture = (Capture)dlsym(RTLD_DEFAULT, "CGWindowListCreateImage");
    if (!capture) return 77;
    CGImageRef image = capture(CGRectNull, kCGWindowListOptionIncludingWindow, number,
      kCGWindowImageBoundsIgnoreFraming | kCGWindowImageNominalResolution);
    if (!image) return 77;
    size_t width = CGImageGetWidth(image);
    // Exclude traffic lights, borders and renderer content. Image coordinates are top-left.
    CGRect region = CGRectMake(90, 3, width > 110 ? width - 110 : 0, caption - 6);
    CGImageRef crop = width > 180 && CGImageGetHeight(image) > caption ? CGImageCreateWithImageInRect(image, region) : NULL;
    CGImageRelease(image);
    if (!crop) return 77;
    width = CGImageGetWidth(crop);
    size_t height = CGImageGetHeight(crop);
    if (!width || width > 2048 || !height || height > 64) { CGImageRelease(crop); return 77; }
    size_t length = width * height * 4;
    uint8_t *bytes = calloc(length, 1);
    CGColorSpaceRef space = CGColorSpaceCreateDeviceRGB();
    CGContextRef context = bytes && space ? CGBitmapContextCreate(bytes, width, height, 8, width * 4, space,
      kCGImageAlphaPremultipliedLast | kCGBitmapByteOrder32Big) : NULL;
    if (!context) {
      if (space) CGColorSpaceRelease(space);
      free(bytes); CGImageRelease(crop); return 77;
    }
    CGContextDrawImage(context, CGRectMake(0, 0, width, height), crop);
    uint64_t signature = UINT64_C(14695981039346656037);
    for (size_t i = 0; i < length; ++i) { signature ^= bytes[i]; signature *= UINT64_C(1099511628211); }
    printf("{\"signature\":\"%016llx\",\"width\":%zu,\"height\":%zu}", (unsigned long long)signature, width, height);
    CGContextRelease(context); CGColorSpaceRelease(space); CGImageRelease(crop); free(bytes);
    return 0;
  }
}
