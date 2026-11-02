// Native GPU integration checks.
// clang -fobjc-arc -Wno-incompatible-pointer-types -Wno-deprecated-declarations \
//   -framework Foundation -framework AppKit -framework Metal -framework CoreVideo \
//   -framework QuartzCore crates/app/src/fluid.m scripts/test-backgrounds.m -o /tmp/del-test-backgrounds
// /tmp/del-test-backgrounds crates/app/src/fluid.metal
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <CoreVideo/CoreVideo.h>
#include <unistd.h>
#include <math.h>
#include <assert.h>
#import <QuartzCore/QuartzCore.h>
void del_fluid_region(void *,unsigned int,double,double,double,double,double);
void *del_fluid_create(const unsigned char *,size_t,const char *,size_t);
void del_fluid_release(void *);
void del_fluid_running(void *,bool);
void *del_fluid_frame(void *,double,double,double,double,double,double,double,double,double,bool,unsigned int,float,float,float,float,bool);
void del_fluid_prepare(const void *,void *);
static uint64_t snapshot(void *renderer,id<MTLCommandQueue> queue,unsigned int style,float strength,bool reactive,float x,float y,uint64_t *alpha) {
    CVPixelBufferRef buffer=del_fluid_frame(renderer,384,256,1,1,12,x,y,0,0,true,style,0.25,0.55,0.8,strength,reactive);
    id<MTLCommandBuffer> command=[queue commandBuffer];del_fluid_prepare(buffer,(__bridge void *)command);
    [command commit];[command waitUntilCompleted];
    CVPixelBufferLockBaseAddress(buffer,kCVPixelBufferLock_ReadOnly);
    unsigned char *bytes=CVPixelBufferGetBaseAddress(buffer);size_t stride=CVPixelBufferGetBytesPerRow(buffer);
    uint64_t hash=1469598103934665603ULL;*alpha=0;
    for(size_t y=0;y<256;y++)for(size_t x=0;x<384;x++){unsigned char a=bytes[y*stride+x*4+3];hash=(hash^a)*1099511628211ULL;*alpha+=a;}
    CVPixelBufferUnlockBaseAddress(buffer,kCVPixelBufferLock_ReadOnly);CFRelease(buffer);return hash;
}
int main(int argc,const char **argv) { @autoreleasepool {
    if(argc!=2){fprintf(stderr,"Usage: test-backgrounds path/to/fluid.metal\n");return 1;}
    NSString *source=[NSString stringWithContentsOfFile:@(argv[1]) encoding:NSUTF8StringEncoding error:nil];
    id<MTLDevice> device=MTLCreateSystemDefaultDevice();
    NSError *error=nil;
    // Compile at the language level of the oldest supported macOS, as a release build does.
    MTLCompileOptions *options=[MTLCompileOptions new];options.languageVersion=MTLLanguageVersion2_3;
    id<MTLLibrary> library=[device newLibraryWithSource:source options:options error:&error];
    if(!library){fprintf(stderr,"Shader compilation failed: %s\n",error.description.UTF8String);return 1;}
    id<MTLCommandQueue> queue=[device newCommandQueue];
    void *renderer=del_fluid_create(NULL,0,source.UTF8String,strlen(source.UTF8String));
    del_fluid_running(renderer,true);
    uint64_t styleHashes[12]={0};
    for(unsigned int style=2;style<=13;style++) {
        uint64_t hashes[4]={0};
        del_fluid_running(renderer,true);
        for(int frame=0;frame<4;frame++) {
            if(frame)usleep(50000);
            if(frame==2)del_fluid_running(renderer,false);
            CVPixelBufferRef buffer=del_fluid_frame(renderer,384,256,1,1,12,(style==3 && frame>0)?145:100,80,0,0,false,style,0.25,0.55,0.8,0.14,true);
            id<MTLCommandBuffer> command=[queue commandBuffer];
            del_fluid_prepare(buffer,(__bridge void *)command);
            [command commit];[command waitUntilCompleted];
            if(command.status==MTLCommandBufferStatusError){fprintf(stderr,"GPU error: %s\n",command.error.description.UTF8String);return 2;}
            CVPixelBufferLockBaseAddress(buffer,kCVPixelBufferLock_ReadOnly);
            unsigned char *bytes=CVPixelBufferGetBaseAddress(buffer);size_t stride=CVPixelBufferGetBytesPerRow(buffer);
            size_t visible=0;uint64_t hash=1469598103934665603ULL;
            for(size_t y=0;y<256;y++)for(size_t x=0;x<384;x++){
                unsigned char *p=bytes+y*stride+x*4;
                hash=(hash^p[3])*1099511628211ULL;
                if(p[3]>0){visible++;if(abs(p[0]-204)>2||abs(p[1]-140)>2||abs(p[2]-64)>2){fprintf(stderr,"Theme tint mismatch style %u\n",style);return 3;}}
            }
            CVPixelBufferUnlockBaseAddress(buffer,kCVPixelBufferLock_ReadOnly);CFRelease(buffer);
            if(visible==0){fprintf(stderr,"Empty output style %u\n",style);return 4;}
            hashes[frame]=hash;
            printf("style %u frame %d: %zu visible pixels\n",style,frame,visible);
        }
        if(hashes[0]==hashes[1]){fprintf(stderr,"No motion style %u\n",style);return 5;}
        if(hashes[2]!=hashes[3]){fprintf(stderr,"Motion continues while paused style %u\n",style);return 7;}
        uint64_t sum=0,oldAlpha=0,newAlpha=0;
        uint64_t off=snapshot(renderer,queue,style,0.14,false,100,80,&sum);
        // Simulated styles only answer the cursor while running; test-cloth and test-liquid
        // cover those. Styles whose reaction is drawn directly must respond at once.
        bool drawn=style==2||style==3||style==6;
        if(drawn && off==hashes[3]){fprintf(stderr,"No reactive effect style %u\n",style);return 8;}
        if(off!=snapshot(renderer,queue,style,0.14,false,310,190,&sum)){fprintf(stderr,"Pointer affects disabled style %u\n",style);return 9;}
        snapshot(renderer,queue,style,0.10,false,100,80,&oldAlpha);
        snapshot(renderer,queue,style,0.16,false,100,80,&newAlpha);
        if(newAlpha<oldAlpha*1.45){fprintf(stderr,"Light visibility did not increase style %u\n",style);return 10;}
        printf("style %u: disabled cursor is stable; light alpha %.2fx\n",style,(double)newAlpha/oldAlpha);
        styleHashes[style-2]=hashes[0];
    }
    for(int i=0;i<12;i++)for(int j=i+1;j<12;j++)if(styleHashes[i]==styleHashes[j])return 6;
    del_fluid_release(renderer);
    puts("All twelve styles compile for macOS 11, render distinct frames, animate, pause, and use the supplied theme tint.");
    return 0;
}}
