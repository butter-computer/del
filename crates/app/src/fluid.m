#import <AppKit/AppKit.h>
#import <Metal/Metal.h>
#import <CoreVideo/CoreVideo.h>
#import <QuartzCore/QuartzCore.h>
#import <simd/simd.h>

struct DelFluidParams { simd_float4 geometry, pointer, impulse, clock, life, composer, footer, blur; };
@class DelFluidRenderer;
@interface DelFluidOwner : NSObject
@property(nonatomic,weak) DelFluidRenderer *renderer;
@end
@implementation DelFluidOwner
@end

@interface DelFluidRenderer : NSObject {
    CVPixelBufferRef _buffer;
    CVMetalTextureCacheRef _cache;
    CVMetalTextureRef _surface;
    id<MTLTexture> _cells[2], _highlight[2], _scene, _blurA, _blurB;
    simd_float4 _regions[2];
    simd_float2 _radii;
    NSUInteger _dyeIndex, _simWidth, _simHeight, _generation, _universeSeed;
    struct DelFluidParams _params;
    CFTimeInterval _lastFrame;
    float _time, _generationTime;
    BOOL _running, _clear;
}
@property(nonatomic,strong) NSData *libraryData;
@property(nonatomic,strong) NSString *librarySource;
@property(nonatomic,strong) id<MTLDevice> device;
@property(nonatomic,strong) id<MTLComputePipelineState> clearState, lifeState, blurHorizontalState, blurVerticalState;
@property(nonatomic,strong) id<MTLRenderPipelineState> displayState, compositeState;
@property(nonatomic) simd_float2 origin;
@property(nonatomic,strong) MTLRenderPassDescriptor *displayPass;
- (CVPixelBufferRef)frameWidth:(double)width height:(double)height scale:(double)scale zoom:(double)zoom radius:(double)radius
    mouseX:(double)x mouseY:(double)y light:(BOOL)light;
