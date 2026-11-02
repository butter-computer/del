#include <metal_stdlib>
using namespace metal;
struct Params { float4 geometry; float4 pointer; float4 impulse; float4 clock; float4 life; float4 composer; float4 footer; float4 blur; float4 tint; float4 animation; float4 stroke; };
// Stable integer hashes are used only for initial soup / rare localized reseeds.
// The visible state between those events is determined exclusively by Conway.
float randomCell(uint2 p,uint seed) {
    uint h=p.x*374761393u+p.y*668265263u+seed*2246822519u;
    h=(h^(h>>13))*1274126177u;h^=h>>16;
    return float(h&0x00ffffffu)/16777216.0f;
}
kernel void fluid_clear(texture2d<float,access::write> a [[texture(0)]],
 texture2d<float,access::write> b [[texture(1)]],texture2d<float,access::write> h0 [[texture(2)]],
 texture2d<float,access::write> h1 [[texture(3)]],constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    if(id.x>=a.get_width()||id.y>=a.get_height())return;
    uint seed=uint(u.life.w);
    uint2 patch=id/20,local=id%20;
    // Distributed 16x16 patches of genuine 22% random soup. Keep small empty
    // channels between patches, without masking the entire composer region.
    bool patchPresent=randomCell(patch,seed+81)<0.72f;
    bool inside=all(local>=uint2(2))&&all(local<uint2(18));
    bool alive=patchPresent&&inside&&randomCell(id,seed)<0.22f;
    float4 state=float4(alive?1:0,0,alive?1:0,0);
    a.write(state,id);b.write(state,id);h0.write(float4(0),id);h1.write(float4(0),id);
}
uint neighborAlive(texture2d<float,access::read> cells,int2 p) {
    // Dead exterior, no edge clamping (which would count edge cells twice).
    if(any(p<int2(0))||p.x>=int(cells.get_width())||p.y>=int(cells.get_height()))return 0;
    return cells.read(uint2(p)).x>0.5f?1u:0u;
}
// RGBA: exact alive bit, age, display transition, time since death. The generation
// reads ONLY input alive bits and writes a separate texture. No in-place rules.
kernel void cellular_life(texture2d<float,access::read> input [[texture(0)]],
 texture2d<float,access::write> output [[texture(1)]],texture2d<float,access::read> highlight [[texture(2)]],
 texture2d<float,access::write> nextHighlight [[texture(3)]],constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    if(any(float2(id)>=u.clock.zw))return;
    float4 previous=input.read(id);
    bool living=previous.x>0.5f,next=living;
    float age=previous.y;
    if(u.life.x>0.5f) {
        uint neighbors=0;
        for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++)
            if(x!=0||y!=0)neighbors+=neighborAlive(input,int2(id)+int2(x,y));
        next=neighbors==3u||(living&&neighbors==2u); // Conway B3 / S23
        age=next?(living?min(age+0.09f,60.0f):0.0f):0.0f;
        // At most ONE 10x10 random patch every 30 seconds, never a global
        // reshuffle. All other cells retain their exact neighbor-driven result.
        uint generation=uint(u.life.y);
        if(generation>0u&&generation%334u==0u) {
            uint2 patchCount=(uint2(u.clock.zw)+19u)/20u;
            uint count=patchCount.x*patchCount.y;
            uint chosen=min(uint(randomCell(uint2(generation,7),uint(u.life.w))*float(count)),count-1u);
            uint2 patch=id/20,local=id%20;
            if(patch.y*patchCount.x+patch.x==chosen&&all(local>=uint2(5))&&all(local<uint2(15))) {
                next=randomCell(id,uint(u.life.w)+generation)<0.22f;age=0;
            }
        }
    }
    // Exact topology still advances at 90ms. Only visual energy interpolates:
    // births reach ~95% in 67ms; deaths become a dim tail before smoothly
    // reaching zero at 200ms, rather than ending at an abrupt threshold.
    float deathAge=next?0.0f:min(living?u.impulse.w:previous.w+u.impulse.w,0.20f);
    float display;
    if(next)display=mix(previous.z,1.0f,1.0f-exp(-u.impulse.w*45.0f));
    else display=previous.z*exp(-u.impulse.w*16.0f)*(1.0f-smoothstep(0.12f,0.20f,deathAge));
    output.write(float4(next?1:0,age,display,deathAge),id);
    // Hover is isolated color state, never read by any neighborhood/seed rule.
    float2 logical=(float2(id)+0.5f)/u.clock.zw*u.geometry.xy;
    float distance=length(logical-u.pointer.zw*u.geometry.xy)/max(u.geometry.z,0.1f);
    float colorTarget=u.life.z*exp(-0.5f*distance*distance/(36.0f*36.0f));
    float heat=highlight.read(id).x;
    heat=mix(heat,colorTarget,1.0f-exp(-u.impulse.w*(colorTarget>heat?20.0f:3.8f)));
    nextHighlight.write(float4(heat),id);
}
// Damped wave equation: RG stores surface height and vertical velocity.
// Pointer strokes inject a depression behind the cursor and a crest ahead;
// propagation continues without pointer input and naturally settles.
kernel void liquid_step(texture2d<float,access::read> input [[texture(0)]],
 texture2d<float,access::write> output [[texture(1)]],constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    if(any(float2(id)>=u.clock.zw))return;
    if(u.impulse.z>0.5f) {output.write(float4(0),id);return;}
    int2 size=int2(u.clock.zw),p=int2(id);
    float2 state=input.read(id).xy;
    float left=input.read(uint2(max(p-int2(1,0),int2(0)))).x;
    float right=input.read(uint2(min(p+int2(1,0),size-1))).x;
    float up=input.read(uint2(max(p-int2(0,1),int2(0)))).x;
    float down=input.read(uint2(min(p+int2(0,1),size-1))).x;
    float2 extent=u.geometry.xy/max(u.geometry.z,0.1f);
    float2 spacing=extent/u.clock.zw;
    float laplacian=(left+right-2.0f*state.x)/(spacing.x*spacing.x)
        +(up+down-2.0f*state.x)/(spacing.y*spacing.y);
    float dt=u.impulse.w;
    float velocity=(state.y+140.0f*140.0f*laplacian*dt)*exp(-3.2f*dt);
    float height=state.x+velocity*dt;
    if(u.impulse.x>0) {
        float2 position=(float2(id)+0.5f)/u.clock.zw*extent;
        float2 start=u.stroke.xy*extent,end=u.stroke.zw*extent;
        float2 path=end-start;
        float along=clamp(dot(position-start,path)/max(dot(path,path),0.001f),0.0f,1.0f);
        float2 delta=position-mix(start,end,along);
        float2 direction=normalize(path);
        float front=dot(position-end,direction)/24.0f;
        float wake=exp(-dot(delta,delta)/(2.0f*24.0f*24.0f));
        height+=0.09f*(u.impulse.x/24.0f)*wake*clamp(front,-1.0f,1.0f);
    }
    // Absorb waves at the edges instead of reflecting them back into the panel.
    float border=min(min(float(id.x),float(id.y)),min(float(size.x-1-p.x),float(size.y-1-p.y)));
    float damping=exp(-dt*12.0f*(1.0f-smoothstep(0.0f,5.0f,border)));
    output.write(float4(clamp(height*damping,-0.45f,0.45f),velocity*damping,0,0),id);
}
struct Vertex { float4 position [[position]]; float2 uv; };
vertex Vertex fluid_vertex(uint id [[vertex_id]]) {
    float2 p=float2((id<<1)&2,id&2);
    return {float4(p*2.0f-1.0f,0,1),float2(p.x,1.0f-p.y)};
}
// Smooth value noise: stable spatial detail, no per-frame reseeding or flicker.
float ambientNoise(float2 p) {
    float2 cell=floor(p),f=fract(p);
    f=f*f*(3.0f-2.0f*f);
    uint2 id=uint2(cell+8192.0f);
    float a=randomCell(id,71u),b=randomCell(id+uint2(1,0),71u);
    float c=randomCell(id+uint2(0,1),71u),d=randomCell(id+uint2(1,1),71u);
    return mix(mix(a,b,f.x),mix(c,d,f.x),f.y);
}
float liquidHeight(float2 p,float time) {
    float2 q=p/170.0f;
    float2 flow=float2(time*0.035f,-time*0.024f);
    float2 warp=float2(ambientNoise(q*0.65f+flow),ambientNoise(q*0.65f-flow+17.0f));
    q+=1.5f*(warp-0.5f);
    return 0.72f*ambientNoise(q+flow)
        +0.20f*ambientNoise(q*2.0f-flow*0.7f)
        +0.08f*ambientNoise(q*4.0f+flow*0.3f);
}
// Analytic backgrounds share the same clock, theme tint, clipping and blur pass.
// Coordinates are logical pixels, so pattern sizes remain stable under zoom.
float backgroundField(float2 p,float time,uint style,float2 distortion,float disturbance) {
    if(style==3u) { // Liquid: domain-warped surface with moving folds and specular sheen.
        float height=liquidHeight(p,time)+disturbance;
        float2 gradient=float2(dfdx(height),dfdy(height))/max(fwidth(p),float2(0.001f))*150.0f;
        float3 normal=normalize(float3(-gradient,0.75f));
        float3 light=normalize(float3(-0.55f,-0.65f,0.65f));
        float diffuse=max(dot(normal,light),0.0f);
        float sheen=pow(max(dot(normal,normalize(light+float3(0,0,1))),0.0f),22.0f);
        float body=smoothstep(0.28f,0.72f,height);
        float fold=exp(-pow((height-0.52f)/0.045f,2.0f));
        return clamp(0.08f+body*diffuse*0.45f+sheen*0.70f+fold*0.18f,0.0f,1.0f);
    }
    if(style==4u) { // Contours: continuous, antialiased topographic lines.
        float2 q=(p-distortion)/150.0f;
        float height=sin(q.x+time*0.055f)+cos(q.y*0.85f-time*0.04f)
            +0.45f*sin(q.x*0.7f+q.y*1.2f+time*0.035f);
        float phase=height*2.2f;
        float distance=abs(fract(phase)-0.5f);
        return (1.0f-smoothstep(0.012f,0.012f+max(fwidth(phase),0.008f),distance))*0.75f;
    }
    if(style==5u) { // Drift: two sparse layers moving upward without blinking.
        float field=0;
        for(uint layer=0;layer<2;layer++) {
            float pitch=layer==0?78.0f:115.0f;
            float2 q=(p-distortion)/pitch+float2(0.025f*sin(time*0.07f),time*(layer==0?0.035f:0.021f));
            uint2 cell=uint2(floor(q)+4096.0f);
            float seed=randomCell(cell,layer+217u);
            float2 center=float2(0.2f+0.6f*seed,0.2f+0.6f*randomCell(cell,layer+573u));
            float distance=length(fract(q)-center)*pitch;
            float radius=layer==0?1.6f:1.0f;
            field+=(1.0f-smoothstep(radius,radius+1.0f,distance))*step(0.55f,seed)*(layer==0?1.0f:0.5f);
        }
        return min(field,1.0f);
    }
    if(style==6u) { // Fluted glass: fixed cylindrical ribs refract a slow moving light field.
        const float pitch=24.0f;
        float rib=fract(p.x/pitch);
        float angle=(rib-0.5f)*2.7f;
        float curve=cos(angle),slope=sin(angle);
        // Compress and displace the scene behind each rib; glass geometry stays still.
        float2 refracted=float2(p.x+slope*32.0f,p.y+curve*7.0f)-distortion*1.5f;
        float lightField=liquidHeight(refracted*0.72f+float2(time*2.0f,0),time*0.7f);
        float illumination=smoothstep(0.24f,0.75f,lightField);
        float edgeDistance=min(rib,1.0f-rib);
        float seam=1.0f-smoothstep(0.015f,0.06f,edgeDistance);
        float highlight=exp(-pow((rib-0.19f)/0.065f,2.0f));
        float transmitted=illumination*(0.2f+0.5f*curve);
        return clamp(transmitted*(1.0f-seam*0.8f)+highlight*(0.12f+illumination*0.35f),0.0f,1.0f);
    }
    return 0;
}
fragment float4 fluid_fragment(Vertex in [[stage_in]],texture2d<float,access::read> cells [[texture(0)]],texture2d<float,access::read> highlight [[texture(1)]],texture2d<float> liquid [[texture(2)]],constant Params &u [[buffer(0)]]) {
    float2 logical=in.uv*u.geometry.xy;
    uint style=uint(u.animation.x);
    float field=0;
    if(style==2u) { // Conway Life retains exact B3/S23 topology.
        float2 lattice=in.uv*u.clock.zw;
        uint2 cell=min(uint2(lattice),uint2(u.clock.zw)-1);
        float4 state=cells.read(cell);
        float2 q=abs(fract(lattice)-0.5f);
        float aa=max(fwidth(q.x),fwidth(q.y))*0.5f;
        float pixels=1.0f-smoothstep(0.31f-aa,min(0.49f,0.46f+aa),max(q.x,q.y));
        float heat=u.animation.y>0.5f?clamp(highlight.read(cell).x,0.0f,1.0f):0.0f;
        field=state.z*mix(0.65f,1.0f,heat)*pixels;
    } else {
        float zoom=max(u.geometry.z,0.1f);
        float2 p=logical/zoom;
        float2 delta=p-u.pointer.zw*u.geometry.xy/zoom;
        float influence=exp(-dot(delta,delta)/(2.0f*110.0f*110.0f))*u.animation.z;
        float2 distortion=delta*(0.18f*influence);
        constexpr sampler waterSampler(coord::normalized,address::clamp_to_edge,filter::linear);
        float disturbance=style==3u&&u.animation.y>0.5f?liquid.sample(waterSampler,in.uv).x:0.0f;
        field=backgroundField(p,u.clock.x,style,distortion,disturbance);
        if(style!=3u)field=min(field*(1.0f+0.25f*influence),1.0f);
    }
    float edge=smoothstep(0.0f,0.035f,min(min(in.uv.x,1-in.uv.x),min(in.uv.y,1-in.uv.y)));
    float verticalFade=1.0f-clamp(in.uv.y,0.0f,1.0f);
    float alpha=field*u.tint.a*edge*verticalFade;
    float2 halfSize=u.geometry.xy*0.5f;
    float radius=u.geometry.w;
    float2 rounded=abs(logical-halfSize)-halfSize+radius;
    float sdf=length(max(rounded,0.0f))+min(max(rounded.x,rounded.y),0.0f)-radius;
    alpha*=1.0f-smoothstep(-0.5f,0.5f,sdf);
    return float4(u.tint.rgb,clamp(alpha,0.0f,0.20f));
}

