#include <metal_stdlib>
using namespace metal;
struct Params { float4 geometry; float4 pointer; float4 impulse; float4 clock; float4 life; float4 composer; float4 footer; float4 blur; };
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
struct Vertex { float4 position [[position]]; float2 uv; };
vertex Vertex fluid_vertex(uint id [[vertex_id]]) {
    float2 p=float2((id<<1)&2,id&2);
    return {float4(p*2.0f-1.0f,0,1),float2(p.x,1.0f-p.y)};
}
fragment float4 fluid_fragment(Vertex in [[stage_in]],texture2d<float,access::read> cells [[texture(0)]],texture2d<float,access::read> highlight [[texture(1)]],constant Params &u [[buffer(0)]]) {
    float2 logical=in.uv*u.geometry.xy;
    float2 lattice=in.uv*u.clock.zw;
    uint2 cell=min(uint2(lattice),uint2(u.clock.zw)-1);
    float4 state=cells.read(cell);
    float2 q=abs(fract(lattice)-0.5f);
    float aa=max(fwidth(q.x),fwidth(q.y))*0.5f;
    // Fixed ~7px squares retain their centers and gap, with a roughly one
    // logical-pixel soft rim. End the rim inside the cell to avoid offset seams.
    float pixels=1.0f-smoothstep(0.31f-aa,min(0.49f,0.46f+aa),max(q.x,q.y));
    float edge=smoothstep(0.0f,0.035f,min(min(in.uv.x,1-in.uv.x),min(in.uv.y,1-in.uv.y)));
    float heat=clamp(highlight.read(cell).x,0.0f,1.0f);
    // One constant orange hue. No spatial/time color field, multistop hue
    // blending or moving brightness bands; hover only raises the same alpha.
    float3 color=float3(0.86f,0.37f,0.10f);
    if(u.clock.y>0.5f)color*=0.70f;
    // Straight RGB; GPUI applies surface alpha once. Preserve only the cell's
    // birth/death transition, soft rim and page-edge masks.
    // Linear opacity only: full animation strength at pane top, zero at
    // bottom. Applied before offscreen Gaussian so sharp/blurred cells agree.
    float verticalFade=1.0f-clamp(in.uv.y,0.0f,1.0f);
    float alpha=state.z*mix(0.10f,0.19f,heat)*pixels*edge*verticalFade;
    float2 halfSize=u.geometry.xy*0.5f;
    float radius=u.geometry.w;
    float2 rounded=abs(logical-halfSize)-halfSize+radius;
    float sdf=length(max(rounded,0.0f))+min(max(rounded.x,rounded.y),0.0f)-radius;
    alpha*=1.0f-smoothstep(-0.5f,0.5f,sdf);
    return float4(color,clamp(alpha,0.0f,0.85f));
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
