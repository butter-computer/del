#import <AppKit/AppKit.h>
#import <Metal/Metal.h>
#import <CoreVideo/CoreVideo.h>
#import <QuartzCore/QuartzCore.h>
#import <simd/simd.h>
#import "weather.h"
#import "curtain.h"
#import <stdatomic.h>

struct DelFluidParams { simd_float4 geometry, pointer, impulse, clock, life, composer, footer, blur, tint, animation, stroke; simd_float4 rain[DEL_RAIN_COUNT]; float lean[DEL_RAIN_COUNT]; simd_float4 rails[DEL_CURTAIN_PANELS]; simd_float4 footerSide, windowMotion; };
@class DelFluidRenderer;
@interface DelFluidOwner : NSObject
@property(nonatomic,weak) DelFluidRenderer *renderer;
@end
@implementation DelFluidOwner
@end

// A pick owns its readback until that command completes. Replaced/cancelled picks
// can finish safely without publishing readiness into a newer request.
@interface DelClothPick : NSObject {
@public
    atomic_bool ready;
}
@property(nonatomic,strong) id<MTLBuffer> buffer;
@property(nonatomic) BOOL succeeded;
@property(nonatomic) simd_float2 cursor;
@end
@implementation DelClothPick
- (instancetype)init { if((self=[super init]))atomic_init(&ready,false);return self; }
@end

@interface DelFluidRenderer : NSObject {
    CVPixelBufferRef _buffer;
    CVMetalTextureCacheRef _cache;
    CVMetalTextureRef _surface;
    id<MTLTexture> _cells[2], _highlight[2], _scene, _blurA, _blurB, _liquid[2];
    // Per-style simulation state and geometry targets, dropped when the style changes.
    id<MTLTexture> _clothX, _clothP[2], _clothV[2], _clothDetail, _shadowMap, _particles[2], _puddingX[2], _puddingV[2];
    id<MTLTexture> _contour, _cloudNoise, _cloud[2], _cloudWake[2], _field, _fieldSamples, _fieldDepth;
    id<MTLTexture> _windowScene, _branches, _shafts[2], _sea[2], _seaSurface, _caustics, _strong[2];
    simd_float4 _seaWaves[26];
    BOOL _seaWavesReady, _wasPressed, _pressBegan;
    // A held Window Curtains panel. Rails are stored as shares of the width, so they survive
    // resizing; picking copies the cloth to the CPU once, on the frame the button goes down.
    DelCurtainRail _rails[DEL_CURTAIN_PANELS];
    BOOL _railsReady, _clothHeld, _pickWanted;
    DelClothPick *_pick;
    int _heldColumn, _heldRow;
    simd_float2 _heldOffset, _heldTarget;
    simd_float3 _heldPoint;
    simd_float2 _heldNatural;
    float _heldDepth;
    float _heldFirmness, _heldSettle, _letGoAge, _handShare;
    BOOL _lettingGo, _cursorMoved;
    simd_float2 _letGoAt;
    simd_float2 _seaDrop;
    float _seaDropAge, _seaBusyUntil;
    BOOL _seaDirty;
    DelRunoff _runoff[DEL_RUNOFF_COUNT];
    DelRunoffDroplet _droplets[DEL_RUNOFF_COUNT];
    simd_float4 _regions[3];
    simd_float2 _radii;
    NSUInteger _dyeIndex, _simWidth, _simHeight, _generation, _universeSeed;
    struct DelFluidParams _params;
    CFTimeInterval _lastFrame;
    float _time, _generationTime, _reaction;
    BOOL _pointerInitialized, _liquidPointerValid, _clearLiquid;
    simd_float2 _liquidPointer;
    NSUInteger _liquidIndex, _particleIndex, _puddingIndex, _cloudIndex, _frameCount;
    unsigned int _activeStyle;
    BOOL _clearStyle, _rainReady, _clothDetailDirty, _sceneDirty, _causticsDirty, _blurred, _forcePress, _grabbing, _pressed, _pointerTracked;
    float _blurMix;
    simd_float2 _pointerVelocity, _lastPointer, _grabAnchor;
    simd_float2 _lastWindowOrigin;
    BOOL _windowTracked;
    NSUInteger _seaIndex, _shaftIndex, _wakeIndex;
    float _nextKick, _kickUntil, _plateY;
    simd_float3 _kick;
    NSMutableDictionary *_renderPipelines, *_computePipelines;
    simd_float4 _rainComposer, _rainFooter, _rainSide, _rainGeometry;
    simd_float2 _rainRadii;
    float _rainTime;
    BOOL _running, _clear;
}
@property(nonatomic,strong) NSData *libraryData;
@property(nonatomic,strong) NSString *librarySource;
@property(nonatomic,strong) id<MTLDevice> device;
@property(nonatomic,strong) id<MTLLibrary> library;
@property(nonatomic,strong) id<MTLComputePipelineState> clearState, lifeState, blurHorizontalState, blurVerticalState;
@property(nonatomic,strong) id<MTLRenderPipelineState> displayState, compositeState;
@property(nonatomic) simd_float2 origin;
@property(nonatomic) simd_float2 windowOrigin;
@property(nonatomic,weak) NSView *windowView;
@property(nonatomic,strong) MTLRenderPassDescriptor *displayPass;
- (CVPixelBufferRef)frameWidth:(double)width height:(double)height scale:(double)scale zoom:(double)zoom radius:(double)radius
    mouseX:(double)x mouseY:(double)y light:(BOOL)light style:(unsigned int)style tint:(simd_float4)tint reactive:(BOOL)reactive;
