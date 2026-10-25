#import <AppKit/AppKit.h>
#import <Metal/Metal.h>
#import <CoreVideo/CoreVideo.h>
#import <simd/simd.h>
@class DelBackdrop;
@interface DelBackdropOwner:NSObject
@property(nonatomic,weak) DelBackdrop *renderer;
@end
@implementation DelBackdropOwner
@end
@interface DelBackdrop:NSObject {
    CVPixelBufferRef _marker;
    id<MTLTexture> _horizontal,_vertical;
}
@property(nonatomic,strong) NSData *libraryData;
@property(nonatomic,strong) id<MTLDevice> device;
@property(nonatomic,strong) id<MTLComputePipelineState> horizontalState,verticalState;
@property(nonatomic,strong) id<MTLRenderPipelineState> compositeState;
@property(nonatomic,strong) MTLRenderPassDescriptor *pass;
@property(nonatomic) float scaleZoom;
- (CVPixelBufferRef)marker;
- (void)encode:(id<MTLCommandBuffer>)command scene:(id<MTLTexture>)scene;
@end
@implementation DelBackdrop
- (void)dealloc {if(_marker)CFRelease(_marker);}
- (CVPixelBufferRef)marker {
    if(!_marker){
        NSDictionary *attributes=@{(__bridge NSString *)kCVPixelBufferIOSurfacePropertiesKey:@{}};
        if(CVPixelBufferCreate(kCFAllocatorDefault,1,1,kCVPixelFormatType_32BGRA,(__bridge CFDictionaryRef)attributes,&_marker)!=kCVReturnSuccess)return NULL;
        DelBackdropOwner *owner=[DelBackdropOwner new];owner.renderer=self;
        CVBufferSetAttachment(_marker,CFSTR("DelBackdropOwner"),(__bridge CFTypeRef)owner,kCVAttachmentMode_ShouldNotPropagate);
    }
    return _marker;
}
- (BOOL)prepare:(id<MTLDevice>)device format:(MTLPixelFormat)format {
    if(self.device==device&&self.compositeState&&self.horizontalState&&self.verticalState)return YES;
    self.device=device;
    NSData *bytes=self.libraryData;
    dispatch_data_t data=dispatch_data_create(bytes.bytes,bytes.length,dispatch_get_main_queue(),^{(void)bytes;});
    NSError *error=nil;
    id<MTLLibrary> library=[device newLibraryWithData:data error:&error];
    if(!library){NSLog(@"Del backdrop library: %@",error);return NO;}
    self.horizontalState=[device newComputePipelineStateWithFunction:[library newFunctionWithName:@"backdrop_horizontal"] error:&error];
    self.verticalState=[device newComputePipelineStateWithFunction:[library newFunctionWithName:@"backdrop_vertical"] error:&error];
    MTLRenderPipelineDescriptor *descriptor=[MTLRenderPipelineDescriptor new];
    descriptor.vertexFunction=[library newFunctionWithName:@"fluid_vertex"];
    descriptor.fragmentFunction=[library newFunctionWithName:@"backdrop_composite"];
    descriptor.colorAttachments[0].pixelFormat=format;
    // Replace the accumulated scene with its blurred premultiplied version.
    // Subsequent GPUI primitives (including the modal) render normally above it.
    self.compositeState=[device newRenderPipelineStateWithDescriptor:descriptor error:&error];
    if(!self.horizontalState||!self.verticalState||!self.compositeState){NSLog(@"Del backdrop pipeline: %@",error);return NO;}
    self.pass=[MTLRenderPassDescriptor renderPassDescriptor];
    self.pass.colorAttachments[0].loadAction=MTLLoadActionDontCare;
    self.pass.colorAttachments[0].storeAction=MTLStoreActionStore;
    _horizontal=nil;_vertical=nil;
    return YES;
}
- (void)encode:(id<MTLCommandBuffer>)command scene:(id<MTLTexture>)scene {
    if(![self prepare:command.commandQueue.device format:scene.pixelFormat])return;
    NSUInteger width=(scene.width+3)/4,height=(scene.height+3)/4;
    if(!_horizontal||_horizontal.width!=width||_horizontal.height!=height){
        MTLTextureDescriptor *descriptor=[MTLTextureDescriptor texture2DDescriptorWithPixelFormat:MTLPixelFormatRGBA16Float width:width height:height mipmapped:NO];
        descriptor.storageMode=MTLStorageModePrivate;
        descriptor.usage=MTLTextureUsageShaderRead|MTLTextureUsageShaderWrite;
        _horizontal=[self.device newTextureWithDescriptor:descriptor];
        _vertical=[self.device newTextureWithDescriptor:descriptor];
    }
    simd_float4 params={self.scaleZoom,0,0,0};
    id<MTLComputeCommandEncoder> encoder=[command computeCommandEncoder];
    [encoder setBytes:&params length:sizeof(params) atIndex:0];
    [encoder setComputePipelineState:self.horizontalState];
    [encoder setTexture:scene atIndex:0];[encoder setTexture:_horizontal atIndex:1];
    [encoder dispatchThreads:MTLSizeMake(width,height,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];
    [encoder memoryBarrierWithScope:MTLBarrierScopeTextures];
    [encoder setComputePipelineState:self.verticalState];
    [encoder setTexture:_horizontal atIndex:0];[encoder setTexture:_vertical atIndex:1];
    [encoder dispatchThreads:MTLSizeMake(width,height,1) threadsPerThreadgroup:MTLSizeMake(8,8,1)];
    [encoder endEncoding];
    self.pass.colorAttachments[0].texture=scene;
    id<MTLRenderCommandEncoder> render=[command renderCommandEncoderWithDescriptor:self.pass];
    [render setRenderPipelineState:self.compositeState];
    [render setFragmentTexture:_vertical atIndex:0];
    [render drawPrimitives:MTLPrimitiveTypeTriangle vertexStart:0 vertexCount:3];
    [render endEncoding];
    // GPUI resumes on this same buffer with Load, before drawing the dialog.
}
@end
void *del_backdrop_create(const unsigned char *bytes,size_t length){
    DelBackdrop *renderer=[DelBackdrop new];renderer.libraryData=[NSData dataWithBytes:bytes length:length];
    return (__bridge_retained void *)renderer;
}
void del_backdrop_release(void *pointer){if(pointer){DelBackdrop *renderer=(__bridge_transfer DelBackdrop *)pointer;(void)renderer;}}
void *del_backdrop_marker(void *pointer,double scale,double zoom){
    if(!pointer)return NULL;
    DelBackdrop *renderer=(__bridge DelBackdrop *)pointer;renderer.scaleZoom=scale*zoom;
    CVPixelBufferRef marker=[renderer marker];return marker?(void *)CFRetain(marker):NULL;
}
bool del_backdrop_encode(const void *buffer,void *command,void *texture){
    if(!buffer)return false;
    DelBackdropOwner *owner=(__bridge DelBackdropOwner *)CVBufferGetAttachment((CVBufferRef)buffer,CFSTR("DelBackdropOwner"),NULL);
    if(!owner.renderer)return false;
    if(command&&texture)[owner.renderer encode:(__bridge id<MTLCommandBuffer>)command scene:(__bridge id<MTLTexture>)texture];
    return true;
}