constexpr sampler sceneSampler(coord::normalized,address::clamp_to_edge,filter::linear);
// Separable 13-tap Gaussian at half resolution, sigma about 5 logical pixels.
// Accumulate premultiplied color to avoid dark fringes in transparent content.
kernel void cell_blur_horizontal(texture2d<float,access::sample> source [[texture(0)]],
 texture2d<float,access::write> output [[texture(1)]],constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    if(id.x>=output.get_width()||id.y>=output.get_height())return;
    float2 uv=(float2(id)+0.5f)/float2(output.get_width(),output.get_height());
    float2 step=float2(2.0f*u.blur.z*u.geometry.z/float(source.get_width()),0);
    float4 sum=0;float total=0;
    for(int i=-6;i<=6;i++){
        float weight=exp(-float(i*i)/12.5f);
        float4 sample=source.sample(sceneSampler,uv+step*float(i));
        sum+=float4(sample.rgb*sample.a,sample.a)*weight;total+=weight;
    }
    output.write(sum/total,id);
}
kernel void cell_blur_vertical(texture2d<float,access::sample> source [[texture(0)]],
 texture2d<float,access::write> output [[texture(1)]],constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    if(id.x>=output.get_width()||id.y>=output.get_height())return;
    float2 uv=(float2(id)+0.5f)/float2(output.get_width(),output.get_height());
    float2 step=float2(0,u.blur.z*u.geometry.z/float(source.get_height()));
    float4 sum=0;float total=0;
    for(int i=-6;i<=6;i++){
        float weight=exp(-float(i*i)/12.5f);
        sum+=source.sample(sceneSampler,uv+step*float(i))*weight;total+=weight;
    }
    output.write(sum/total,id);
}
float regionMask(float2 p,float4 rect,float radius,bool bottomOnly,float zoom) {
    if(rect.z<=0||rect.w<=0)return 0;
    float2 center=rect.xy+rect.zw*0.5f;
    radius=min(radius,min(rect.z,rect.w)*0.5f);
    if(bottomOnly&&p.y<center.y)radius=0;
    float2 q=abs(p-center)-rect.zw*0.5f+radius;
    float sdf=length(max(q,0.0f))+min(max(q.x,q.y),0.0f)-radius;
    return 1.0f-smoothstep(-1.5f*zoom,0.5f*zoom,sdf);
}
fragment float4 cell_composite(Vertex in [[stage_in]],texture2d<float> scene [[texture(0)]],
 texture2d<float> blurred [[texture(1)]],constant Params &u [[buffer(0)]]) {
    float2 p=in.uv*u.geometry.xy;
    float mask=max(regionMask(p,u.composer,u.blur.x,false,u.geometry.z),
                   regionMask(p,u.footer,u.blur.y,true,u.geometry.z));
    float4 sharp=scene.sample(sceneSampler,in.uv);
    float4 soft=blurred.sample(sceneSampler,in.uv); // premultiplied Gaussian
    float4 result=mix(float4(sharp.rgb*sharp.a,sharp.a),soft,mask);
    return float4(result.rgb/max(result.a,0.00001f),result.a);
}

