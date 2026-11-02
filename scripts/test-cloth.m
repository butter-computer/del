// Runs the real cloth solver headless and checks the physics, not the pixels.
// clang -fobjc-arc -framework Foundation -framework Metal scripts/test-cloth.m -o /tmp/del-test-cloth
// /tmp/del-test-cloth crates/app/src/fluid.metal [--dump]
#import <Foundation/Foundation.h>
#import <Metal/Metal.h>
#import <simd/simd.h>
#include <assert.h>
#include <math.h>
#include <stdlib.h>
#include <string.h>
struct Params { simd_float4 geometry,pointer,impulse,clock,life,composer,footer,blur,tint,animation,stroke; simd_float4 rain[64]; float lean[64]; };
enum { W=360, H=180, ITER=20, PANEL=120 };
static id<MTLComputePipelineState> pipeline(id<MTLDevice> device,id<MTLLibrary> library,NSString *name) {
    NSError *error=nil;
    id<MTLComputePipelineState> state=[device newComputePipelineStateWithFunction:[library newFunctionWithName:name] error:&error];
    assert(state);
    return state;
}
int main(int argc,const char **argv) { @autoreleasepool {
    if(argc<2){fprintf(stderr,"Usage: test-cloth path/to/fluid.metal [--dump]\n");return 1;}
    bool dump=argc>2&&strcmp(argv[2],"--dump")==0;
    id<MTLDevice> device=MTLCreateSystemDefaultDevice();
    NSError *error=nil;
    NSString *source=[NSString stringWithContentsOfFile:@(argv[1]) encoding:NSUTF8StringEncoding error:&error];
    id<MTLLibrary> library=[device newLibraryWithSource:source options:nil error:&error];
    if(!library){fprintf(stderr,"%s\n",error.description.UTF8String);return 1;}
    id<MTLComputePipelineState> init=pipeline(device,library,@"cloth_init"),predict=pipeline(device,library,@"cloth_predict"),relax=pipeline(device,library,@"cloth_relax"),commit=pipeline(device,library,@"cloth_commit");
    id<MTLCommandQueue> queue=[device newCommandQueue];
    MTLTextureDescriptor *d=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA32Float width:W height:H mipmapped:NO];
    d.storageMode=MTLStorageModeShared;d.usage=MTLTextureUsageShaderRead|MTLTextureUsageShaderWrite;
    id<MTLTexture> X=[device newTextureWithDescriptor:d],P[2]={[device newTextureWithDescriptor:d],[device newTextureWithDescriptor:d]},V[2]={[device newTextureWithDescriptor:d],[device newTextureWithDescriptor:d]};
    struct Params u={0};u.geometry=(simd_float4){1100,760,1,0};u.pointer=(simd_float4){.5,.5,.5,.5};
    id<MTLCommandBuffer> c=[queue commandBuffer];id<MTLComputeCommandEncoder> e=[c computeCommandEncoder];
    [e setComputePipelineState:init];[e setBytes:&u length:sizeof(u) atIndex:0];[e setTexture:X atIndex:0];[e setTexture:V[0] atIndex:1];
    [e dispatchThreads:MTLSizeMake(W,H,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];[e endEncoding];[c commit];[c waitUntilCompleted];
    static float x[W*H*4],start[W*H*4],v[W*H*4];
    MTLRegion all=MTLRegionMake2D(0,0,W,H);
    [X getBytes:start bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
    u.impulse.w=1.0f/120.0f;
    double moved=0,worstFold=0;
    bool compare=getenv("DEL_CLOTH_WHIP")!=NULL;
    for(int step=0;step<1440;step++) {
        u.clock.x=step/120.0f;
        // Press the cloth with the cursor for one simulated second.
        u.animation.z=step>=720&&step<840?1:0;
        // Then whip the cursor across the window as fast as the renderer lets it travel.
        if(step>=1000&&step<1300) {
            float t=step/120.0f;
            u.animation.z=1;
            // The renderer caps the hand at 700 per second, so a real flick arrives like this.
            // DEL_CLOTH_WHIP scales it, to compare with an uncapped solver.
            float whip=getenv("DEL_CLOTH_WHIP")?atof(getenv("DEL_CLOTH_WHIP")):1;
            float rate=2.0f*whip;
            u.pointer.z=0.5f+0.30f*sinf(t*rate);u.pointer.w=0.5f+0.15f*sinf(t*rate*1.4f+1);
            u.impulse.x=660*whip*cosf(t*rate);u.impulse.y=320*whip*cosf(t*rate*1.4f+1);
        } else {u.pointer.z=u.pointer.w=0.5f;u.impulse.x=u.impulse.y=0;}
        c=[queue commandBuffer];e=[c computeCommandEncoder];
        [e setBytes:&u length:sizeof(u) atIndex:0];
        [e setComputePipelineState:predict];[e setTexture:X atIndex:0];[e setTexture:V[0] atIndex:1];[e setTexture:P[0] atIndex:2];
        [e dispatchThreads:MTLSizeMake(W,H,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];[e memoryBarrierWithScope:MTLBarrierScopeTextures];
        [e setComputePipelineState:relax];
        for(int i=0;i<ITER;i++) {
            [e setTexture:P[i&1] atIndex:0];[e setTexture:P[1-(i&1)] atIndex:1];
            [e dispatchThreads:MTLSizeMake(W,H,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];[e memoryBarrierWithScope:MTLBarrierScopeTextures];
        }
        [e setComputePipelineState:commit];[e setTexture:X atIndex:0];[e setTexture:P[0] atIndex:1];[e setTexture:V[1] atIndex:2];
        [e dispatchThreads:MTLSizeMake(W,H,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];[e endEncoding];
        [c commit];[c waitUntilCompleted];assert(c.status!=MTLCommandBufferStatusError);
        id<MTLTexture> t=X;X=P[0];P[0]=t;t=V[0];V[0]=V[1];V[1]=t;
        if(step==839) {
            // The cursor pressed the fabric at the centre of the window: it must have given way.
            [X getBytes:x bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
            int pressed=0;
            for(int r=0;r<H;r++)for(int k=0;k<W;k++) {
                float *p=x+(r*W+k)*4;
                if(hypot(p[0]-550,p[1]-380)<40){pressed++;assert(p[2]<=-30);}
            }
            assert(pressed>0);
        }
        if(step>=1000&&step<1300&&step%6==0) {
            // Fabric that has folded back over itself: a thread running upwards on screen.
            [X getBytes:x bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
            int backwards=0;
            for(int r=1;r<H;r++)for(int k=0;k<W;k++)if(x[(r*W+k)*4+1]<x[((r-1)*W+k)*4+1]-0.01f)backwards++;
            worstFold=fmax(worstFold,backwards);
        }
        if(step%240==239) {
            [X getBytes:x bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
            double total=0,worstStretch=0,furthest=0,nearLimit=0;
            for(int r=0;r<H;r++)for(int k=0;k<W;k++) {
                float *p=x+(r*W+k)*4;
                assert(isfinite(p[0])&&isfinite(p[1])&&isfinite(p[2]));
                if(r==0)for(int a=0;a<3;a++)assert(fabsf(p[a]-start[(k)*4+a])<0.001f); // The rail never moves.
                total+=fabs(p[2]-start[(r*W+k)*4+2]);
                // No point ever swings beyond the limit, wherever the cursor has been.
                float sway=70.0f+r*((760.0f+70.0f)/(H-1))*0.20f;
                double off=fmax(fabs(p[0]-start[k*4]),fabs(p[2]-start[k*4+2]));
                if(!compare)assert(off<=sway+0.01f);
                // Nothing can lift the fabric: every point stays well below its pin.
                if(!compare)assert(p[1]>=start[k*4+1]+r*((760.0f+70.0f)/(H-1))*0.84f-0.01f);
                furthest=fmax(furthest,off);
                if(off>sway*0.98f)nearLimit+=1;
                if(r>0) {
                    float *q=x+((r-1)*W+k)*4;
                    double len=sqrt(pow(p[0]-q[0],2)+pow(p[1]-q[1],2)+pow(p[2]-q[2],2));
                    worstStretch=fmax(worstStretch,len/((760.0+70.0)/(H-1)));
                }
            }
            moved=fmax(moved,total/(W*H));
            printf("t=%4.1fs mean depth change %.1f worst warp stretch %.2fx furthest sway %.0f, %.2f%% of cloth at the limit\n",(step+1)/120.0,total/(W*H),worstStretch,furthest,nearLimit*100.0/(W*H));
            if(!compare)assert(worstStretch<1.6);
        }
    }
    printf("Most threads folded back during the cursor whip: %.0f of %d\n",worstFold,W*H);
    if(!compare)assert(worstFold<W*H*0.002);
    [V[0] getBytes:v bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
    assert(moved>2);
    if(dump) for(int r=100;r<106;r++) {
        printf("row %d:",r);
        for(int k=0;k<8;k++){float *p=x+(r*W+k)*4;printf(" (%.1f %.1f %.1f)",p[0],p[1],p[2]);}
        printf("\n");
    }
    puts("Cloth stays finite, pinned along its rail, unstretched, and moves under wind and cursor.");
}}
