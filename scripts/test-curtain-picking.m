// Real queued GPU readbacks: deliberately let an old pick complete while its
// replacement is blocked. This must never publish the replacement as ready.
// clang -fobjc-arc -Wno-deprecated-declarations -framework Foundation -framework AppKit \
//   -framework Metal -framework CoreVideo -framework QuartzCore \
//   scripts/test-curtain-picking.m -o /tmp/del-test-curtain-picking
// /tmp/del-test-curtain-picking crates/app/src/fluid.metal
#import "../crates/app/src/fluid.m"
#include <assert.h>
#include <unistd.h>

@interface DelFluidRenderer (TestParams)
- (struct DelFluidParams)testParams;
- (simd_float2)testHeldTarget;
- (BOOL)testClothHeld;
- (float)testGrabError:(id<MTLCommandQueue>)queue;
@end
@implementation DelFluidRenderer (TestParams)
- (struct DelFluidParams)testParams { return _params; }
- (simd_float2)testHeldTarget { return _heldTarget; }
- (BOOL)testClothHeld { return _clothHeld; }
- (float)testGrabError:(id<MTLCommandQueue>)queue {
    id<MTLBuffer> result=[self.device newBufferWithLength:16 options:MTLResourceStorageModeShared];
    id<MTLCommandBuffer> command=[queue commandBuffer];
    id<MTLBlitCommandEncoder> copy=[command blitCommandEncoder];
    [copy copyFromTexture:_clothX sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(_heldColumn,_heldRow,0)
        sourceSize:MTLSizeMake(1,1,1) toBuffer:result destinationOffset:0 destinationBytesPerRow:16 destinationBytesPerImage:16];
    [copy endEncoding];[command commit];[command waitUntilCompleted];
    assert(command.status==MTLCommandBufferStatusCompleted);
    simd_float4 actual=*(simd_float4 *)result.contents;
    return simd_distance(actual.xyz,_heldPoint);
}
@end

