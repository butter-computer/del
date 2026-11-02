// Run from the repository root:
// clang -fobjc-arc -framework Foundation -framework Metal scripts/test-liquid.m -o /tmp/del-test-liquid
// /tmp/del-test-liquid crates/app/src/fluid.metal
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <simd/simd.h>
#include <assert.h>
#include <math.h>
struct Params { simd_float4 geometry,pointer,impulse,clock,life,composer,footer,blur,tint,animation,stroke; };
int main(int argc,const char **argv) { @autoreleasepool {
    if(argc!=2){fprintf(stderr,"Usage: test-liquid path/to/fluid.metal\n");return 1;}
    id<MTLDevice> device=MTLCreateSystemDefaultDevice();
    NSError *error=nil;
    NSString *source=[NSString stringWithContentsOfFile:@(argv[1]) encoding:NSUTF8StringEncoding error:&error];
    id<MTLLibrary> library=[device newLibraryWithSource:source options:nil error:&error];
    assert(library);
    id<MTLComputePipelineState> pipeline=[device newComputePipelineStateWithFunction:[library newFunctionWithName:@"liquid_step"] error:&error];
    assert(pipeline);
    id<MTLCommandQueue> queue=[device newCommandQueue];
    MTLTextureDescriptor *desc=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRG32Float width:80 height:60 mipmapped:NO];
    desc.storageMode=MTLStorageModeShared;desc.usage=MTLTextureUsageShaderRead|MTLTextureUsageShaderWrite;
    id<MTLTexture> fields[2]={[device newTextureWithDescriptor:desc],[device newTextureWithDescriptor:desc]};
    struct Params u={0};u.geometry=(simd_float4){560,420,1,0};u.clock=(simd_float4){0,0,80,60};u.stroke=(simd_float4){.35,.35,.55,.35};
    float data[80*60*2];double initial=0,late=0,spread=0;
    int index=0;
    for(int frame=0;frame<=720;frame++) {
        u.impulse=(simd_float4){frame>0&&frame<=6?12:0,0,frame==0?1:0,1.0/120};
        id<MTLCommandBuffer> command=[queue commandBuffer];
        id<MTLComputeCommandEncoder> encoder=[command computeCommandEncoder];
        [encoder setComputePipelineState:pipeline];[encoder setBytes:&u length:sizeof(u) atIndex:0];
        [encoder setTexture:fields[index] atIndex:0];[encoder setTexture:fields[1-index] atIndex:1];
        [encoder dispatchThreads:MTLSizeMake(80,60,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];[encoder endEncoding];
        [command commit];[command waitUntilCompleted];assert(command.status!=MTLCommandBufferStatusError);index=1-index;
        if(frame==0||frame==6||frame==120||frame==720) {
            [fields[index] getBytes:data bytesPerRow:80*2*sizeof(float) fromRegion:MTLRegionMake2D(0,0,80,60) mipmapLevel:0];
            double energy=0;
            for(int y=0;y<60;y++)for(int x=0;x<80;x++) {
                float h=data[(y*80+x)*2],v=data[(y*80+x)*2+1];assert(isfinite(h)&&isfinite(v));energy+=h*h;
                if(frame==120&&y>36)spread+=h*h;
            }
            printf("step %d: height energy %.8f\n",frame,energy);
            if(frame==0)assert(energy==0);
            if(frame==6)initial=energy;
            if(frame==720)late=energy;
        }
    }
    assert(initial>.01);assert(spread>.00001);assert(late<initial*.02);
    printf("Wake propagates beyond the stroke, remains finite, and settles (%.3f%% remaining).\n",100*late/initial);
}}