- (void)setRunning:(BOOL)running;
- (void)setBlurred:(BOOL)blurred;
- (void)forcePress:(BOOL)pressed;
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
- (instancetype)init { if((self=[super init])) { _clear=YES;_seaDropAge=1;_handShare=1;_cursorMoved=YES;_universeSeed=arc4random_uniform(65536); } return self; }
- (void)dealloc { if(_surface)CFRelease(_surface);if(_cache)CFRelease(_cache);if(_buffer)CFRelease(_buffer); }
- (void)setBlurred:(BOOL)blurred { _blurred=blurred; }
- (void)forcePress:(BOOL)pressed { _forcePress=pressed; }
- (void)setRunning:(BOOL)running {
    if(_running!=running) { _running=running;_lastFrame=0;_liquidPointerValid=NO;_rainReady=NO;_windowTracked=NO;_pointerTracked=NO; }
}
- (void)region:(NSUInteger)slot x:(double)x y:(double)y width:(double)width height:(double)height radius:(double)radius {
    if(slot>2)return;
    // Slots 1 and 2 are the two groups under the input bar, which share one corner radius.
    _regions[slot]=(simd_float4){x,y,width,height};_radii[slot==0?0:1]=radius;
}
- (CVPixelBufferRef)frameWidth:(double)width height:(double)height scale:(double)scale zoom:(double)zoom radius:(double)radius
    mouseX:(double)x mouseY:(double)y light:(BOOL)light style:(unsigned int)style tint:(simd_float4)tint reactive:(BOOL)reactive {
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
    _params.pointer.x=point.x;_params.pointer.y=point.y;
    if(!_pointerInitialized&&inside) {
        _params.pointer.z=point.x;_params.pointer.w=point.y;_pointerInitialized=YES;
    }
    _params.composer=_regions[0];_params.footer=_regions[1];_params.footerSide=_regions[2];
    _params.composer.x-=self.origin.x;_params.composer.y-=self.origin.y;
    _params.footer.x-=self.origin.x;_params.footer.y-=self.origin.y;
    _params.footerSide.x-=self.origin.x;_params.footerSide.y-=self.origin.y;
    _params.blur=(simd_float4){_radii.x,_radii.y,scale,0};
    _params.clock.y=light?1:0;
    _params.tint=tint;_params.animation.x=style;_params.animation.y=reactive?1:0;
    _params.life.z=(_running&&inside&&reactive)?1:0;
    _pressed=_running&&reactive&&(_forcePress||([NSEvent pressedMouseButtons]&1)!=0);
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
    self.library=library;_renderPipelines=[NSMutableDictionary new];_computePipelines=[NSMutableDictionary new];
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
    _simWidth=0;_clear=YES;[self releaseStyleState];_activeStyle=0;return YES;
}
- (void)releaseStyleState {
    _liquid[0]=_liquid[1]=nil;_clothX=nil;_clothP[0]=_clothP[1]=nil;_clothV[0]=_clothV[1]=nil;_clothDetail=nil;_shadowMap=nil;
    _particles[0]=_particles[1]=nil;_puddingX[0]=_puddingX[1]=nil;_puddingV[0]=_puddingV[1]=nil;
    _contour=nil;_cloudNoise=nil;_cloud[0]=_cloud[1]=nil;_cloudWake[0]=_cloudWake[1]=nil;_field=nil;_fieldSamples=nil;_fieldDepth=nil;
    _windowScene=nil;_branches=nil;_shafts[0]=_shafts[1]=nil;_sea[0]=_sea[1]=nil;_seaSurface=nil;_caustics=nil;_grabbing=NO;_seaDropAge=1;
    _railsReady=NO;_clothHeld=NO;_pickWanted=NO;_pick=nil;_heldFirmness=0;_heldSettle=0;_lettingGo=NO;_handShare=1;_cursorMoved=YES;
    memset(_runoff,0,sizeof(_runoff));memset(_droplets,0,sizeof(_droplets));
    _liquidPointerValid=NO;_clearStyle=YES;
}
// Pipelines are built only for the style in use, so an unused effect costs nothing.
- (id<MTLComputePipelineState>)compute:(NSString *)name {
    id<MTLComputePipelineState> state=_computePipelines[name];
    if(state)return state;
    NSError *error=nil;
    state=[self.device newComputePipelineStateWithFunction:[self.library newFunctionWithName:name] error:&error];
    if(!state){NSLog(@"Del Metal pipeline %@: %@",name,error);return nil;}
    _computePipelines[name]=state;
    return state;
}
typedef NS_ENUM(NSInteger,DelBlend){DelBlendNone,DelBlendAdd,DelBlendOver,DelBlendTransmit};
- (id<MTLRenderPipelineState>)render:(NSString *)vertex fragment:(NSString *)fragment blend:(DelBlend)blend depth:(BOOL)depth {
    NSString *key=[NSString stringWithFormat:@"%@/%@/%ld/%d",vertex,fragment,(long)blend,depth];
    id<MTLRenderPipelineState> state=_renderPipelines[key];
    if(state)return state;
    MTLRenderPipelineDescriptor *descriptor=[MTLRenderPipelineDescriptor new];
    descriptor.vertexFunction=[self.library newFunctionWithName:vertex];
    if(fragment) {
        descriptor.fragmentFunction=[self.library newFunctionWithName:fragment];
        descriptor.colorAttachments[0].pixelFormat=MTLPixelFormatRG16Float;
        descriptor.rasterSampleCount=4;
        if(depth)descriptor.depthAttachmentPixelFormat=MTLPixelFormatDepth32Float;
    } else descriptor.depthAttachmentPixelFormat=MTLPixelFormatDepth32Float;
    MTLRenderPipelineColorAttachmentDescriptor *color=descriptor.colorAttachments[0];
    if(blend!=DelBlendNone) {
        color.blendingEnabled=YES;
        color.sourceRGBBlendFactor=MTLBlendFactorOne;color.sourceAlphaBlendFactor=MTLBlendFactorOne;
        // Transmit: light scattered by this layer, plus what passes through it.
        color.destinationRGBBlendFactor=blend==DelBlendAdd?MTLBlendFactorOne:blend==DelBlendOver?MTLBlendFactorOneMinusSourceAlpha:MTLBlendFactorSourceAlpha;
        color.destinationAlphaBlendFactor=color.destinationRGBBlendFactor;
    }
    NSError *error=nil;
    state=[self.device newRenderPipelineStateWithDescriptor:descriptor error:&error];
    if(!state){NSLog(@"Del Metal pipeline %@: %@",key,error);return nil;}
    _renderPipelines[key]=state;
    return state;
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
- (void)dispatch:(id<MTLComputeCommandEncoder>)encoder pipeline:(id<MTLComputePipelineState>)pipeline width:(NSUInteger)width height:(NSUInteger)height {
    [encoder setComputePipelineState:pipeline];
    [encoder dispatchThreads:MTLSizeMake(width,height,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];
    [encoder memoryBarrierWithScope:MTLBarrierScopeTextures];
}
// Fixed-step cloth: predict with forces, relax constraints, derive velocity from
// what the constraints allowed. Grid size is fixed, so resizing never resets it.
- (void)stepCurtains:(id<MTLComputeCommandEncoder>)encoder dt:(float)dt {
    const NSUInteger width=360,height=180,iterations=20;
    id<MTLComputePipelineState> predict=[self compute:@"cloth_predict"],relax=[self compute:@"cloth_relax"],commit=[self compute:@"cloth_commit"];
    if(!_clothX) {
        _clothX=[self texture:MTLPixelFormatRGBA32Float width:width height:height];
        for(int i=0;i<2;i++) {
            _clothP[i]=[self texture:MTLPixelFormatRGBA32Float width:width height:height];
            _clothV[i]=[self texture:MTLPixelFormatRGBA32Float width:width height:height];
        }
        [encoder setBytes:&_params length:sizeof(_params) atIndex:0];
        [encoder setTexture:_clothX atIndex:0];[encoder setTexture:_clothV[0] atIndex:1];
        [self dispatch:encoder pipeline:[self compute:@"cloth_init"] width:width height:height];
    }
    if(!_clothDetail) {
        MTLTextureDescriptor *bake=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA16Float width:1024 height:1024 mipmapped:YES];
        bake.storageMode=MTLStorageModePrivate;bake.usage=MTLTextureUsageShaderRead|MTLTextureUsageShaderWrite;
        _clothDetail=[self.device newTextureWithDescriptor:bake];
        [encoder setTexture:_clothDetail atIndex:0];
        [self dispatch:encoder pipeline:[self compute:@"cloth_detail"] width:1024 height:1024];
        _clothDetailDirty=YES;
    }
    if(dt<=0)return;
    NSUInteger steps=MAX(1,(NSUInteger)ceilf(dt*120.0f));
    struct DelFluidParams cloth=_params;
    cloth.impulse.w=dt/(float)steps;
    cloth.windowMotion.xy/=(float)steps;
    // A held curtain follows the hand directly; the passing-hand wind and dent stand down.
    cloth.animation.z*=_handShare;cloth.impulse.x*=_handShare;cloth.impulse.y*=_handShare;
    if(_clothHeld||_heldFirmness>0||_heldSettle>0) {
        cloth.life=(simd_float4){_heldColumn,_heldRow,_heldFirmness,_heldSettle};
        cloth.stroke=(simd_float4){_heldPoint.x,_heldPoint.y,_heldPoint.z,_clothHeld?1.0f:0.0f};
    } else cloth.life=(simd_float4){0,0,0,0};
    [encoder setBytes:&cloth length:sizeof(cloth) atIndex:0];
    for(NSUInteger step=0;step<steps;step++) {
        [encoder setTexture:_clothX atIndex:0];[encoder setTexture:_clothV[0] atIndex:1];[encoder setTexture:_clothP[0] atIndex:2];
        [self dispatch:encoder pipeline:predict width:width height:height];
        for(NSUInteger i=0;i<iterations;i++) {
            [encoder setTexture:_clothP[i&1] atIndex:0];[encoder setTexture:_clothP[1-(i&1)] atIndex:1];
            [self dispatch:encoder pipeline:relax width:width height:height];
        }
        // An even iteration count leaves the corrected positions back in _clothP[0].
        [encoder setTexture:_clothX atIndex:0];[encoder setTexture:_clothP[0] atIndex:1];[encoder setTexture:_clothV[1] atIndex:2];
        [self dispatch:encoder pipeline:commit width:width height:height];
        id<MTLTexture> swap=_clothX;_clothX=_clothP[0];_clothP[0]=swap;
        swap=_clothV[0];_clothV[0]=_clothV[1];_clothV[1]=swap;
    }
}
// One frame of taking hold of, dragging and letting go of a Window Curtains panel.
- (void)holdCurtains:(float)dt {
    float zoom=fmaxf(_params.geometry.z,0.1f);
    simd_float2 extent={_params.geometry.x/zoom,_params.geometry.y/zoom};
    if(!_railsReady) {
        for(int i=0;i<DEL_CURTAIN_PANELS;i++) {
            DelCurtainRail rest=del_curtain_rest(i,extent.x);
            _rails[i]=(DelCurtainRail){rest.left/extent.x,rest.hook/extent.x,rest.right/extent.x,rest.at};
        }
        _railsReady=YES;
    }
    simd_float2 cursor={_params.pointer.x*extent.x,_params.pointer.y*extent.y};
    if(!_pressed) {_pick=nil;_pickWanted=NO;}
    else if(_pressBegan&&_params.pointer.x>=0&&_params.pointer.y>=0&&_params.pointer.x<1&&_params.pointer.y<1) {
        _pick=nil;_pickWanted=YES;
    }
    DelClothPick *pick=_pick;
    if(pick&&atomic_load(&pick->ready)) {
        _pick=nil;
        int column=0,row=0;
        if(pick.succeeded&&_pressed&&del_curtain_pick(pick.buffer.contents,DEL_CURTAIN_PANELS*DEL_CURTAIN_PANEL_COLUMNS,DEL_CURTAIN_ROWS,pick.cursor,&column,&row)) {
            const float *p=(const float *)pick.buffer.contents+(row*DEL_CURTAIN_PANELS*DEL_CURTAIN_PANEL_COLUMNS+column)*4;
            _clothHeld=YES;_lettingGo=NO;_heldSettle=0;_heldColumn=column;_heldRow=row;
            _heldOffset=(simd_float2){p[0],p[1]}-pick.cursor;
            _heldTarget=(simd_float2){p[0],p[1]};
            _heldPoint=(simd_float3){p[0],p[1],p[2]};
            _heldDepth=p[2];
            // Take hold at this hook, leaving every hook where it hangs.
            int panel=column/DEL_CURTAIN_PANEL_COLUMNS;
            float s=(float)(column%DEL_CURTAIN_PANEL_COLUMNS)/(DEL_CURTAIN_PANEL_COLUMNS-1);
            DelCurtainRail *rail=&_rails[panel];
            rail->hook=del_curtain_hook_x(*rail,s);rail->at=fminf(fmaxf(s,0.02f),0.98f);
            // Limits count from how it's hanging now, so taking hold moves nothing by itself.
            _heldNatural=(simd_float2){p[0]-rail->hook*extent.x,p[1]};
        }
    }
    if(_clothHeld&&!_pressed){_clothHeld=NO;_lettingGo=YES;_letGoAge=0;_cursorMoved=NO;_letGoAt=cursor;}
    if(!_cursorMoved&&simd_distance(cursor,_letGoAt)>10.0f)_cursorMoved=YES;
    _handShare=del_curtain_hand(_handShare,_clothHeld||_heldFirmness>0,_cursorMoved,dt);
    if(_clothHeld) {
        _heldFirmness=1;
        _heldSettle=0;
    }
    if(_lettingGo&&dt>0) {
        // The hand opens slowly and the point sinks to hang below its hook again.
        _letGoAge+=dt;
        int panel=_heldColumn/DEL_CURTAIN_PANEL_COLUMNS;
        float s=(float)(_heldColumn%DEL_CURTAIN_PANEL_COLUMNS)/(DEL_CURTAIN_PANEL_COLUMNS-1);
        DelCurtainRail *stored=&_rails[panel];
        DelCurtainRail rail={stored->left*extent.x,stored->hook*extent.x,stored->right*extent.x,stored->at};
        // Exactly where this fabric was hanging when taken hold of, relative to where its hook is
        // now. Sinking anywhere else pulls it out of shape, and it springs back as a late ripple.
        simd_float3 hanging={del_curtain_hook_x(rail,s)+_heldNatural.x,_heldNatural.y,_heldDepth};
        _heldPoint=del_curtain_sink(_heldPoint,hanging,_letGoAge,dt);
        _heldFirmness=fminf(_heldFirmness,del_curtain_grip(_letGoAge));
        _heldSettle=del_curtain_settle(_letGoAge);
        if(_letGoAge>=DEL_CURTAIN_SETTLE_SECONDS){_lettingGo=NO;_heldFirmness=0;_heldSettle=0;}
    }
    if(_clothHeld&&dt>0) {
        // Track the hand on this frame. Cloth constraints and rail travel limits
        // provide resistance; an extra cursor filter only adds input latency.
        _heldTarget=cursor+_heldOffset;
        int panel=_heldColumn/DEL_CURTAIN_PANEL_COLUMNS;
        DelCurtainHome home=del_curtain_home(panel,extent.x);
        DelCurtainRail *stored=&_rails[panel];
        DelCurtainRail rail={stored->left*extent.x,stored->hook*extent.x,stored->right*extent.x,stored->at};
        float restV=(extent.y+70.0f)/(DEL_CURTAIN_ROWS-1);
        simd_float2 held=del_curtain_hold(&rail,_heldTarget,_heldRow*restV,_heldNatural,home);
        *stored=(DelCurtainRail){rail.left/extent.x,rail.hook/extent.x,rail.right/extent.x,rail.at};
        // Keep the caught depth too: no animated lift or delayed target.
        _heldPoint=(simd_float3){held.x,held.y,_heldDepth};
    }
    for(int i=0;i<DEL_CURTAIN_PANELS;i++)_params.rails[i]=(simd_float4){_rails[i].left,_rails[i].hook,_rails[i].right,_rails[i].at};
}
- (void)stepParticles:(id<MTLComputeCommandEncoder>)encoder kernel:(NSString *)name width:(NSUInteger)width height:(NSUInteger)height dt:(float)dt {
    id<MTLComputePipelineState> pipeline=[self compute:name];
    BOOL fresh=!_particles[0];
    if(fresh) {
        for(int i=0;i<2;i++)_particles[i]=[self texture:MTLPixelFormatRGBA32Float width:width height:height];
        _particleIndex=0;
        struct DelFluidParams start=_params;start.impulse=(simd_float4){0,0,1,0};
        [encoder setBytes:&start length:sizeof(start) atIndex:0];
        [encoder setTexture:_particles[0] atIndex:0];[encoder setTexture:_particles[0] atIndex:1];
        [self dispatch:encoder pipeline:pipeline width:width height:height];
    }
    if(dt<=0)return;
    [encoder setBytes:&_params length:sizeof(_params) atIndex:0];
    [encoder setTexture:_particles[_particleIndex] atIndex:0];[encoder setTexture:_particles[1-_particleIndex] atIndex:1];
    [self dispatch:encoder pipeline:pipeline width:width height:height];
    _particleIndex=1-_particleIndex;
}
- (void)stepPudding:(id<MTLComputeCommandEncoder>)encoder dt:(float)dt {
    const NSUInteger width=128,height=64;
    id<MTLComputePipelineState> pipeline=[self compute:@"pudding_step"];
    float zoom=fmaxf(_params.geometry.z,0.1f);
    float plateY=_params.composer.y/zoom;
    if(!_puddingX[0]) {
        for(int i=0;i<2;i++) {
            _puddingX[i]=[self texture:MTLPixelFormatRGBA32Float width:width height:height];
            _puddingV[i]=[self texture:MTLPixelFormatRGBA32Float width:width height:height];
        }
        _puddingIndex=0;_plateY=plateY;_nextKick=_time;_kickUntil=0; // It wobbles as soon as it appears.
        struct DelFluidParams start=_params;start.impulse=(simd_float4){0,0,1,0};
        [encoder setBytes:&start length:sizeof(start) atIndex:0];
        [encoder setTexture:_puddingX[0] atIndex:0];[encoder setTexture:_puddingV[0] atIndex:1];
        [encoder setTexture:_puddingX[0] atIndex:2];[encoder setTexture:_puddingV[0] atIndex:3];
        [self dispatch:encoder pipeline:pipeline width:width height:height];
    }
    if(dt<=0)return;
    // Every few seconds something nudges the table; the pudding does the rest.
    if(_time>=_nextKick) {
        uint32_t beat=(uint32_t)(_nextKick*7.0f);
        float side=arc4random_uniform(2)?1.0f:-1.0f;
        _kick=(simd_float3){side*(2500.0f+1900.0f*del_weather_random(beat)),-(500.0f+900.0f*del_weather_random(beat+3)),(del_weather_random(beat+5)-0.5f)*1500.0f};
        _kickUntil=_time+0.09f;
        _nextKick=_time+3.4f+del_weather_random(beat+7)*3.6f;
    }
    simd_float3 force=_time<_kickUntil?_kick:(simd_float3){0,0,0};
    // Pressing the button on the pudding takes hold of that spot, however far it is then
    // pulled, until the button is released. Hovering alone does nothing.
    float zoomFactor=zoom;
    simd_float2 extent={_params.geometry.x/zoomFactor,_params.geometry.y/zoomFactor};
    BOOL hasBar=_params.composer.z>0&&_params.composer.w>0;
    float span=hasBar?_params.composer.z/zoomFactor:extent.x*0.8f;
    float radius=fminf(fmaxf(fminf(span*0.40f,extent.y*0.40f),70.0f),360.0f),puddingHeight=radius*0.76f;
    simd_float2 base={hasBar?(_params.composer.x+_params.composer.z*0.5f)/zoomFactor:extent.x*0.5f,(hasBar?plateY:extent.y*0.94f)+16.0f};
    simd_float2 raw={_params.pointer.x*extent.x-base.x,_params.pointer.y*extent.y-base.y};
    BOOL over=_reaction>0.02f&&fabsf(raw.x)<radius*1.02f&&raw.y<6.0f&&raw.y>-puddingHeight*1.25f;
    if(_pressBegan&&over){_grabbing=YES;_grabAnchor=raw;}
    if(!_pressed)_grabbing=NO;
    simd_float2 pull=_grabbing?raw-_grabAnchor:(simd_float2){0,0};
    float grip=_grabbing?1.0f:0.0f;
    // The bar is the plate: when it moves, the body lags behind it and wobbles.
    float travel=plateY-_plateY;
    _plateY=plateY;
    force.y-=fminf(fmaxf(travel/fmaxf(dt,0.001f),-1500.0f),1500.0f)*6.0f;
    NSUInteger steps=MAX(1,(NSUInteger)ceilf(dt*300.0f));
    struct DelFluidParams pudding=_params;
    pudding.impulse.w=dt/(float)steps;pudding.stroke=(simd_float4){force.x,force.y,0,force.z};
    pudding.life=(simd_float4){_grabAnchor.x,_grabAnchor.y,grip,0};pudding.clock.z=pull.x;pudding.clock.w=pull.y;
    [encoder setBytes:&pudding length:sizeof(pudding) atIndex:0];
    for(NSUInteger step=0;step<steps;step++) {
        NSUInteger next=1-_puddingIndex;
        [encoder setTexture:_puddingX[_puddingIndex] atIndex:0];[encoder setTexture:_puddingV[_puddingIndex] atIndex:1];
        [encoder setTexture:_puddingX[next] atIndex:2];[encoder setTexture:_puddingV[next] atIndex:3];
        [self dispatch:encoder pipeline:pipeline width:width height:height];
        _puddingIndex=next;
    }
}
- (void)paintWindow:(id<MTLComputeCommandEncoder>)encoder running:(BOOL)running {
    NSUInteger width=CVPixelBufferGetWidth(_buffer)/2,height=CVPixelBufferGetHeight(_buffer)/2;
    BOOL fresh=!_windowScene||_windowScene.width!=width||_windowScene.height!=height;
    if(fresh) {
        MTLTextureDescriptor *scene=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatR16Float width:width height:height mipmapped:YES];
        scene.storageMode=MTLStorageModePrivate;scene.usage=MTLTextureUsageShaderRead|MTLTextureUsageShaderWrite;
        _windowScene=[self.device newTextureWithDescriptor:scene];
    }
    if(!_branches) {
        // Mipmapped: lower levels are the same branches as light scattered by fabric sees them.
        MTLTextureDescriptor *strip=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatR16Float width:4096 height:1 mipmapped:YES];
        strip.storageMode=MTLStorageModePrivate;strip.usage=MTLTextureUsageShaderRead|MTLTextureUsageShaderWrite;
        _branches=[self.device newTextureWithDescriptor:strip];
    }
    if(!running&&!fresh)return;
    [encoder setBytes:&_params length:sizeof(_params) atIndex:0];
    [encoder setTexture:_windowScene atIndex:0];
    [self dispatch:encoder pipeline:[self compute:@"window_paint"] width:width height:height];
    [encoder setTexture:_branches atIndex:0];
    [self dispatch:encoder pipeline:[self compute:@"window_branches"] width:4096 height:1];
    _sceneDirty=YES;
}
// The sea's waves, fixed for its lifetime: long swell that draws the big polygons, down to
// short chop that adds fine detail, from every direction but leaning with the wind, each
// moving at its true deep-water speed.
- (void)prepareSeaWaves {
    if(_seaWavesReady)return;
    const int count=26;
    for(int i=0;i<count;i++) {
        float s=(float)i/(count-1);
        float wavelength=40.0f*powf(520.0f/40.0f,s)*(0.9f+0.2f*del_weather_random(i*3u+1u));
        float angle=0.6f+(del_weather_random(i*3u+2u)-0.5f)*5.4f;
        float k=2.0f*(float)M_PI/wavelength;
        float slope=(0.030f+0.050f*s)*(0.8f+0.4f*del_weather_random(i*3u+3u)); // Each wave's steepness.
        _seaWaves[i]=(simd_float4){cosf(angle)*k,sinf(angle)*k,sqrtf(110.0f*k),slope/k};
    }
    _seaWavesReady=YES;
}
- (void)stepSea:(id<MTLComputeCommandEncoder>)encoder dt:(float)dt {
    [self prepareSeaWaves];
    // Matches seaSize and seaOrigin in fluid.metal: the floor that can be seen, plus a margin.
    const float tilt=0.46f,margin=150.0f;
    float zoom=fmaxf(_params.geometry.z,0.1f);
    simd_float2 extent={_params.geometry.x/zoom,_params.geometry.y/zoom};
    simd_float2 domain=extent/(1.0f-tilt)+2.0f*margin;
    NSUInteger width=(NSUInteger)ceilf(domain.x/3.0f),height=(NSUInteger)ceilf(domain.y/3.0f);
    id<MTLComputePipelineState> pipeline=[self compute:@"sea_water"];
    BOOL fresh=!_sea[0]||_sea[0].width!=width||_sea[0].height!=height;
    if(fresh) {
        for(int i=0;i<2;i++)_sea[i]=[self texture:MTLPixelFormatRG32Float width:width height:height];
        _seaSurface=[self texture:MTLPixelFormatR32Float width:width height:height];
        _seaIndex=0;_seaBusyUntil=0;_seaDirty=YES;
    }
    if(_seaDirty) {
        // Clear ripples that have died down, so the wave solver needn't run on still water.
        struct DelFluidParams start=_params;start.impulse=(simd_float4){0,0,1,0};
        [encoder setBytes:&start length:sizeof(start) atIndex:0];
        for(int i=0;i<2;i++) {
            [encoder setTexture:_sea[i] atIndex:0];[encoder setTexture:_sea[i] atIndex:1];
            [self dispatch:encoder pipeline:pipeline width:width height:height];
        }
        _seaDirty=NO;
    }
    if(dt>0) {
        // Only a fresh click disturbs the water; moving the cursor never does.
        BOOL inside=_params.pointer.x>=0&&_params.pointer.y>=0&&_params.pointer.x<1&&_params.pointer.y<1;
        if(_pressBegan&&_params.animation.y>0.5f&&inside) {
            // The same homography as the shader: from where the click is on screen to the floor.
            float up=extent.y-_params.pointer.y*extent.y,g=1.0f-tilt*up/extent.y;
            _seaDrop=(simd_float2){extent.x*0.5f+(_params.pointer.x*extent.x-extent.x*0.5f)/g,extent.y-up/g};
            _seaDropAge=0;_seaBusyUntil=_time+9.0f;
        }
        if(_time<_seaBusyUntil) {
            NSUInteger steps=MAX(1,(NSUInteger)ceilf(dt*180.0f));
            struct DelFluidParams water=_params;water.impulse.w=dt/(float)steps;
            for(NSUInteger step=0;step<steps;step++) {
                // The stone bobs for a moment after it lands, three rings' worth, then is gone.
                float stepDt=dt/(float)steps,bob=0;
                if(_seaDropAge<0.9f) {
                    bob=190.0f*cosf(_seaDropAge*2.0f*(float)M_PI*2.7f)*expf(-_seaDropAge*3.4f)*stepDt;
                    _seaDropAge+=stepDt;
                }
                water.stroke=(simd_float4){_seaDrop.x,_seaDrop.y,bob,0};
                [encoder setBytes:&water length:sizeof(water) atIndex:0];
                [encoder setTexture:_sea[_seaIndex] atIndex:0];[encoder setTexture:_sea[1-_seaIndex] atIndex:1];
                [self dispatch:encoder pipeline:pipeline width:width height:height];_seaIndex=1-_seaIndex;
            }
        } else if(_seaBusyUntil>0) {_seaBusyUntil=0;_seaDirty=YES;}
    }
    [encoder setBytes:&_params length:sizeof(_params) atIndex:0];
    [encoder setBytes:_seaWaves length:sizeof(_seaWaves) atIndex:1];
    [encoder setTexture:_sea[_seaIndex] atIndex:0];[encoder setTexture:_seaSurface atIndex:1];
    [self dispatch:encoder pipeline:[self compute:@"sea_surface"] width:width height:height];
}
// Light rays refracted through the surface onto the sand, summed additively so folds
// of the light sheet overlap. Mipmapped so the water can scatter it softly as well.
- (void)drawCaustics:(id<MTLCommandBuffer>)command {
    NSUInteger width=CVPixelBufferGetWidth(_buffer),height=CVPixelBufferGetHeight(_buffer);
    if(!_caustics||_caustics.width!=width||_caustics.height!=height) {
        MTLTextureDescriptor *target=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatR16Float width:width height:height mipmapped:YES];
        target.storageMode=MTLStorageModePrivate;target.usage=MTLTextureUsageRenderTarget|MTLTextureUsageShaderRead;
        _caustics=[self.device newTextureWithDescriptor:target];
    }
    id<MTLRenderPipelineState> state=_renderPipelines[@"caustic"];
    if(!state) {
        MTLRenderPipelineDescriptor *descriptor=[MTLRenderPipelineDescriptor new];
        descriptor.vertexFunction=[self.library newFunctionWithName:@"caustic_vertex"];
        descriptor.fragmentFunction=[self.library newFunctionWithName:@"caustic_fragment"];
        MTLRenderPipelineColorAttachmentDescriptor *color=descriptor.colorAttachments[0];
        color.pixelFormat=MTLPixelFormatR16Float;color.blendingEnabled=YES;
        color.sourceRGBBlendFactor=MTLBlendFactorOne;color.destinationRGBBlendFactor=MTLBlendFactorOne;
        color.sourceAlphaBlendFactor=MTLBlendFactorOne;color.destinationAlphaBlendFactor=MTLBlendFactorOne;
        NSError *error=nil;
        state=[self.device newRenderPipelineStateWithDescriptor:descriptor error:&error];
        if(!state){NSLog(@"Del Metal caustics: %@",error);return;}
        _renderPipelines[@"caustic"]=state;
    }
    MTLRenderPassDescriptor *pass=[MTLRenderPassDescriptor renderPassDescriptor];
    pass.colorAttachments[0].texture=_caustics;pass.colorAttachments[0].loadAction=MTLLoadActionClear;
    pass.colorAttachments[0].storeAction=MTLStoreActionStore;pass.colorAttachments[0].clearColor=MTLClearColorMake(0,0,0,0);
    id<MTLRenderCommandEncoder> render=[command renderCommandEncoderWithDescriptor:pass];
    [render setRenderPipelineState:state];
    [render setVertexTexture:_seaSurface atIndex:0];
    [render setVertexBytes:&_params length:sizeof(_params) atIndex:0];
    [render drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:6*719*499];
    [render endEncoding];
    id<MTLBlitCommandEncoder> blit=[command blitCommandEncoder];
    [blit generateMipmapsForTexture:_caustics];[blit endEncoding];
}
- (void)stepClouds:(id<MTLComputeCommandEncoder>)encoder running:(BOOL)running dt:(float)dt {
    if(!_cloudNoise) {
        MTLTextureDescriptor *volume=[MTLTextureDescriptor new];
        volume.textureType=MTLTextureType3D;volume.pixelFormat=MTLPixelFormatRGBA16Float;
        volume.width=volume.height=volume.depth=64;volume.storageMode=MTLStorageModePrivate;
        volume.usage=MTLTextureUsageShaderRead|MTLTextureUsageShaderWrite;
        _cloudNoise=[self.device newTextureWithDescriptor:volume];
        [encoder setComputePipelineState:[self compute:@"cloud_noise"]];
        [encoder setTexture:_cloudNoise atIndex:0];
        [encoder dispatchThreads:MTLSizeMake(64,64,64) threadsPerThreadgroup:MTLSizeMake(4,4,4)];
        [encoder memoryBarrierWithScope:MTLBarrierScopeTextures];
    }
    NSUInteger width=MAX(1,(NSUInteger)(_params.geometry.x*0.85f)),height=MAX(1,(NSUInteger)(_params.geometry.y*0.85f));
    BOOL fresh=!_cloud[0]||_cloud[0].width!=width||_cloud[0].height!=height;
    if(fresh)for(int i=0;i<2;i++)_cloud[i]=[self texture:MTLPixelFormatRG16Float width:width height:height];
    if(!running&&!fresh)return;
    // The trail the cursor clears: only allocated once someone actually uses it.
    if(!_cloudWake[0]||_cloudWake[0].width!=_simWidth||_cloudWake[0].height!=_simHeight)
        for(int i=0;i<2;i++)_cloudWake[i]=[self texture:MTLPixelFormatR16Float width:_simWidth height:_simHeight];
    struct DelFluidParams trail=_params;
    trail.impulse.w=dt;
    [encoder setBytes:&trail length:sizeof(trail) atIndex:0];
    [encoder setTexture:_cloudWake[_wakeIndex] atIndex:0];[encoder setTexture:_cloudWake[1-_wakeIndex] atIndex:1];
    [self dispatch:encoder pipeline:[self compute:@"cloud_wake"] width:_simWidth height:_simHeight];
    _wakeIndex=1-_wakeIndex;
    // New views have no history to blend with; converge immediately.
    {
        struct DelFluidParams clouds=_params;
        clouds.impulse.z=fresh?1:0;clouds.life.y=(float)_frameCount;
        [encoder setBytes:&clouds length:sizeof(clouds) atIndex:0];
        [encoder setTexture:_cloudNoise atIndex:0];[encoder setTexture:_cloud[_cloudIndex] atIndex:1];[encoder setTexture:_cloud[1-_cloudIndex] atIndex:2];
        [encoder setTexture:_cloudWake[_wakeIndex] atIndex:3];
        [self dispatch:encoder pipeline:[self compute:@"cloud_march"] width:width height:height];
        _cloudIndex=1-_cloudIndex;
    }
}
// Geometry styles draw into one RG float target, multisampled for clean edges.
// R carries scene light; G carries coverage for the ink rendering on light themes.
- (void)drawField:(id<MTLCommandBuffer>)command style:(unsigned int)style {
    NSUInteger width=CVPixelBufferGetWidth(_buffer),height=CVPixelBufferGetHeight(_buffer);
    if(!_field||_field.width!=width||_field.height!=height) {
        MTLTextureDescriptor *resolved=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRG16Float width:width height:height mipmapped:YES];
        resolved.storageMode=MTLStorageModePrivate;resolved.usage=MTLTextureUsageRenderTarget|MTLTextureUsageShaderRead;
        _field=[self.device newTextureWithDescriptor:resolved];
        MTLTextureDescriptor *samples=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRG16Float width:width height:height mipmapped:NO];
        samples.textureType=MTLTextureType2DMultisample;samples.sampleCount=4;
        samples.storageMode=MTLStorageModeMemoryless;samples.usage=MTLTextureUsageRenderTarget;
        _fieldSamples=[self.device newTextureWithDescriptor:samples];
        MTLTextureDescriptor *depth=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatDepth32Float width:width height:height mipmapped:NO];
        depth.textureType=MTLTextureType2DMultisample;depth.sampleCount=4;
        depth.storageMode=MTLStorageModeMemoryless;depth.usage=MTLTextureUsageRenderTarget;
        _fieldDepth=[self.device newTextureWithDescriptor:depth];
    }
    const NSUInteger clothVertices=6*359*179;
    if(style==7||style==12) {
        // The sun's view of the fabric: a shadow map, re-rendered as the cloth moves.
        if(!_shadowMap) {
            MTLTextureDescriptor *map=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatDepth32Float width:2048 height:2048 mipmapped:NO];
            map.storageMode=MTLStorageModePrivate;map.usage=MTLTextureUsageRenderTarget|MTLTextureUsageShaderRead;
            _shadowMap=[self.device newTextureWithDescriptor:map];
        }
        MTLRenderPassDescriptor *sunPass=[MTLRenderPassDescriptor renderPassDescriptor];
        sunPass.depthAttachment.texture=_shadowMap;sunPass.depthAttachment.loadAction=MTLLoadActionClear;
        sunPass.depthAttachment.storeAction=MTLStoreActionStore;sunPass.depthAttachment.clearDepth=1;
        id<MTLRenderCommandEncoder> sun=[command renderCommandEncoderWithDescriptor:sunPass];
        [sun setRenderPipelineState:[self render:@"cloth_shadow_vertex" fragment:nil blend:DelBlendNone depth:YES]];
        MTLDepthStencilDescriptor *nearest=[MTLDepthStencilDescriptor new];
        nearest.depthCompareFunction=MTLCompareFunctionLess;nearest.depthWriteEnabled=YES;
        [sun setDepthStencilState:[self.device newDepthStencilStateWithDescriptor:nearest]];
        // Slope-scaled, so a fold turned edge-on to the sun never shades itself in rows.
        [sun setDepthBias:0.002f slopeScale:3.0f clamp:0.06f];
        [sun setVertexTexture:_clothX atIndex:0];[sun setVertexBytes:&_params length:sizeof(_params) atIndex:0];
        [sun drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:clothVertices];
        [sun endEncoding];
    }
    MTLRenderPassDescriptor *pass=[MTLRenderPassDescriptor renderPassDescriptor];
    pass.colorAttachments[0].texture=_fieldSamples;pass.colorAttachments[0].resolveTexture=_field;
    pass.colorAttachments[0].loadAction=MTLLoadActionClear;pass.colorAttachments[0].storeAction=MTLStoreActionMultisampleResolve;
    pass.colorAttachments[0].clearColor=MTLClearColorMake(0,0,0,0);
    if(style==11) {
        pass.depthAttachment.texture=_fieldDepth;pass.depthAttachment.loadAction=MTLLoadActionClear;
        pass.depthAttachment.storeAction=MTLStoreActionDontCare;pass.depthAttachment.clearDepth=1;
    }
    id<MTLRenderCommandEncoder> render=[command renderCommandEncoderWithDescriptor:pass];
    [render setVertexBytes:&_params length:sizeof(_params) atIndex:0];
    [render setFragmentBytes:&_params length:sizeof(_params) atIndex:0];
    if(style==12) {
        [render setRenderPipelineState:[self render:@"cloth_backdrop_vertex" fragment:@"window_backdrop" blend:DelBlendNone depth:NO]];
        [render setFragmentTexture:_windowScene atIndex:0];
        [render drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
        [render setRenderPipelineState:[self render:@"cloth_vertex" fragment:@"window_cloth_fragment" blend:DelBlendTransmit depth:NO]];
        [render setVertexTexture:_clothX atIndex:0];[render setVertexTexture:_clothV[0] atIndex:1];
        [render setFragmentTexture:_windowScene atIndex:0];[render setFragmentTexture:_shadowMap atIndex:1];[render setFragmentTexture:_clothDetail atIndex:2];[render setFragmentTexture:_branches atIndex:3];
        [render drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:clothVertices];
    } else if(style==7) {
        // The window behind the cloth first; each layer of fabric then filters what's behind it.
        [render setRenderPipelineState:[self render:@"cloth_backdrop_vertex" fragment:@"cloth_backdrop" blend:DelBlendNone depth:NO]];
        [render setFragmentTexture:_shadowMap atIndex:0];
        [render drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
        [render setRenderPipelineState:[self render:@"cloth_vertex" fragment:@"cloth_fragment" blend:DelBlendTransmit depth:NO]];
        [render setVertexTexture:_clothX atIndex:0];[render setVertexTexture:_clothV[0] atIndex:1];
        [render setFragmentTexture:_shadowMap atIndex:0];[render setFragmentTexture:_clothDetail atIndex:1];
        [render drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:clothVertices];
    } else if(style==8) {
        [render setRenderPipelineState:[self render:@"rain_streak_vertex" fragment:@"rain_streak_fragment" blend:DelBlendAdd depth:NO]];
        [render drawPrimitives:MTLPrimitiveTypeTriangleStrip vertexStart:0 vertexCount:4 instanceCount:DEL_RAIN_COUNT+340];
        [render setRenderPipelineState:[self render:@"rain_splash_vertex" fragment:@"rain_splash_fragment" blend:DelBlendAdd depth:NO]];
        [render drawPrimitives:MTLPrimitiveTypeTriangleStrip vertexStart:0 vertexCount:4 instanceCount:DEL_RAIN_COUNT*3];
        // setVertexBytes keeps this free of buffers, and the droplets are only 2 KB.
        [render setRenderPipelineState:[self render:@"rain_runoff_vertex" fragment:@"rain_runoff_fragment" blend:DelBlendAdd depth:NO]];
        [render setVertexBytes:_droplets length:sizeof(_droplets) atIndex:1];
        [render drawPrimitives:MTLPrimitiveTypeTriangleStrip vertexStart:0 vertexCount:4 instanceCount:DEL_RUNOFF_COUNT];
    } else if(style==5) {
        [render setRenderPipelineState:[self render:@"mote_vertex" fragment:@"mote_fragment" blend:DelBlendAdd depth:NO]];
        [render setVertexTexture:_particles[_particleIndex] atIndex:0];
        [render drawPrimitives:MTLPrimitiveTypeTriangleStrip vertexStart:0 vertexCount:4 instanceCount:300];
    } else if(style==11) {
        [render setRenderPipelineState:[self render:@"pudding_vertex" fragment:@"pudding_fragment" blend:DelBlendNone depth:YES]];
        MTLDepthStencilDescriptor *test=[MTLDepthStencilDescriptor new];
        test.depthCompareFunction=MTLCompareFunctionLess;test.depthWriteEnabled=YES;
        [render setDepthStencilState:[self.device newDepthStencilStateWithDescriptor:test]];
        [render setVertexTexture:_puddingX[_puddingIndex] atIndex:0];
        [render drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:6*128*63];
    }
    [render endEncoding];
    if(style==7||style==11||style==12) {
        id<MTLBlitCommandEncoder> blit=[command blitCommandEncoder];
        [blit generateMipmapsForTexture:_field];
        [blit endEncoding];
    }
    if(style==12) {
        NSUInteger shaftWidth=width/2,shaftHeight=height/2;
        BOOL fresh=!_shafts[0]||_shafts[0].width!=shaftWidth||_shafts[0].height!=shaftHeight;
        if(fresh)for(int i=0;i<2;i++) {
            // Mipmapped: the wide levels are how a beam looks once fabric has spread it.
            MTLTextureDescriptor *shaft=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatR16Float width:shaftWidth height:shaftHeight mipmapped:YES];
            shaft.storageMode=MTLStorageModePrivate;shaft.usage=MTLTextureUsageShaderRead|MTLTextureUsageShaderWrite;
            _shafts[i]=[self.device newTextureWithDescriptor:shaft];
        }
        if(_running||fresh) {
            struct DelFluidParams light=_params;
            light.impulse.z=fresh?1:0;light.life.y=(float)_frameCount;
            id<MTLComputeCommandEncoder> shaftEncoder=[command computeCommandEncoder];
            [shaftEncoder setBytes:&light length:sizeof(light) atIndex:0];
            [shaftEncoder setTexture:_shadowMap atIndex:0];[shaftEncoder setTexture:_branches atIndex:1];
            [shaftEncoder setTexture:_shafts[_shaftIndex] atIndex:2];[shaftEncoder setTexture:_shafts[1-_shaftIndex] atIndex:3];
            [self dispatch:shaftEncoder pipeline:[self compute:@"window_shafts"] width:shaftWidth height:shaftHeight];
            [shaftEncoder endEncoding];
            _shaftIndex=1-_shaftIndex;
            id<MTLBlitCommandEncoder> spread=[command blitCommandEncoder];
            [spread generateMipmapsForTexture:_shafts[_shaftIndex]];[spread endEncoding];
        }
    }
}
- (void)encode:(id<MTLCommandBuffer>)command buffer:(CVPixelBufferRef)buffer {
    if(buffer!=_buffer||![self prepareDevice:command.commandQueue.device])return;
    float pitch=fmaxf(7.0f*_params.geometry.z,fmaxf(_params.geometry.x/384.0f,_params.geometry.y/256.0f));
    NSUInteger width=MAX(1,(NSUInteger)ceilf(_params.geometry.x/pitch));
    NSUInteger height=MAX(1,(NSUInteger)ceilf(_params.geometry.y/pitch));
    if(width!=_simWidth||height!=_simHeight) {
        _liquid[0]=nil;_liquid[1]=nil;_contour=nil;_liquidPointerValid=NO;
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
    _time+=dt;
    // Smooth only while the frame clock runs; inactive windows stay completely still.
    BOOL directCurtain=(unsigned int)_params.animation.x==12;
    float follow=directCurtain?1.0f:1.0f-expf(-dt*9.0f);
    if(_params.life.z>0) {
        simd_float2 step={(_params.pointer.x-_params.pointer.z)*follow,(_params.pointer.y-_params.pointer.w)*follow};
        // Cloth can only be swept so fast: a flick reaches it as a firm sweep, never a whip.
        unsigned int shown=(unsigned int)_params.animation.x;
        if(shown==7&&dt>0) {
            float scale=fmaxf(_params.geometry.z,0.1f);
            simd_float2 room={_params.geometry.x/scale,_params.geometry.y/scale};
            float travel=simd_length(step*room),furthest=700.0f*dt;
            if(travel>furthest)step*=furthest/travel;
        }
        _params.pointer.z+=step.x;_params.pointer.w+=step.y;
    }
    // A press only counts on the background itself, not on the input bar or its footer.
    {
        simd_float2 at={_params.pointer.x*_params.geometry.x,_params.pointer.y*_params.geometry.y};
        simd_float4 bars[3]={_params.composer,_params.footer,_params.footerSide};
        BOOL onBar=NO;
        for(int i=0;i<3;i++)if(bars[i].z>0&&bars[i].w>0&&at.x>=bars[i].x&&at.y>=bars[i].y&&at.x<bars[i].x+bars[i].z&&at.y<bars[i].y+bars[i].w)onBar=YES;
        _pressBegan=_pressed&&!_wasPressed&&!onBar;_wasPressed=_pressed;
    }
    if(_params.animation.y<0.5f)_reaction=0;
    else if(directCurtain&&_params.life.z>0&&dt>0)_reaction=1;
    else _reaction+=(_params.life.z-_reaction)*(1.0f-expf(-dt*7.0f));
    _params.animation.z=_reaction;
    _params.life.z=_reaction;
    BOOL simulatesLife=(unsigned int)_params.animation.x==2;
    if(simulatesLife)_generationTime+=dt;
    BOOL generation=_running&&simulatesLife&&_generationTime>=0.09f;
    if(generation){_generationTime=fmodf(_generationTime,0.09f);_generation++;}
    // How fast the cursor is moving, in logical pixels per second, for styles that feel it.
    simd_float2 smoothed={_params.pointer.z,_params.pointer.w};
    float zoomed=fmaxf(_params.geometry.z,0.1f);
    simd_float2 span={_params.geometry.x/zoomed,_params.geometry.y/zoomed};
    if(dt>0) {
        simd_float2 moved=_pointerTracked?(smoothed-_lastPointer)*span/dt:(simd_float2){0,0};
        if(directCurtain)_pointerVelocity=del_curtain_limit_vector(moved,700.0f);
        else _pointerVelocity+=(moved-_pointerVelocity)*(1.0f-expf(-dt*14.0f));
        _lastPointer=smoothed;_pointerTracked=YES;
    }
    _params.impulse=(simd_float4){_pointerVelocity.x*_reaction,_pointerVelocity.y*_reaction,0,dt};
    // During a native title-bar drag the WindowServer can move the window before
    // AppKit publishes its new frame. Sample the compositor's actual bounds.
    NSWindow *nativeWindow=self.windowView.window;
    if(directCurtain&&nativeWindow) {
        NSRect frame=nativeWindow.frame;
        CGFloat desktopTop=NSMaxY(NSScreen.screens.firstObject.frame);
        self.windowOrigin=(simd_float2){NSMinX(frame),desktopTop-NSMaxY(frame)};
        CFArrayRef windows=CGWindowListCopyWindowInfo(kCGWindowListOptionIncludingWindow,(CGWindowID)nativeWindow.windowNumber);
        if(windows) {
            if(CFArrayGetCount(windows)>0) {
                CFDictionaryRef info=CFArrayGetValueAtIndex(windows,0);
                CGRect live;
                if(CGRectMakeWithDictionaryRepresentation(CFDictionaryGetValue(info,kCGWindowBounds),&live))
                    self.windowOrigin=(simd_float2){live.origin.x,live.origin.y};
            }
            CFRelease(windows);
        }
    }
    simd_float2 windowDelta={0,0};
    if(directCurtain&&dt>0) {
        if(_windowTracked)windowDelta=(self.windowOrigin-_lastWindowOrigin)/zoomed;
        _lastWindowOrigin=self.windowOrigin;_windowTracked=YES;
    } else _windowTracked=NO;
    // Feed displacement directly: no velocity filter, force ramp, or speed cap.
    _params.windowMotion=(simd_float4){windowDelta.x,windowDelta.y,0,0};
    // The blur eases in when a message is sent, rather than snapping.
    float blurTarget=_blurred?1:0;
    _blurMix=_running?_blurMix+(blurTarget-_blurMix)*(1.0f-expf(-dt*5.0f)):blurTarget;
    if(fabsf(blurTarget-_blurMix)<0.002f)_blurMix=blurTarget;
    _params.blur.w=_blurMix;
    _params.clock.x=_time;_params.clock.z=_simWidth;_params.clock.w=_simHeight;
    _params.life.x=generation?1:0;_params.life.y=(float)_generation;_params.life.w=(float)_universeSeed;
    unsigned int style=(unsigned int)_params.animation.x;
    if(style!=_activeStyle) {[self releaseStyleState];_activeStyle=style;}
    if(style==12)[self holdCurtains:dt];
    else memset(_params.rails,0,sizeof(_params.rails));
    _frameCount++;
    if(style==8) {
        float zoom=fmaxf(_params.geometry.z,0.1f);
        simd_float2 extent=(simd_float2){_params.geometry.x,_params.geometry.y}/zoom;
        simd_float4 composer=_params.composer/zoom,footer=_params.footer/zoom,side=_params.footerSide/zoom;
        simd_float2 radii=(simd_float2){_params.blur.x,_params.blur.y}/zoom;
        simd_float3 shelter={_params.pointer.z*extent.x,_params.pointer.w*extent.y,64.0f*_reaction};
        BOOL stable=_rainReady&&simd_all(composer==_rainComposer)&&simd_all(footer==_rainFooter)&&simd_all(side==_rainSide)
            &&simd_all(_params.geometry==_rainGeometry)&&simd_all(radii==_rainRadii);
        simd_float4 surfaces[3]={composer,footer,side};
        for(unsigned i=0;i<DEL_RAIN_COUNT;i++) {
            float lean=0;
            simd_float4 drop=del_rain_drop(i,_time,extent,composer,footer,side,radii,shelter,&lean);
            int surface=del_rain_surface(drop,composer,footer,side,radii);
            if(stable&&_running&&surface>=0&&del_rain_impact(del_rain_drop(i,_rainTime,extent,composer,footer,side,radii,shelter,NULL),drop)) {
                // Every drop that lands leaves a bead behind.
                del_runoff_spawn(_runoff,surface,(simd_float2){drop.x,drop.y},surfaces[surface],surface==0?radii.x:radii.y,1.05f+0.75f*del_weather_random(i*31u+(uint32_t)(_time*60.0f)));
            }
            _params.rain[i]=drop;_params.lean[i]=lean;
        }
        if(_running)del_runoff_step(_runoff,dt,surfaces,radii,extent.y+30,_droplets);
        _params.stroke.x=del_rain_slope(_time);
        _rainComposer=composer;_rainFooter=footer;_rainSide=side;_rainGeometry=_params.geometry;_rainRadii=radii;
        _rainTime=_time;_rainReady=_running;
    } else _rainReady=NO;
    id<MTLComputeCommandEncoder> encoder=[command computeCommandEncoder];
    [encoder setBytes:&_params length:sizeof(_params) atIndex:0];
    if(_clear) {
        [encoder setTexture:_cells[0] atIndex:0];[encoder setTexture:_cells[1] atIndex:1];
        [encoder setTexture:_highlight[0] atIndex:2];[encoder setTexture:_highlight[1] atIndex:3];
        [self dispatch:encoder pipeline:self.clearState];_clear=NO;
    }
    if(_running&&simulatesLife) {
        [encoder setBytes:&_params length:sizeof(_params) atIndex:0];
        [encoder setTexture:_cells[_dyeIndex] atIndex:0];[encoder setTexture:_cells[1-_dyeIndex] atIndex:1];
        [encoder setTexture:_highlight[_dyeIndex] atIndex:2];[encoder setTexture:_highlight[1-_dyeIndex] atIndex:3];
        [self dispatch:encoder pipeline:self.lifeState];_dyeIndex=1-_dyeIndex;
    }
    BOOL liquid=style==3;
    // A separate height/velocity field carries the liquid wake between frames.
    // Substeps bound the wave solver independently of display refresh rate.
    BOOL reactive=_params.animation.y>0.5f;
    if(!liquid||!reactive) {_clearLiquid=YES;_liquidPointerValid=NO;}
    if(liquid) {
        if(!_liquid[0]) {
            for(int i=0;i<2;i++)_liquid[i]=[self texture:MTLPixelFormatRG32Float width:width height:height];
            _liquidIndex=0;_clearLiquid=YES;
        }
        struct DelFluidParams water=_params;
        simd_float2 point={_params.pointer.x,_params.pointer.y};
        BOOL inside=point.x>=0&&point.y>=0&&point.x<1&&point.y<1;
        simd_float2 previous=_liquidPointerValid?_liquidPointer:point;
        simd_float2 travel=(point-previous)*(simd_float2){_params.geometry.x,_params.geometry.y}/fmaxf(_params.geometry.z,0.1f);
        float distance=simd_length(travel);
        // Entering/resuming starts a fresh stroke; never draw a wake across the window.
        BOOL stirring=reactive&&inside&&_liquidPointerValid&&distance>0.01f;
        water.stroke=(simd_float4){previous.x,previous.y,point.x,point.y};
        if(_clearLiquid) {
            water.impulse=(simd_float4){0,0,1,0};
            [encoder setBytes:&water length:sizeof(water) atIndex:0];
            [encoder setTexture:_liquid[_liquidIndex] atIndex:0];[encoder setTexture:_liquid[1-_liquidIndex] atIndex:1];
            [self dispatch:encoder pipeline:[self compute:@"liquid_step"]];_liquidIndex=1-_liquidIndex;_clearLiquid=NO;
        }
        if(_running&&reactive) {
            NSUInteger steps=MAX(1,(NSUInteger)ceilf(dt*120.0f));
            for(NSUInteger step=0;step<steps;step++) {
                water.impulse=(simd_float4){stirring?fminf(distance,80.0f)/(float)steps:0,0,0,dt/(float)steps};
                [encoder setBytes:&water length:sizeof(water) atIndex:0];
                [encoder setTexture:_liquid[_liquidIndex] atIndex:0];[encoder setTexture:_liquid[1-_liquidIndex] atIndex:1];
                [self dispatch:encoder pipeline:[self compute:@"liquid_step"]];_liquidIndex=1-_liquidIndex;
            }
            _liquidPointer=point;_liquidPointerValid=inside;
        }
    }
    if(style==4) {
        BOOL fresh=!_contour;
        if(fresh)_contour=[self texture:MTLPixelFormatR32Float width:width height:height];
        if(_running||fresh) {
            [encoder setBytes:&_params length:sizeof(_params) atIndex:0];
            [encoder setTexture:_contour atIndex:0];
            [self dispatch:encoder pipeline:[self compute:@"contour_step"]];
        }
    }
    if(style==7||style==12)[self stepCurtains:encoder dt:dt];
    if(style==12)[self paintWindow:encoder running:_running];
    if(style==13)[self stepSea:encoder dt:dt];
    if(style==5)[self stepParticles:encoder kernel:@"mote_step" width:300 height:1 dt:dt];
    if(style==11)[self stepPudding:encoder dt:dt];
    if(style==9)[self stepClouds:encoder running:_running dt:dt];
    [encoder endEncoding];
    if(_sceneDirty) {
        id<MTLBlitCommandEncoder> blit=[command blitCommandEncoder];
        [blit generateMipmapsForTexture:_windowScene];[blit generateMipmapsForTexture:_branches];[blit endEncoding];
        _sceneDirty=NO;
    }
    if(style==13)[self drawCaustics:command];
    if(_clothDetailDirty) {
        id<MTLBlitCommandEncoder> blit=[command blitCommandEncoder];
        [blit generateMipmapsForTexture:_clothDetail];[blit endEncoding];
        _clothDetailDirty=NO;
    }
    if(_pickWanted&&_clothX) {
        // Keep the press location with its snapshot: a fast drag must not pick a
        // different point (or miss the panel) when asynchronous readback finishes.
        _pickWanted=NO;
        NSUInteger columns=DEL_CURTAIN_PANELS*DEL_CURTAIN_PANEL_COLUMNS;
        DelClothPick *pick=[DelClothPick new];
        pick.cursor=_params.pointer.xy*(simd_float2){_params.geometry.x,_params.geometry.y}/fmaxf(_params.geometry.z,0.1f);
        pick.buffer=[self.device newBufferWithLength:columns*DEL_CURTAIN_ROWS*16 options:MTLResourceStorageModeShared];
        id<MTLBlitCommandEncoder> copy=[command blitCommandEncoder];
        [copy copyFromTexture:_clothX sourceSlice:0 sourceLevel:0 sourceOrigin:MTLOriginMake(0,0,0) sourceSize:MTLSizeMake(columns,DEL_CURTAIN_ROWS,1)
            toBuffer:pick.buffer destinationOffset:0 destinationBytesPerRow:columns*16 destinationBytesPerImage:columns*DEL_CURTAIN_ROWS*16];
        [copy endEncoding];
        _pick=pick;
        [command addCompletedHandler:^(id<MTLCommandBuffer> finished) {
            pick.succeeded=finished.status==MTLCommandBufferStatusCompleted;
            atomic_store(&pick->ready,true);
        }];
    }
    if(style==5||style==7||style==8||style==11||style==12)[self drawField:command style:style];
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
    [render setFragmentTexture:liquid?_liquid[_liquidIndex]:style==13?_seaSurface:_contour?:_cells[_dyeIndex] atIndex:2];
    [render setFragmentTexture:style==9?_cloud[_cloudIndex]:style==13?_caustics:_field atIndex:3];
    [render setFragmentTexture:style==12?_shafts[_shaftIndex]:_field atIndex:4];
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
    if(_blurMix>0) {
        // Half of half resolution, then a Gaussian of about 6 logical pixels of sigma in all:
        // enough to soften the scene behind text while still letting its shapes read.
        NSUInteger strongWidth=MAX(_blurB.width/2,1),strongHeight=MAX(_blurB.height/2,1);
        if(!_strong[0]||_strong[0].width!=strongWidth||_strong[0].height!=strongHeight)
            for(int i=0;i<2;i++)_strong[i]=[self texture:MTLPixelFormatRGBA16Float width:strongWidth height:strongHeight];
        float pixel=0.60f*_params.blur.z*_params.geometry.z;
        simd_float2 across={pixel/(float)_params.geometry.x/(float)_params.blur.z,0},down={0,pixel/(float)_params.geometry.y/(float)_params.blur.z};
        encoder=[command computeCommandEncoder];
        [encoder setComputePipelineState:[self compute:@"strong_blur"]];
        [encoder setTexture:_blurB atIndex:0];[encoder setTexture:_strong[0] atIndex:1];[encoder setBytes:&across length:sizeof(across) atIndex:1];
        [encoder dispatchThreads:MTLSizeMake(strongWidth,strongHeight,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];
        [encoder memoryBarrierWithScope:MTLBarrierScopeTextures];
        [encoder setTexture:_strong[0] atIndex:0];[encoder setTexture:_strong[1] atIndex:1];[encoder setBytes:&down length:sizeof(down) atIndex:1];
        [encoder dispatchThreads:MTLSizeMake(strongWidth,strongHeight,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];
        [encoder endEncoding];
    }
    pass.colorAttachments[0].texture=CVMetalTextureGetTexture(_surface);
    render=[command renderCommandEncoderWithDescriptor:pass];
    [render setRenderPipelineState:self.compositeState];
    [render setFragmentTexture:_scene atIndex:0];[render setFragmentTexture:_blurB atIndex:1];[render setFragmentTexture:_strong[1] atIndex:2];
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
void del_fluid_blurred(void *pointer,bool blurred) { if(pointer)[(__bridge DelFluidRenderer *)pointer setBlurred:blurred]; }
void del_fluid_force_press(void *pointer,bool pressed) { if(pointer)[(__bridge DelFluidRenderer *)pointer forcePress:pressed]; }
void del_fluid_window_view(void *pointer,void *view) {
    if(pointer)[(__bridge DelFluidRenderer *)pointer setWindowView:(__bridge NSView *)view];
}
void del_fluid_window_origin(void *pointer,double x,double y) {
    if(pointer)[(__bridge DelFluidRenderer *)pointer setWindowOrigin:(simd_float2){x,y}];
}
void del_fluid_running(void *pointer,bool running) { if(pointer)[(__bridge DelFluidRenderer *)pointer setRunning:running]; }
void *del_fluid_frame(void *pointer,double width,double height,double scale,double zoom,double radius,double x,double y,double originX,double originY,bool light,unsigned int style,float red,float green,float blue,float strength,bool reactive) {
    if(!pointer)return NULL;
    DelFluidRenderer *renderer=(__bridge DelFluidRenderer *)pointer;renderer.origin=(simd_float2){originX,originY};
    CVPixelBufferRef buffer=[renderer frameWidth:width height:height scale:scale zoom:zoom radius:radius mouseX:x mouseY:y light:light style:style tint:(simd_float4){red,green,blue,strength} reactive:reactive];
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