static id<MTLCommandBuffer> frame(void *renderer,id<MTLCommandQueue> queue,id<MTLSharedEvent> gate,
    uint64_t value,unsigned style,bool pressed,bool interactive) {
    del_fluid_force_press(renderer,pressed);
    CVPixelBufferRef buffer=del_fluid_frame(renderer,384,256,1,1,12,70,100,0,0,false,style,.8,.8,.8,.14,interactive);
    assert(buffer);
    id<MTLCommandBuffer> command=[queue commandBuffer];
    if(value)[command encodeWaitForEvent:gate value:value];
    del_fluid_prepare(buffer,(__bridge void *)command);
    [command commit];CFRelease(buffer);return command;
}
static void finish(id<MTLCommandBuffer> command) {
    [command waitUntilCompleted];assert(command.status==MTLCommandBufferStatusCompleted);
}
static void completed(DelClothPick *pick) {
    for(int i=0;i<1000&&!atomic_load(&pick->ready);i++)usleep(1000);
    assert(atomic_load(&pick->ready)&&pick.succeeded);
}
int main(int argc,const char **argv) { @autoreleasepool {
    assert(argc==2);
    NSString *source=[NSString stringWithContentsOfFile:@(argv[1]) encoding:NSUTF8StringEncoding error:nil];
    assert(source);
    id<MTLDevice> device=MTLCreateSystemDefaultDevice();
    id<MTLCommandQueue> queue=[device newCommandQueue];
    id<MTLSharedEvent> gate=[device newSharedEvent];assert(gate);
    void *renderer=del_fluid_create(NULL,0,source.UTF8String,strlen(source.UTF8String));
    DelFluidRenderer *native=(__bridge DelFluidRenderer *)renderer;
    del_fluid_running(renderer,true);
    finish(frame(renderer,queue,gate,0,12,false,true));

    id<MTLCommandBuffer> first=frame(renderer,queue,gate,1,12,true,true);
    DelClothPick *old=[native valueForKey:@"pick"];assert(old&&!atomic_load(&old->ready));
    id<MTLCommandBuffer> release=frame(renderer,queue,gate,0,12,false,true);
    assert(![native valueForKey:@"pick"]);
    id<MTLCommandBuffer> second=frame(renderer,queue,gate,2,12,true,true);
    DelClothPick *current=[native valueForKey:@"pick"];
    assert(current&&current!=old&&current.buffer!=old.buffer);
    gate.signaledValue=1;finish(first);finish(release);completed(old);
    // The old GPU callback ran, but the new readback is still blocked.
    assert(!atomic_load(&current->ready));
    assert([native valueForKey:@"pick"]==current);

    // A style change cancels the current request even if its completion arrives later.
    id<MTLCommandBuffer> change=frame(renderer,queue,gate,0,2,false,true);
    assert(![native valueForKey:@"pick"]);
    gate.signaledValue=2;finish(second);finish(change);completed(current);
    assert(![native valueForKey:@"pick"]);

    // A dialog disables interaction even with a physically held mouse button.
    finish(frame(renderer,queue,gate,0,12,true,false));
    assert(![native valueForKey:@"pick"]);
    id<MTLCommandBuffer> third=frame(renderer,queue,gate,3,12,true,true);
    DelClothPick *cancelled=[native valueForKey:@"pick"];assert(cancelled);
    id<MTLCommandBuffer> modal=frame(renderer,queue,gate,0,12,true,false);
    assert(![native valueForKey:@"pick"]);
    gate.signaledValue=3;finish(third);finish(modal);completed(cancelled);
    assert(![native valueForKey:@"pick"]);
    // Hover follows the new pointer on this very frame, even after a large jump.
    del_fluid_force_press(renderer,false);
    CVPixelBufferRef hovered=del_fluid_frame(renderer,384,256,1,1,12,320,220,0,0,false,12,.8,.8,.8,.14,true);
    id<MTLCommandBuffer> hover=[queue commandBuffer];del_fluid_prepare(hovered,(__bridge void *)hover);
    [hover commit];CFRelease(hovered);finish(hover);
    struct DelFluidParams p=[native testParams];
    assert(fabsf(p.pointer.z-320.0f/384)<0.00001f&&fabsf(p.pointer.w-220.0f/256)<0.00001f);
    assert(p.animation.z==1);
    assert(simd_length(p.impulse.xy)<=700.01f);
    // Window movement uses its desktop origin and also works behind a non-interactive chat.
    del_fluid_window_origin(renderer,20,0);
    finish(frame(renderer,queue,gate,0,12,false,false));p=[native testParams];
    assert(p.windowMotion.x==20&&p.windowMotion.y==0);
    del_fluid_running(renderer,false);del_fluid_window_origin(renderer,1000,500);
    finish(frame(renderer,queue,gate,0,12,false,false));p=[native testParams];
    assert(simd_length(p.windowMotion.xy)==0);
    del_fluid_running(renderer,true);
    finish(frame(renderer,queue,gate,0,12,false,false));p=[native testParams];
    assert(simd_length(p.windowMotion.xy)==0); // No stored movement kick after reactivation.
    // Move a full 100 points before the GPU pick is consumed. It must retain
    // the original fabric and offset, and follow that movement on the first frame.
    finish(frame(renderer,queue,gate,0,12,true,true));
    DelClothPick *dragPick=[native valueForKey:@"pick"];assert(dragPick);completed(dragPick);
    int column=0,row=0;
    assert(del_curtain_pick(dragPick.buffer.contents,360,180,dragPick.cursor,&column,&row));
    const float *caught=(const float *)dragPick.buffer.contents+(row*360+column)*4;
    simd_float2 expected={caught[0]+100,caught[1]};
    CVPixelBufferRef dragged=del_fluid_frame(renderer,384,256,1,1,12,170,100,0,0,false,12,.8,.8,.8,.14,true);
    id<MTLCommandBuffer> drag=[queue commandBuffer];del_fluid_prepare(dragged,(__bridge void *)drag);
    [drag commit];finish(drag);CFRelease(dragged);
    assert([native testClothHeld]);
    assert(simd_distance([native testHeldTarget],expected)<0.001f);
    assert([native testGrabError:queue]<0.001f); // Actual GPU fabric, not just the CPU target.
    for(int i=0;i<8;i++) {
        float x=i%2?30:330;
        CVPixelBufferRef b=del_fluid_frame(renderer,384,256,1,1,12,x,100,0,0,false,12,.8,.8,.8,.14,true);
        id<MTLCommandBuffer> c=[queue commandBuffer];del_fluid_prepare(b,(__bridge void *)c);
        [c commit];finish(c);CFRelease(b);
        assert([native testGrabError:queue]<0.001f);
    }
    finish(frame(renderer,queue,gate,0,12,false,false));
    // A real AppKit window supplies its origin at encode time, without another
    // Rust paint or del_fluid_frame call. This also checks screen Y direction.
    [NSApplication sharedApplication];
    NSWindow *window=[[NSWindow alloc] initWithContentRect:NSMakeRect(100,100,384,256)
        styleMask:NSWindowStyleMaskBorderless backing:NSBackingStoreBuffered defer:NO];
    del_fluid_window_view(renderer,(__bridge void *)window.contentView);
    del_fluid_running(renderer,false);del_fluid_running(renderer,true);
    finish(frame(renderer,queue,gate,0,12,false,false));
    CVPixelBufferRef cached=del_fluid_frame(renderer,384,256,1,1,12,70,100,0,0,false,12,.8,.8,.8,.14,false);
    [window setFrameOrigin:NSMakePoint(140,120)];
    id<MTLCommandBuffer> moved=[queue commandBuffer];del_fluid_prepare(cached,(__bridge void *)moved);
    [moved commit];finish(moved);CFRelease(cached);p=[native testParams];
    assert(p.windowMotion.x==40&&p.windowMotion.y==-20);

    del_fluid_window_view(renderer,NULL);
    del_fluid_release(renderer);
    puts("Picks, immediate hover/drag, native window inertia and inactive reset passed.");
}}
