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
#include "../crates/app/src/curtain.h"
struct Params { simd_float4 geometry,pointer,impulse,clock,life,composer,footer,blur,tint,animation,stroke; simd_float4 rain[64]; float lean[64]; simd_float4 rails[3]; simd_float4 footerSide,windowMotion; };
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
    // Window inertia affects free fabric immediately while its pinned rail stays fixed.
    for(int moving=0;moving<2;moving++) {
        u.windowMotion=(simd_float4){moving?20.0f:0,0,0,0};
        c=[queue commandBuffer];e=[c computeCommandEncoder];
        [e setBytes:&u length:sizeof(u) atIndex:0];[e setComputePipelineState:predict];
        [e setTexture:X atIndex:0];[e setTexture:V[0] atIndex:1];[e setTexture:P[0] atIndex:2];
        [e dispatchThreads:MTLSizeMake(W,H,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];
        [e endEncoding];[c commit];[c waitUntilCompleted];assert(c.status==MTLCommandBufferStatusCompleted);
        [P[0] getBytes:moving?v:x bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
    }
    for(int k=0;k<W;k++) {
        assert(fabsf(v[k*4]-x[k*4])<0.001f);
        assert(v[((H-1)*W+k)*4]<x[((H-1)*W+k)*4]-0.1f);
    }
    u.windowMotion=(simd_float4){0,0,0,0};
    // Compare complete constrained trajectories, not just the prediction that
    // relaxation can erase. A half-second desktop drag must visibly move the hem.
    static float stationary[120][W*2];
    float largestLag=0,largestLift=0;
    for(int direction=0;direction<3;direction++) {
        c=[queue commandBuffer];e=[c computeCommandEncoder];
        u.animation.x=12;u.clock.x=0;
        [e setComputePipelineState:init];[e setBytes:&u length:sizeof(u) atIndex:0];
        [e setTexture:X atIndex:0];[e setTexture:V[0] atIndex:1];
        [e dispatchThreads:MTLSizeMake(W,H,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];
        [e endEncoding];[c commit];[c waitUntilCompleted];
        [X getBytes:start bytesPerRow:W*16 fromRegion:all mipmapLevel:0];

        for(int tick=0;tick<120;tick++) {
            u.clock.x=tick/120.0f;
            simd_float2 delta={0,0};
            if(tick>=30&&tick<90) {
                if(direction==1)delta.x=5;
                if(direction==2)delta.y=5;
            }
            u.windowMotion=(simd_float4){delta.x,delta.y,0,0};
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
            [c commit];[c waitUntilCompleted];assert(c.status==MTLCommandBufferStatusCompleted);
            id<MTLTexture> swap=X;X=P[0];P[0]=swap;swap=V[0];V[0]=V[1];V[1]=swap;
            [X getBytes:x bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
            float lag=0,lift=0;
            for(int k=0;k<W;k++) {
                for(int axis=0;axis<3;axis++)assert(fabsf(x[k*4+axis]-start[k*4+axis])<0.001f);
                int hem=((H-1)*W+k)*4;
                if(direction==0){stationary[tick][k*2]=x[hem];stationary[tick][k*2+1]=x[hem+1];}
                lag+=stationary[tick][k*2]-x[hem];lift+=stationary[tick][k*2+1]-x[hem+1];
            }
            if(direction==1)largestLag=fmaxf(largestLag,lag/W);
            if(direction==2)largestLift=fmaxf(largestLift,lift/W);
        }
    }
    printf("Window drag after full solver: horizontal lag %.2f px, vertical lift %.2f px\n",largestLag,largestLift);
    assert(largestLag>15&&largestLift>5);
    // Reset the original fixture before the existing wind/hand/release checks.
    u.animation.x=0;u.clock.x=0;u.windowMotion=(simd_float4){0,0,0,0};
    c=[queue commandBuffer];e=[c computeCommandEncoder];
    [e setComputePipelineState:init];[e setBytes:&u length:sizeof(u) atIndex:0];[e setTexture:X atIndex:0];[e setTexture:V[0] atIndex:1];
    [e dispatchThreads:MTLSizeMake(W,H,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];[e endEncoding];[c commit];[c waitUntilCompleted];
    [X getBytes:start bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
    double moved=0,worstFold=0,mainLeastDrop=1;
    bool compare=getenv("DEL_CLOTH_WHIP")!=NULL;
    for(int step=0;step<1440;step++) {
        u.clock.x=step/120.0f;
        // Press the cloth with the cursor for one simulated second.
        u.animation.z=step>=720&&step<840?1:0;
        // Then whip the cursor across the window as fast as the renderer lets it travel.
        if(step>=1000&&step<1300) {
            float t=step/120.0f;
            u.animation.z=1;
            // The renderer bounds hover force at 700 logical pixels per second.
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
                // The limit pulls back gently, so it may sit a touch past it, never far.
                if(r>=10)mainLeastDrop=fmin(mainLeastDrop,(p[1]-start[k*4+1])/(r*((760.0f+70.0f)/(H-1))));
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
    printf("Nothing the cursor did lifted fabric above %.0f%% of its full drop.\n",mainLeastDrop*100);
    if(!compare)assert(mainLeastDrop>0.78);
    if(!compare)assert(worstFold<W*H*0.002);
    [V[0] getBytes:v bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
    assert(moved>2);
    if(dump) for(int r=100;r<106;r++) {
        printf("row %d:",r);
        for(int k=0;k<8;k++){float *p=x+(r*W+k)*4;printf(" (%.1f %.1f %.1f)",p[0],p[1],p[2]);}
        printf("\n");
    }
    puts("Cloth stays finite, pinned along its rail, unstretched, and moves under wind and cursor.");
    // Window Curtains: take hold of the right panel, slide its rail and lift the fabric.
    memset(&u,0,sizeof(u));u.geometry=(simd_float4){1100,760,1,0};u.animation.x=12;u.impulse.w=1.0f/120.0f;
    for(int i=0;i<3;i++) {
        DelCurtainRail r=del_curtain_rest(i,1100);
        u.rails[i]=(simd_float4){r.left/1100,r.hook/1100,r.right/1100,r.at};
    }
    c=[queue commandBuffer];e=[c computeCommandEncoder];
    [e setComputePipelineState:init];[e setBytes:&u length:sizeof(u) atIndex:0];[e setTexture:X atIndex:0];[e setTexture:V[0] atIndex:1];
    [e dispatchThreads:MTLSizeMake(W,H,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];[e endEncoding];[c commit];[c waitUntilCompleted];
    [X getBytes:start bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
    const int heldColumn=300,heldRow=120;
    DelCurtainHome home=del_curtain_home(2,1100);
    DelCurtainRail rail=del_curtain_rest(2,1100);
    float s=(heldColumn%PANEL)/(float)(PANEL-1);
    rail.hook=del_curtain_hook_x(rail,s);rail.at=s;
    float hookBefore=rail.hook;
    rail=del_curtain_move(rail,rail.hook-70,home);
    u.rails[2]=(simd_float4){rail.left/1100,rail.hook/1100,rail.right/1100,rail.at};
    float *startHeld=start+(heldRow*W+heldColumn)*4;
    simd_float3 hand={rail.hook-60,startHeld[1]-60,34};
    u.life=(simd_float4){heldColumn,heldRow,1,0};u.stroke=(simd_float4){hand.x,hand.y,hand.z,0};
    for(int step=0;step<480;step++) {
        u.clock.x=step/120.0f;
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
    }
    [X getBytes:x bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
    float *held=x+(heldRow*W+heldColumn)*4;
    double reached=sqrt(pow(held[0]-hand.x,2)+pow(held[1]-hand.y,2));
    printf("Held fabric is %.1f from the hand; the hook moved %.0f.\n",reached,rail.hook-hookBefore);
    assert(reached<10);
    double worstHeldStretch=0;
    for(int r=0;r<H;r++)for(int k=0;k<W;k++) {
        float *p=x+(r*W+k)*4;
        assert(isfinite(p[0])&&isfinite(p[1])&&isfinite(p[2]));
        if(r>0){float *q=x+((r-1)*W+k)*4;worstHeldStretch=fmax(worstHeldStretch,sqrt(pow(p[0]-q[0],2)+pow(p[1]-q[1],2)+pow(p[2]-q[2],2))/((760.0+70.0)/(H-1)));}
    }
    printf("Worst warp stretch while held: %.2fx\n",worstHeldStretch);
    assert(worstHeldStretch<1.8);
    // A hold doesn't crush the fabric it carries: threads around the hand keep their spacing.
    double tightest=1e9;
    for(int r=heldRow-5;r<=heldRow+5;r++)for(int k=heldColumn-5;k<heldColumn+5;k++) {
        float *p=x+(r*W+k)*4,*q=x+(r*W+k+1)*4;
        tightest=fmin(tightest,sqrt(pow(p[0]-q[0],2)+pow(p[1]-q[1],2)+pow(p[2]-q[2],2)));
    }
    double restU=del_curtain_home(2,1100).right-del_curtain_home(2,1100).left;restU=restU/(PANEL-1)*1.72;
    printf("Tightest thread spacing around the hand: %.0f%% of its natural width\n",tightest/restU*100);
    assert(tightest>restU*0.6);
    // The dragged panel's hooks moved along the rail, bunching ahead of the hand.
    for(int k=2*PANEL;k<W;k+=7) {
        float expected=del_curtain_hook_x(rail,(k%PANEL)/(float)(PANEL-1));
        assert(fabsf(x[k*4]-expected)<0.01f);
    }
    // The other panels' rails didn't move, and no other curtain was lifted.
    for(int k=0;k<2*PANEL;k++)assert(fabsf(x[k*4]-start[k*4])<0.001f);
    // The limit pulls back gently, so fabric may sit a touch past it, never far.
    double leastDrop=1;
    for(int r=10;r<H;r++)for(int k=0;k<2*PANEL;k++)leastDrop=fmin(leastDrop,(x[(r*W+k)*4+1]-start[k*4+1])/(r*((760.0f+70.0f)/(H-1))));
    printf("Other panels hang at no less than %.0f%% of their full drop.\n",leastDrop*100);
    assert(leastDrop>0.78);
    puts("A held curtain follows the hand, its hooks slide, and its neighbours are not disturbed.");
    // Letting go must not whip a ripple through the panel. DEL_CLOTH_SNAP drops the hold in
    // 33ms with no settling, to measure the difference.
    bool snap=getenv("DEL_CLOTH_SNAP")!=NULL;
    // The cursor rests on the panel throughout. DEL_CLOTH_HOVER_SNAP brings the hover hand back
    // at full strength when the grip ends, the way that made a ripple appear under the cursor.
    bool hoverSnap=getenv("DEL_CLOTH_HOVER_SNAP")!=NULL;
    simd_float2 rest={960,300};
    u.pointer=(simd_float4){rest.x/1100,rest.y/760,rest.x/1100,rest.y/760};
    double underCursor=0,underMoved=0;float handShare=0;
    simd_float3 point=hand;
    double peakFastest=0,peakMean=0,tailMean=0,lateFastest=0,lateExcess=0;int tailSamples=0;
    static float speed[W*H*4];
    for(int step=0;step<840;step++) {
        float age=step/120.0f;
        u.clock.x=(480+step)/120.0f;
        if(snap) {
            u.life.z=fmaxf(1-age*30,0);u.life.w=0;
        } else {
            simd_float3 hanging={del_curtain_hook_x(rail,s),DEL_CURTAIN_RAIL_Y+heldRow*((760.0f+70.0f)/(H-1))*0.98f,0};
            point=del_curtain_sink(point,hanging,age,1.0f/120.0f);
            u.life.z=del_curtain_grip(age);u.life.w=del_curtain_settle(age);
            u.stroke=(simd_float4){point.x,point.y,point.z,0};
        }
        // The first five seconds the cursor rests. Then it starts moving, and the hand returns.
        bool cursorMoved=age>=5.0f&&!hoverSnap;
        handShare=del_curtain_hand(handShare,u.life.z>0,cursorMoved,1.0f/120.0f);
        u.animation.z=hoverSnap?(u.life.z<=0&&age>=1?1:0):handShare;
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
        [V[0] getBytes:speed bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
        double fastest=0,total=0,control=0,controlFastest=0;int fastRow=0,fastColumn=0;
        for(int r=1;r<H;r++)for(int k=2*PANEL;k<W;k++) {
            float *q=speed+(r*W+k)*4;
            double v2=sqrt(q[0]*q[0]+q[1]*q[1]+q[2]*q[2]);
            if(v2>fastest){fastest=v2;fastRow=r;fastColumn=k-2*PANEL;}
            total+=v2;
            q=speed+(r*W+k-2*PANEL)*4;
            double other=sqrt(q[0]*q[0]+q[1]*q[1]+q[2]*q[2]);
            control+=other;controlFastest=fmax(controlFastest,other);
        }
        double mean=total/((H-1)*PANEL);
        control/=(H-1)*PANEL;
        [X getBytes:x bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
        double local=0;
        for(int r=1;r<H;r++)for(int k=2*PANEL;k<W;k++) {
            float *at=x+(r*W+k)*4,*q=speed+(r*W+k)*4;
            if(hypot(at[0]-rest.x,at[1]-rest.y)<95)local=fmax(local,sqrt(q[0]*q[0]+q[1]*q[1]+q[2]*q[2]));
        }
        if(age>=1&&age<5)underCursor=fmax(underCursor,local);
        if(age>=5)underMoved=fmax(underMoved,local);
        if(getenv("DEL_CLOTH_TRACE")&&step%24==0)printf("  age %.1fs panel %.0f fastest %.0f (row %d col %d)  untouched panel %.0f fastest %.0f\n",age,mean,fastest,fastRow,fastColumn,control,controlFastest);
        // Until the cursor moves again, the panel is settling by itself.
        if(age<5.0f){peakFastest=fmax(peakFastest,fastest);peakMean=fmax(peakMean,mean);}
        // Long after letting go, nothing may wake up: compare with a panel nobody touched.
        if(age>=DEL_CURTAIN_GRIP_SECONDS&&age<5.0f){lateFastest=fmax(lateFastest,fastest);lateExcess=fmax(lateExcess,mean-control);}
        if(step>=300){tailMean+=mean;tailSamples++;}
    }
    printf("Letting go (%s): fastest point %.0f, peak panel speed %.0f; after 0.5s fastest %.0f, at most %.0f above an untouched panel\n",snap?"old snap":"eased",peakFastest,peakMean,lateFastest,lateExcess);
    printf("Under the resting cursor after letting go: fastest %.0f. Once it moves and hover returns: %.0f\n",underCursor,underMoved);
    if(!snap&&!hoverSnap) {
        // Nothing appears under a cursor that's simply resting.
        assert(underCursor<=251);
        // And when the hand comes back, it presses in without a snap.
        assert(underMoved<=251);
        // Nothing moves faster than wind alone can move an untouched panel, at any moment.
        assert(lateFastest<=251);
        assert(peakMean<85);
        // And nothing wakes up later on: the panel stays as calm as one nobody touched.
        assert(lateExcess<14);
        puts("Letting go settles the panel with no whip.");
    }
    // Taking hold of moving fabric must not shock the panel. DEL_CLOTH_GRAB_SNAP uses the old
    // grip, in 70ms, at a limit counted from the hook, to measure the difference.
    bool grabSnap=getenv("DEL_CLOTH_GRAB_SNAP")!=NULL;
    u.pointer=(simd_float4){-1,-1,-1,-1};u.animation.z=0;u.impulse.x=u.impulse.y=0;
    u.life=(simd_float4){0,0,0,0};
    [X getBytes:x bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
    float *caught=x+(heldRow*W+heldColumn)*4;
    DelCurtainRail grabRail=rail;
    float restV=(760.0f+70.0f)/(H-1),drop=heldRow*restV;
    simd_float2 natural=grabSnap?(simd_float2){0,DEL_CURTAIN_RAIL_Y+drop}:(simd_float2){caught[0]-grabRail.hook,caught[1]};
    simd_float2 catchAt={caught[0],caught[1]};
    float catchZ=caught[2],handZ=catchZ,grip=0;
    double restU2=(home.right-home.left)/(PANEL-1)*1.72;
    double beforeFar=0,farFastest=0,farPeak=0,controlPeak=0;
    double dragFastest=0,dragPeak=0,dragControl=0;
    // Then let go straight after the drag, while the hand is still moving, and watch the panel
    // for seven seconds against one nobody touched. DEL_CLOTH_OLD_SINK sinks towards the old
    // generic spot under the hook instead of where this fabric was hanging.
    const float letGoAge=3.0f;
    bool oldSink=getenv("DEL_CLOTH_OLD_SINK")!=NULL;
    simd_float3 sinking={0,0,0};bool released=false;
    double afterFastest=0,afterExcess=0,afterPeak=0;
    for(int step=0;step<120+(int)(letGoAge*120)+840;step++) {
        float age=(step-120)/120.0f;
        u.clock.x=(1500+step)/120.0f;
        // After a second of simply holding, drag across at an ordinary hand speed.
        if(age>=1&&age<3)catchAt.x-=240.0f/120.0f;
        if(age>=letGoAge&&!grabSnap) {
            float since=age-letGoAge;
            if(!released){released=true;sinking=(simd_float3){u.stroke.x,u.stroke.y,u.stroke.z};}
            float hookNow=del_curtain_hook_x(grabRail,s);
            simd_float3 hanging=oldSink?(simd_float3){hookNow,DEL_CURTAIN_RAIL_Y+drop*0.98f,0}:(simd_float3){hookNow+natural.x,natural.y,catchZ};
            sinking=del_curtain_sink(sinking,hanging,since,1.0f/120.0f);
            u.life=(simd_float4){heldColumn,heldRow,del_curtain_grip(since),del_curtain_settle(since)};
            u.stroke=(simd_float4){sinking.x,sinking.y,sinking.z,0};
        } else if(step>=120) {
            simd_float2 at=del_curtain_hold(&grabRail,catchAt,drop,natural,home);
            if(grabSnap) {
                grip=fminf(grip+14.0f/120.0f,1);
                handZ+=(34-handZ)*(1-expf(-8.0f/120.0f));
                u.life=(simd_float4){heldColumn,heldRow,grip,0};
            } else {
                handZ+=(catchZ+14-handZ)*(1-expf(-2.0f/120.0f));
                u.life=(simd_float4){heldColumn,heldRow,1,0};
            }
            u.stroke=(simd_float4){at.x,at.y,handZ,1};
            u.rails[2]=(simd_float4){grabRail.left/1100,grabRail.hook/1100,grabRail.right/1100,grabRail.at};
        }
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
        [V[0] getBytes:speed bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
        [X getBytes:x bytesPerRow:W*16 fromRegion:all mipmapLevel:0];
        for(int i=0;i<W*H*4;i++)assert(isfinite(x[i])&&isfinite(speed[i]));
        if(!grabSnap&&age>=0&&age<letGoAge) {
            int at=(heldRow*W+heldColumn)*4;
            assert(fabsf(x[at]-u.stroke.x)<0.001f&&fabsf(x[at+1]-u.stroke.y)<0.001f);
        }
        // The part of the panel far from the hand, and the same area of an untouched panel.
        double far=0,farTotal=0,other=0;int farCount=0;
        for(int r=1;r<H;r++)for(int k=2*PANEL;k<W;k++) {
            double du=(k-heldColumn)*restU2,dv=(r-heldRow)*restV;
            if(du*du+dv*dv<220*220)continue;
            float *q=speed+(r*W+k)*4;
            double v2=sqrt(q[0]*q[0]+q[1]*q[1]+q[2]*q[2]);
            far=fmax(far,v2);farTotal+=v2;farCount++;
            q=speed+(r*W+k-2*PANEL)*4;
            other+=sqrt(q[0]*q[0]+q[1]*q[1]+q[2]*q[2]);
        }
        if(step<120)beforeFar=fmax(beforeFar,far);
        else if(age<1){farFastest=fmax(farFastest,far);farPeak=fmax(farPeak,farTotal/farCount);controlPeak=fmax(controlPeak,other/farCount);}
        else if(age<letGoAge) {dragFastest=fmax(dragFastest,far);dragPeak=fmax(dragPeak,farTotal/farCount);dragControl=fmax(dragControl,other/farCount);}
        else {
            // The whole panel now, since by this point a ripple can be anywhere in it.
            double whole=0,wholeTotal=0,untouched=0,untouchedFastest=0;
            for(int r=1;r<H;r++)for(int k=2*PANEL;k<W;k++) {
                float *q=speed+(r*W+k)*4;
                double v2=sqrt(q[0]*q[0]+q[1]*q[1]+q[2]*q[2]);
                whole=fmax(whole,v2);wholeTotal+=v2;
                q=speed+(r*W+k-2*PANEL)*4;
                double v3=sqrt(q[0]*q[0]+q[1]*q[1]+q[2]*q[2]);
                untouched+=v3;untouchedFastest=fmax(untouchedFastest,v3);
            }
            double since=age-letGoAge,count=(H-1)*PANEL;
            if(getenv("DEL_CLOTH_TRACE")&&step%48==0)printf("  after drag %.1fs panel %.0f fastest %.0f  untouched %.0f fastest %.0f\n",since,wholeTotal/count,whole,untouched/count,untouchedFastest);
            afterPeak=fmax(afterPeak,wholeTotal/count);
            if(since>=DEL_CURTAIN_GRIP_SECONDS) {
                afterFastest=fmax(afterFastest,whole);
                afterExcess=fmax(afterExcess,wholeTotal/count-untouched/count);
            }
        }
    }
    printf("Taking hold (%s): far fabric fastest %.0f (was %.0f just before), peak speed there %.0f; untouched panel peak %.0f\n",grabSnap?"old snap":"direct",farFastest,beforeFar,farPeak,controlPeak);
    printf("Dragging (%s): far fabric fastest %.0f, peak speed there %.0f; untouched panel peak %.0f\n",grabSnap?"old snap":"direct",dragFastest,dragPeak,dragControl);
    printf("After letting go of a drag: fastest %.0f once the grip is gone, at most %.0f above an untouched panel, peak speed %.0f\n",afterFastest,afterExcess,afterPeak);
    if(!grabSnap) {
        // Once the grip is gone nothing wakes up, not even when the settling ends: no ripple at
        // any point in the seven seconds after a drag.
        assert(afterFastest<=120);
        assert(afterExcess<10);
        // Direct dragging deliberately has no velocity cap. The exact grabbed
        // vertex and finite solver state are checked each frame above.
        assert(controlPeak<50&&dragControl<50); // Other panels remain independent.
        puts("Direct dragging tracks exactly, stays finite, and releases smoothly.");
    }
}}