- (void)setRunning:(BOOL)running;
- (void)region:(NSUInteger)slot x:(double)x y:(double)y width:(double)width height:(double)height radius:(double)radius;
- (void)encode:(id<MTLCommandBuffer>)command buffer:(CVPixelBufferRef)buffer;
@end
// Shared by fluid.m and backdrop.m. A zero-length library means the build host had
// no `xcrun metal`; compile the embedded shader source at runtime instead.
id<MTLLibrary> del_metal_library(id<MTLDevice> device,NSData *libraryBytes,NSString *source,NSError **error) {
    if(libraryBytes.length==0) {
        if(source.length==0)return nil;
        MTLCompileOptions *options=[MTLCompileOptions new];
        return [device newLibraryWithSource:source options:options error:error];
    }
    dispatch_data_t data=dispatch_data_create(libraryBytes.bytes,libraryBytes.length,dispatch_get_main_queue(),^{ (void)libraryBytes; });
    return [device newLibraryWithData:data error:error];
}
@implementation DelFluidRenderer
- (instancetype)init { if((self=[super init])) { _clear=YES;_universeSeed=arc4random_uniform(65536); } return self; }
- (void)dealloc { if(_surface)CFRelease(_surface);if(_cache)CFRelease(_cache);if(_buffer)CFRelease(_buffer); }
- (void)setRunning:(BOOL)running {
    if(_running!=running) { _running=running;_lastFrame=0; }
}
- (void)region:(NSUInteger)slot x:(double)x y:(double)y width:(double)width height:(double)height radius:(double)radius {
    if(slot>1)return;
    _regions[slot]=(simd_float4){x,y,width,height};_radii[slot]=radius;
}
- (CVPixelBufferRef)frameWidth:(double)width height:(double)height scale:(double)scale zoom:(double)zoom radius:(double)radius
    mouseX:(double)x mouseY:(double)y light:(BOOL)light {
    if(width<=0||height<=0)return NULL;
    size_t pixelsWide=(size_t)ceil(width*scale),pixelsHigh=(size_t)ceil(height*scale);
    if(!_buffer||CVPixelBufferGetWidth(_buffer)!=pixelsWide||CVPixelBufferGetHeight(_buffer)!=pixelsHigh) {
        CVPixelBufferRef fresh=NULL;
        NSDictionary *attributes=@{(__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey:@{},
            (__bridge NSString *)kCVPixelBufferMetalCompatibilityKey:@YES};
        if(CVPixelBufferCreate(kCFAllocatorDefault,pixelsWide,pixelsHigh,kCVPixelFormatType_32BGRA,
            (__bridge CFDictionaryRef)attributes,&fresh)!=kCVReturnSuccess)return NULL;
        DelFluidOwner *owner=[DelFluidOwner new];owner.renderer=self;
        CVBufferSetAttachment(fresh,CFSTR("DelFluidOwner"),(__bridge CFTypeRef)owner,kCVAttachmentMode_ShouldNotPropagate);
        if(_surface){CFRelease(_surface);_surface=NULL;}
        if(_buffer)CFRelease(_buffer);_buffer=fresh;
        _clear=YES;_lastFrame=0;
    }
    simd_float2 point={(float)(x/width),(float)(y/height)};
    BOOL inside=x>=0&&y>=0&&x<width&&y<height;
    _params.geometry=(simd_float4){width,height,zoom,radius};
    _params.pointer=(simd_float4){point.x,point.y,point.x,point.y};
    _params.composer=_regions[0];_params.footer=_regions[1];
    _params.composer.x-=self.origin.x;_params.composer.y-=self.origin.y;
    _params.footer.x-=self.origin.x;_params.footer.y-=self.origin.y;
    _params.blur=(simd_float4){_radii.x,_radii.y,scale,0};
    _params.clock.y=light?1:0;
    _params.life.z=(_running&&inside)?1:0;
    // Simulation time is consumed in encode, never in GPUI paint preparation.
    return _buffer;
}
- (BOOL)prepareDevice:(id<MTLDevice>)device {
    if(self.device==device&&self.displayState&&self.compositeState&&self.blurHorizontalState&&self.blurVerticalState)return YES;
    self.device=device;
    NSError *error=nil;
    id<MTLLibrary> library=del_metal_library(device,self.libraryData,self.librarySource,&error);
    if(!library){NSLog(@"Del Metal library: %@",error);return NO;}
#define PIPE(property,name) self.property=[device newComputePipelineStateWithFunction:[library newFunctionWithName:@name] error:&error]; if(!self.property){NSLog(@"Del Metal pipeline: %@",error);return NO;}
    PIPE(clearState,"fluid_clear") PIPE(lifeState,"cellular_life")
    PIPE(blurHorizontalState,"cell_blur_horizontal") PIPE(blurVerticalState,"cell_blur_vertical")
#undef PIPE
    MTLRenderPipelineDescriptor *display=[MTLRenderPipelineDescriptor new];
    display.vertexFunction=[library newFunctionWithName:@"fluid_vertex"];
    display.fragmentFunction=[library newFunctionWithName:@"fluid_fragment"];
    display.colorAttachments[0].pixelFormat=MTLPixelFormatBGRA8Unorm;
    self.displayState=[device newRenderPipelineStateWithDescriptor:display error:&error];
    if(!self.displayState){NSLog(@"Del Metal display: %@",error);return NO;}
    display.fragmentFunction=[library newFunctionWithName:@"cell_composite"];
    self.compositeState=[device newRenderPipelineStateWithDescriptor:display error:&error];
    if(!self.compositeState){NSLog(@"Del Metal composite: %@",error);return NO;}
    _scene=nil;_blurA=nil;_blurB=nil;
    // The surface texture belongs to the old device's cache; recreate both.
    if(_surface){CFRelease(_surface);_surface=NULL;}
    if(_cache)CFRelease(_cache);_cache=NULL;
    if(CVMetalTextureCacheCreate(kCFAllocatorDefault,NULL,device,NULL,&_cache)!=kCVReturnSuccess)return NO;
    self.displayPass=[MTLRenderPassDescriptor renderPassDescriptor];
    self.displayPass.colorAttachments[0].loadAction=MTLLoadActionDontCare;
    self.displayPass.colorAttachments[0].storeAction=MTLStoreActionStore;
    _simWidth=0;_clear=YES;return YES;
}
- (id<MTLTexture>)texture:(MTLPixelFormat)format width:(NSUInteger)width height:(NSUInteger)height {
    MTLTextureDescriptor *descriptor=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:format width:width height:height mipmapped:NO];
    descriptor.storageMode=MTLStorageModePrivate;
    descriptor.usage=MTLTextureUsageShaderRead|MTLTextureUsageShaderWrite;
    return [self.device newTextureWithDescriptor:descriptor];
}
- (void)dispatch:(id<MTLComputeCommandEncoder>)encoder pipeline:(id<MTLComputePipelineState>)pipeline {
    [encoder setComputePipelineState:pipeline];
    [encoder dispatchThreads:MTLSizeMake(_simWidth,_simHeight,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];
    [encoder memoryBarrierWithScope:MTLBarrierScopeTextures];
}
- (void)encode:(id<MTLCommandBuffer>)command buffer:(CVPixelBufferRef)buffer {
    if(buffer!=_buffer||![self prepareDevice:command.commandQueue.device])return;
    float pitch=fmaxf(7.0f*_params.geometry.z,fmaxf(_params.geometry.x/384.0f,_params.geometry.y/256.0f));
    NSUInteger width=MAX(1,(NSUInteger)ceilf(_params.geometry.x/pitch));
    NSUInteger height=MAX(1,(NSUInteger)ceilf(_params.geometry.y/pitch));
    if(width!=_simWidth||height!=_simHeight) {
        _simWidth=width;_simHeight=height;_clear=YES;_dyeIndex=0;_generation=0;_generationTime=0;_lastFrame=0;
        for(int i=0;i<2;i++){
            _cells[i]=[self texture:MTLPixelFormatRGBA16Float width:width height:height];
            _highlight[i]=[self texture:MTLPixelFormatR16Float width:width height:height];
        }
    }
    if(!_surface) {
        NSDictionary *attributes=@{(__bridge NSString *)kCVMetalTextureUsage:@(MTLTextureUsageRenderTarget|MTLTextureUsageShaderRead)};
        if(CVMetalTextureCacheCreateTextureFromImage(kCFAllocatorDefault,_cache,_buffer,(__bridge CFDictionaryRef)attributes,
            MTLPixelFormatBGRA8Unorm,CVPixelBufferGetWidth(_buffer),CVPixelBufferGetHeight(_buffer),0,&_surface)!=kCVReturnSuccess)return;
    }
    CFTimeInterval now=CACurrentMediaTime();
    float dt=_running?(_lastFrame>0?fminf(fmaxf(now-_lastFrame,0),0.05f):1.0f/60.0f):0;
    _lastFrame=now;
    _time+=dt;_generationTime+=dt;
    BOOL generation=_running&&_generationTime>=0.09f;
    if(generation){_generationTime=fmodf(_generationTime,0.09f);_generation++;}
    _params.impulse=(simd_float4){0,0,0,dt};
    _params.clock.x=_time;_params.clock.z=_simWidth;_params.clock.w=_simHeight;
    _params.life.x=generation?1:0;_params.life.y=(float)_generation;_params.life.w=(float)_universeSeed;
    id<MTLComputeCommandEncoder> encoder=[command computeCommandEncoder];
    [encoder setBytes:&_params length:sizeof(_params) atIndex:0];
    if(_clear) {
        [encoder setTexture:_cells[0] atIndex:0];[encoder setTexture:_cells[1] atIndex:1];
        [encoder setTexture:_highlight[0] atIndex:2];[encoder setTexture:_highlight[1] atIndex:3];
        [self dispatch:encoder pipeline:self.clearState];_clear=NO;
    }
    if(_running) {
        [encoder setBytes:&_params length:sizeof(_params) atIndex:0];
        [encoder setTexture:_cells[_dyeIndex] atIndex:0];[encoder setTexture:_cells[1-_dyeIndex] atIndex:1];
        [encoder setTexture:_highlight[_dyeIndex] atIndex:2];[encoder setTexture:_highlight[1-_dyeIndex] atIndex:3];
        [self dispatch:encoder pipeline:self.lifeState];_dyeIndex=1-_dyeIndex;
    }
    [encoder endEncoding];
    // Reusable offscreen scene + two half-resolution Gaussian intermediates.
    // These contain only the ambient animation; GPUI draws composer foreground
    // later. Nothing here samples or blurs text, controls or desktop content.
    NSUInteger outputWidth=CVPixelBufferGetWidth(_buffer),outputHeight=CVPixelBufferGetHeight(_buffer);
    if(!_scene||_scene.width!=outputWidth||_scene.height!=outputHeight) {
        MTLTextureDescriptor *descriptor=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatBGRA8Unorm width:outputWidth height:outputHeight mipmapped:NO];
        descriptor.storageMode=MTLStorageModePrivate;
        descriptor.usage=MTLTextureUsageRenderTarget|MTLTextureUsageShaderRead;
        _scene=[self.device newTextureWithDescriptor:descriptor];
        _blurA=[self texture:MTLPixelFormatRGBA16Float width:(outputWidth+1)/2 height:(outputHeight+1)/2];
        _blurB=[self texture:MTLPixelFormatRGBA16Float width:(outputWidth+1)/2 height:(outputHeight+1)/2];
    }
    MTLRenderPassDescriptor *pass=self.displayPass;
    pass.colorAttachments[0].texture=_scene;
    id<MTLRenderCommandEncoder> render=[command renderCommandEncoderWithDescriptor:pass];
    [render setRenderPipelineState:self.displayState];[render setFragmentTexture:_cells[_dyeIndex] atIndex:0];
    [render setFragmentTexture:_highlight[_dyeIndex] atIndex:1];
    [render setFragmentBytes:&_params length:sizeof(_params) atIndex:0];
    [render drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];[render endEncoding];
    encoder=[command computeCommandEncoder];
    [encoder setBytes:&_params length:sizeof(_params) atIndex:0];
    [encoder setComputePipelineState:self.blurHorizontalState];
    [encoder setTexture:_scene atIndex:0];[encoder setTexture:_blurA atIndex:1];
    [encoder dispatchThreads:MTLSizeMake(_blurA.width,_blurA.height,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];
    [encoder memoryBarrierWithScope:MTLBarrierScopeTextures];
    [encoder setComputePipelineState:self.blurVerticalState];
    [encoder setTexture:_blurA atIndex:0];[encoder setTexture:_blurB atIndex:1];
    [encoder dispatchThreads:MTLSizeMake(_blurB.width,_blurB.height,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];
    [encoder endEncoding];
    pass.colorAttachments[0].texture=CVMetalTextureGetTexture(_surface);
    render=[command renderCommandEncoderWithDescriptor:pass];
    [render setRenderPipelineState:self.compositeState];
    [render setFragmentTexture:_scene atIndex:0];[render setFragmentTexture:_blurB atIndex:1];
    [render setFragmentBytes:&_params length:sizeof(_params) atIndex:0];
    [render drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];[render endEncoding];
    // GPUI continues encoding/compositing on this SAME command buffer. No commit,
    // CPU wait, staging copies, readbacks, or second presentation queue here.
}
@end
void *del_fluid_create(const unsigned char *bytes,size_t length,const char *source,size_t sourceLength) {
    DelFluidRenderer *renderer=[DelFluidRenderer new];renderer.libraryData=[NSData dataWithBytes:bytes length:length];
    if(source&&sourceLength)renderer.librarySource=[[NSString alloc] initWithBytes:source length:sourceLength encoding:NSUTF8StringEncoding];
    return (__bridge_retained void *)renderer;
}
void del_fluid_release(void *pointer) { if(pointer) { DelFluidRenderer *renderer=(__bridge_transfer DelFluidRenderer *)pointer;(void)renderer; } }
void del_fluid_running(void *pointer,bool running) { if(pointer)[(__bridge DelFluidRenderer *)pointer setRunning:running]; }
void *del_fluid_frame(void *pointer,double width,double height,double scale,double zoom,double radius,double x,double y,double originX,double originY,bool light) {
    if(!pointer)return NULL;
    DelFluidRenderer *renderer=(__bridge DelFluidRenderer *)pointer;renderer.origin=(simd_float2){originX,originY};
    CVPixelBufferRef buffer=[renderer frameWidth:width height:height scale:scale zoom:zoom radius:radius mouseX:x mouseY:y light:light];
    return buffer?(void *)CFRetain(buffer):NULL;
}
void del_fluid_region(void *pointer,unsigned int slot,double x,double y,double width,double height,double radius) {
    if(pointer)[(__bridge DelFluidRenderer *)pointer region:slot x:x y:y width:width height:height radius:radius];
}
void del_fluid_prepare(const void *buffer,void *command) {
    if(!buffer||!command)return;
    DelFluidOwner *owner=(__bridge DelFluidOwner *)CVBufferGetAttachment((CVBufferRef)buffer,CFSTR("DelFluidOwner"),NULL);
    [owner.renderer encode:(__bridge id<MTLCommandBuffer>)command buffer:(CVPixelBufferRef)buffer];
}