// Full GPUI scene backdrop, already premultiplied by the renderer. Blur only
// primitives submitted before the marker; modal text is submitted afterward.
kernel void backdrop_horizontal(texture2d<float,access::sample> source [[texture(0)]],
 texture2d<float,access::write> output [[texture(1)]],constant float4 &params [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    if(id.x>=output.get_width()||id.y>=output.get_height())return;
    float2 uv=(float2(id)+0.5f)/float2(output.get_width(),output.get_height());
    float2 offset=float2(4.0f*params.x/float(source.get_width()),0);
    float4 sum=0;float total=0;
    for(int i=-20;i<=20;i++){
        float weight=exp(-float(i*i)/98.0f);
        sum+=source.sample(sceneSampler,uv+offset*float(i))*weight;total+=weight;
    }
    output.write(sum/total,id);
}
kernel void backdrop_vertical(texture2d<float,access::sample> source [[texture(0)]],
 texture2d<float,access::write> output [[texture(1)]],constant float4 &params [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    if(id.x>=output.get_width()||id.y>=output.get_height())return;
    float2 uv=(float2(id)+0.5f)/float2(output.get_width(),output.get_height());
    float2 offset=float2(0,params.x/float(source.get_height()));
    float4 sum=0;float total=0;
    for(int i=-20;i<=20;i++){
        float weight=exp(-float(i*i)/98.0f);
        sum+=source.sample(sceneSampler,uv+offset*float(i))*weight;total+=weight;
    }
    output.write(sum/total,id);
}
fragment float4 backdrop_composite(Vertex in [[stage_in]],texture2d<float> blurred [[texture(0)]]) {
    return blurred.sample(sceneSampler,in.uv);
}
