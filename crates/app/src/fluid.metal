#include <metal_stdlib>
using namespace metal;
struct Params { float4 geometry; float4 pointer; float4 impulse; float4 clock; float4 life; float4 composer; float4 footer; float4 blur; float4 tint; float4 animation; float4 stroke; float4 rain[64]; float lean[64]; float4 rails[3]; float4 footerSide; };
// Stable integer hashes are used only for initial soup / rare localized reseeds.
// The visible state between those events is determined exclusively by Conway.
float randomCell(uint2 p,uint seed) {
    uint h=p.x*374761393u+p.y*668265263u+seed*2246822519u;
    h=(h^(h>>13))*1274126177u;h^=h>>16;
    return float(h&0x00ffffffu)/16777216.0f;
}
float2 extentOf(constant Params &u) { return u.geometry.xy/max(u.geometry.z,0.1f); }
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
        // A moving cursor is a brush: it scatters living cells, which Conway then takes
        // over. Only ever on the page and setting that allow it, and never at rest.
        if(u.animation.y>0.5f&&u.life.z>0.05f&&length(u.impulse.xy)>24.0f) {
            float2 from=((float2(id)+0.5f)/u.clock.zw-u.pointer.zw)*extentOf(u);
            float reach=dot(from,from)/(2.0f*22.0f*22.0f);
            if(reach<4.0f&&randomCell(id,uint(u.life.w)+uint(u.life.y)*7u+3u)<0.42f*exp(-reach)) {
                if(!next)age=0;
                next=true;
            }
        }
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
    // The cursor's glow is colour only, never read by any neighbourhood rule.
    float2 logical=(float2(id)+0.5f)/u.clock.zw*u.geometry.xy;
    float distance=length(logical-u.pointer.zw*u.geometry.xy)/max(u.geometry.z,0.1f);
    float colorTarget=u.life.z*exp(-0.5f*distance*distance/(52.0f*52.0f));
    float heat=highlight.read(id).x;
    heat=mix(heat,colorTarget,1.0f-exp(-u.impulse.w*(colorTarget>heat?20.0f:3.8f)));
    nextHighlight.write(float4(heat),id);
}
// ---------------------------------------------------------------------------
// Shared math. Every simulated style works in zoom-independent logical pixels
// ("p-space"), with +x right, +y down and +z towards the viewer.
// ---------------------------------------------------------------------------
float3 latticeGradient(int3 c,uint seed) {
    uint h=uint(c.x)*374761393u+uint(c.y)*668265263u+uint(c.z)*2147483647u+seed*2246822519u;
    h=(h^(h>>13))*1274126177u;h^=h>>16;
    float z=float(h&0xffffu)/32767.5f-1.0f,a=float(h>>16)/65536.0f*6.2831853f;
    float r=sqrt(max(1.0f-z*z,0.0f));
    return float3(r*cos(a),r*sin(a),z);
}
// Quintic-interpolated gradient noise: C2 continuous, so isolines and normals
// derived from it never show the grid the way value noise does. Range about ±0.7.
float gradientNoise(float3 p,uint seed=0) {
    float3 i=floor(p),f=p-i,w=f*f*f*(f*(f*6.0f-15.0f)+10.0f);
    int3 c=int3(i);
    float n000=dot(latticeGradient(c,seed),f);
    float n100=dot(latticeGradient(c+int3(1,0,0),seed),f-float3(1,0,0));
    float n010=dot(latticeGradient(c+int3(0,1,0),seed),f-float3(0,1,0));
    float n110=dot(latticeGradient(c+int3(1,1,0),seed),f-float3(1,1,0));
    float n001=dot(latticeGradient(c+int3(0,0,1),seed),f-float3(0,0,1));
    float n101=dot(latticeGradient(c+int3(1,0,1),seed),f-float3(1,0,1));
    float n011=dot(latticeGradient(c+int3(0,1,1),seed),f-float3(0,1,1));
    float n111=dot(latticeGradient(c+int3(1,1,1),seed),f-float3(1,1,1));
    return mix(mix(mix(n000,n100,w.x),mix(n010,n110,w.x),w.y),mix(mix(n001,n101,w.x),mix(n011,n111,w.x),w.y),w.z);
}
float fractalNoise(float3 p,int octaves,uint seed=0) {
    float sum=0,amplitude=0.5f;
    for(int i=0;i<octaves;i++){sum+=amplitude*gradientNoise(p,seed+uint(i));p=p*2.03f+11.7f;amplitude*=0.5f;}
    return sum;
}
float interleavedGradient(float2 p) { return fract(52.9829189f*fract(dot(p,float2(0.06711056f,0.00583715f)))); }
float4 regionOf(float4 rect,constant Params &u) { return rect/max(u.geometry.z,0.1f); }
float4 toClip(float2 p,float2 extent,float depth=0.5f) { return float4(p.x/extent.x*2.0f-1.0f,1.0f-p.y/extent.y*2.0f,depth,1); }
constant float4 offscreenVertex=float4(4,4,0,1);

// Signed distance to the input bar or footer, matching weather.h exactly.
float barDistance(float2 p,float4 rect,float radius) {
    if(rect.z<=0||rect.w<=0)return 1e9f;
    float r=min(radius,min(rect.z,rect.w)*0.5f);
    float2 q=abs(p-(rect.xy+rect.zw*0.5f))-rect.zw*0.5f+r;
    return length(max(q,0.0f))+min(max(q.x,q.y),0.0f)-r;
}

// B-spline reconstruction with analytic derivatives. Reading a coarse simulation
// grid through it gives C2 surfaces: smooth normals and a true Laplacian, with
// none of the facets or diamonds that bilinear sampling draws into highlights.
struct Surface { float height; float2 gradient; float laplacian; };
Surface splineSurface(texture2d<float> field,float2 uv,float2 spacing) {
    int2 size=int2(field.get_width(),field.get_height());
    float2 g=uv*float2(size)-0.5f,cell=floor(g),t=g-cell;
    float2 t2=t*t,t3=t2*t;
    float2 w[4]={(1-3*t+3*t2-t3)/6,(4-6*t2+3*t3)/6,(1+3*t+3*t2-3*t3)/6,t3/6};
    float2 d[4]={(-1+2*t-t2)/2,(-4*t+3*t2)/2,(1+2*t-3*t2)/2,t2/2};
    float2 s[4]={1-t,3*t-2,1-3*t,t};
    Surface out={0,float2(0),0};
    int2 base=int2(cell);
    float dxx=0,dyy=0;
    for(int y=0;y<4;y++)for(int x=0;x<4;x++) {
        float value=field.read(uint2(clamp(base+int2(x-1,y-1),int2(0),size-1))).x;
        out.height+=value*w[x].x*w[y].y;
        out.gradient+=value*float2(d[x].x*w[y].y,w[x].x*d[y].y);
        dxx+=value*s[x].x*w[y].y;
        dyy+=value*w[x].x*s[y].y;
    }
    out.gradient/=spacing;
    out.laplacian=dxx/(spacing.x*spacing.x)+dyy/(spacing.y*spacing.y);
    return out;
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
// Liquid: domain-warped surface with moving folds and specular sheen.
float liquidField(float2 p,float time,float disturbance) {
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

// ---------------------------------------------------------------------------
// Curtains: position-based cloth in 3D. Three panels of real fabric hang from a
// rail. More fabric than rail (the gather) is what forms the folds: nothing
// prescribes them below the pins. Gravity, fabric drag and aerodynamic pressure
// on each surface normal drive it; distance, shear and bending constraints and
// long-range attachments hold it together without visible stretch.
// ---------------------------------------------------------------------------
constant int CLOTH_PANELS=3,CLOTH_PANEL_COLUMNS=120,CLOTH_ROWS=180;
constant int CLOTH_COLUMNS=CLOTH_PANELS*CLOTH_PANEL_COLUMNS;
// However hard the cursor sweeps: the strongest wind it can raise, and the furthest any
// point may swing from below its pin. Slow moves stay fully responsive; fast ones saturate,
// so fabric can never whip round itself and clip through its own folds.
constant float CLOTH_HAND_WIND=760.0f,CLOTH_SWAY=70.0f,CLOTH_SWAY_DROP=0.20f,CLOTH_TOP_SPEED=800.0f,CLOTH_CALM_SPEED=250.0f,CLOTH_LEAST_DROP=0.84f;
struct ClothRig { float panelWidth,gap,left,restU,restV,amplitude; };
ClothRig clothRig(float2 extent,float style) {
    ClothRig r;
    // Window Curtains hang further apart, so the window shows through open gaps.
    bool open=style>11.5f&&style<12.5f;
    r.left=extent.x*(open?0.012f:0.006f);r.gap=extent.x*(open?0.105f:0.024f);
    r.panelWidth=(extent.x-2.0f*r.left-2.0f*r.gap)/float(CLOTH_PANELS);
    float wavelength=r.panelWidth/5.0f;
    r.restU=r.panelWidth/float(CLOTH_PANEL_COLUMNS-1)*1.72f;
    r.restV=(extent.y+70.0f)/float(CLOTH_ROWS-1);
    r.amplitude=wavelength*0.36f;
    return r;
}
// Where each thread is pinned. Window Curtains' hooks can be dragged along the rail (rails:
// left, held hook and right as shares of the width, then how far along the hook is). Fabric
// doesn't change length, so where hooks bunch up the pleats between them run deeper.
float3 clothRail(int column,float2 extent,constant Params &u) {
    ClothRig r=clothRig(extent,u.animation.x);
    int panel=column/CLOTH_PANEL_COLUMNS;
    float s=float(column%CLOTH_PANEL_COLUMNS)/float(CLOTH_PANEL_COLUMNS-1);
    // The middle panel hangs on a rail further back, so panels overlap in depth.
    float layer=panel==1?-34.0f:0.0f;
    // Hand-hung pleats: uneven spacing and depth, so the rail never reads as a comb.
    float warped=s+0.035f*sin(s*11.0f+float(panel)*2.3f)+0.018f*sin(s*23.0f+float(panel)*4.1f);
    float depth=r.amplitude*(0.72f+0.38f*sin(s*7.0f+float(panel)*1.1f));
    float x=r.left+float(panel)*(r.panelWidth+r.gap)+s*r.panelWidth;
    float4 rail=u.rails[panel];
    if(u.animation.x>11.5f&&u.animation.x<12.5f&&rail.z>rail.x) {
        float left=rail.x*extent.x,hook=rail.y*extent.x,right=rail.z*extent.x,at=rail.w;
        float before=(hook-left)/max(at,0.001f),after=(right-hook)/max(1.0f-at,0.001f);
        x=s<=at?left+s*before:hook+(s-at)*after;
        float spacing=mix(before,after,smoothstep(at-0.08f,at+0.08f,s));
        depth*=clamp(1.0f+0.50f*(r.panelWidth/max(spacing,1.0f)-1.0f),0.65f,1.8f);
    }
    return float3(x,-6.0f,layer+depth*sin(warped*5.0f*6.2831853f+float(panel)*1.7f));
}
bool sameCloth(int2 a,int2 b) {
    return b.x>=0&&b.y>=0&&b.x<CLOTH_COLUMNS&&b.y<CLOTH_ROWS&&a.x/CLOTH_PANEL_COLUMNS==b.x/CLOTH_PANEL_COLUMNS;
}
float3 clothNormal(texture2d<float> position,int2 id) {
    // Two-cell stencil: smooth shading across the mesh without losing the fold shapes.
    int2 l=id-int2(2,0),r=id+int2(2,0),t=id-int2(0,2),b=id+int2(0,2);
    if(!sameCloth(id,l))l=id; if(!sameCloth(id,r))r=id; if(!sameCloth(id,t))t=id; if(!sameCloth(id,b))b=id;
    float3 dx=position.read(uint2(r)).xyz-position.read(uint2(l)).xyz;
    float3 dy=position.read(uint2(b)).xyz-position.read(uint2(t)).xyz;
    float3 n=cross(dy,dx);
    float m=length(n);
    return m>0.0001f?n/m:float3(0,0,1);
}
kernel void cloth_init(texture2d<float,access::write> position [[texture(0)]],texture2d<float,access::write> velocity [[texture(1)]],
 constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    float2 extent=extentOf(u);
    float3 rail=clothRail(int(id.x),extent,u);
    ClothRig r=clothRig(extent,u.animation.x);
    float descent=float(id.y)*r.restV,fall=descent/(extent.y+70.0f);
    // Fabric never falls plumb: pleats wander as they descend and open towards the hem.
    float s=float(int(id.x)%CLOTH_PANEL_COLUMNS)/float(CLOTH_PANEL_COLUMNS-1)-0.5f;
    float3 q=float3(float(id.x)/46.0f,descent/240.0f,1.0f);
    float3 wander=float3(gradientNoise(q,61u)*46.0f+s*r.panelWidth*0.10f,0,gradientNoise(q,67u)*38.0f)*fall;
    position.write(float4(rail.x+wander.x,rail.y+descent,rail.z+wander.z,1),id);
    velocity.write(float4(0),id);
}
// One gust of air: long calm stretches, then a front that sweeps across the room.
float gustEnvelope(float t) {
    float slow=gradientNoise(float3(t*0.11f,3.7f,1.3f))*1.5f+0.5f;
    return 0.30f+0.95f*smoothstep(0.42f,0.86f,slow);
}
kernel void cloth_predict(texture2d<float> position [[texture(0)]],texture2d<float> velocity [[texture(1)]],
 texture2d<float,access::write> predicted [[texture(2)]],constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    float2 extent=extentOf(u);
    float3 x=position.read(id).xyz;
    if(id.y==0) {predicted.write(float4(clothRail(int(id.x),extent,u),1),id);return;}
    float dt=u.impulse.w,t=u.clock.x;
    float3 v=velocity.read(id).xyz;
    float3 normal=clothNormal(position,int2(id));
    float gust=gustEnvelope(t-x.x/620.0f);
    float3 q=float3(x.x/190.0f-t*0.46f,x.y/230.0f,t*0.31f);
    float3 air=float3(gradientNoise(q,3u)*0.75f+0.06f,gradientNoise(q,9u)*0.22f,gradientNoise(q,17u)*1.9f+0.12f)*gust*360.0f;
    // Thin fabric is pushed along its surface normal by the relative air speed;
    // moving through still air is what damps it, so motion looks floaty, not springy.
    // A sweeping hand drags the fabric along with it.
    float reaction=u.animation.z;
    if(reaction>0.01f) {
        float2 hand=x.xy-u.pointer.zw*extent;
        float sweep=length(u.impulse.xy);
        float wind=CLOTH_HAND_WIND*(1.0f-exp(-sweep*2.6f/CLOTH_HAND_WIND));
        if(sweep>0.01f)air.xy+=u.impulse.xy/sweep*wind*exp(-dot(hand,hand)/(2.0f*80.0f*80.0f))*reaction;
    }
    float3 relative=air-v;
    float3 accel=normal*dot(normal,relative)*abs(dot(normal,relative))*0.024f+relative*0.08f+float3(0,1500.0f,0);
    v+=accel*dt;
    // life.w: this panel was just let go. Thick air lets it float down instead of whipping.
    float settling=int(id.x)/CLOTH_PANEL_COLUMNS==int(u.life.x)/CLOTH_PANEL_COLUMNS?u.life.w:0.0f;
    v*=exp(-(0.65f+8.0f*settling)*dt);
    x+=v*dt;
    predicted.write(float4(x,1),id);
}
bool hemColumn(int column) {
    int local=column%CLOTH_PANEL_COLUMNS;
    return min(local,CLOTH_PANEL_COLUMNS-1-local)<7;
}
kernel void cloth_relax(texture2d<float,access::read> input [[texture(0)]],texture2d<float,access::write> output [[texture(1)]],
 constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    int2 p=int2(id);
    float3 x=input.read(id).xyz;
    if(p.y==0) {output.write(float4(x,1),id);return;}
    float2 extent=extentOf(u);
    ClothRig r=clothRig(extent,u.animation.x);
    float diagonal=length(float2(r.restU,r.restV));
    // Structure, shear, then bending over short and long spans. The long weft span is
    // what makes fabric roll into round tubes rather than creasing flat like paper.
    const int2 offsets[16]={int2(1,0),int2(-1,0),int2(0,1),int2(0,-1),int2(1,1),int2(-1,1),int2(1,-1),int2(-1,-1),int2(2,0),int2(-2,0),int2(0,2),int2(0,-2),int2(4,0),int2(-4,0),int2(8,0),int2(-8,0)};
    float rest[16]={r.restU,r.restU,r.restV,r.restV,diagonal,diagonal,diagonal,diagonal,r.restU*1.92f,r.restU*1.92f,r.restV*2.0f,r.restV*2.0f,r.restU*3.55f,r.restU*3.55f,r.restU*5.4f,r.restU*5.4f};
    float stiffness[16]={1,1,1,1,0.5f,0.5f,0.5f,0.5f,0.55f,0.55f,0.55f,0.55f,0.42f,0.42f,0.22f,0.22f};
    float3 correction=0;float weight=0;
    for(int i=0;i<16;i++) {
        int2 n=p+offsets[i];
        if(!sameCloth(p,n))continue;
        float3 delta=input.read(uint2(n)).xyz-x;
        float length2=length(delta);
        if(length2<0.0001f)continue;
        // Side hems: a stiff sewn tape down each free edge that lies flat, never rippling.
        bool hem=(i>=8&&i<10||i>=12&&i<14)&&hemColumn(p.x)&&hemColumn(n.x);
        // Elsewhere fabric folds freely but does not stretch: resist bending only under compression.
        if(!hem&&((i>=8&&i<10)||i>=12)&&length2>rest[i])continue;
        float k=hem?1.0f:stiffness[i];
        float target=hem?abs(float(offsets[i].x))*r.restU*0.985f:rest[i];
        float share=n.y==0?1.0f:0.5f;
        correction+=delta*(1.0f-target/length2)*share*k;
        weight+=k;
    }
    x+=correction/max(weight,0.001f)*1.45f;
    // Woven cloth cannot hold a kink one thread wide: pull each point towards the line
    // between its neighbours along the warp, so tight folds stay smooth curves.
    if(p.y+1<CLOTH_ROWS) {
        float3 warp=(input.read(uint2(p.x,p.y-1)).xyz+input.read(uint2(p.x,p.y+1)).xyz)*0.5f;
        x+=(warp-input.read(id).xyz)*mix(0.42f,0.10f,smoothstep(0.0f,7.0f,float(min(p.x%CLOTH_PANEL_COLUMNS,CLOTH_PANEL_COLUMNS-1-p.x%CLOTH_PANEL_COLUMNS))));
    }
    float reaction=u.animation.z;
    if(reaction>0.01f) {
        float2 delta=x.xy-u.pointer.zw*extent;
        float radius=95.0f,distance=length(delta);
        if(distance<radius) {
            // A hand behind the fabric presses a smooth dent into the cloth.
            // The profile has no steep rim, so a passing hand cannot snap the cloth.
            float dome=1.0f-distance*distance/(radius*radius);
            float dent=52.0f*dome*dome*reaction;
            // A hand presses at a limited speed. Folds standing proud of the dent would otherwise
            // all be shoved flat at once, however lightly it touched, and snap into a ripple.
            x.z+=clamp((-dent-x.z)*0.30f,-0.09f,0.0f);
        }
    }
    // Held: a hand gathers a soft patch of fabric and carries it together, so nothing in the
    // patch is crushed and there is no stored squeeze waiting to spring out on release. life.xy
    // is the thread and row held, life.z the grip, life.w how settled the panel is since being
    // let go, stroke.xyz the hand. Other panels aren't touched.
    // Limits always pull back gently. A hard stop anywhere would yank whatever fabric is outside
    // it at that moment, which is a ripple from nowhere, seconds after the hand has gone.
    float drawnOut=0.0f;
    const float limitEase=0.03f;
    if(max(u.life.z,u.life.w)>0.001f&&p.x/CLOTH_PANEL_COLUMNS==int(u.life.x)/CLOTH_PANEL_COLUMNS) {
        float2 fabric=float2(p-int2(u.life.xy))*float2(r.restU,r.restV);
        float reachSquared=dot(fabric,fabric);
        float3 carry=u.stroke.xyz-input.read(uint2(u.life.xy)).xyz;
        x+=carry*exp(-reachSquared/(2.0f*52.0f*52.0f))*0.06f*u.life.z;
        // The wider limits outlast the grip, so settling fabric is never caught by one.
        drawnOut=exp(-reachSquared/(2.0f*130.0f*130.0f))*max(u.life.z,u.life.w);
    }
    // Long-range attachment: no point can sit further from its pin than the fabric reaches.
    float3 pin=clothRail(p.x,extent,u);
    float reach=float(p.y)*r.restV*1.01f;
    float3 fromPin=x-pin;
    float distance=length(fromPin);
    if(distance>reach)x=pin+fromPin*reach/distance;
    float sway=CLOTH_SWAY+float(p.y)*r.restV*CLOTH_SWAY_DROP+drawnOut*90.0f;
    x.x=mix(x.x,clamp(x.x,pin.x-sway,pin.x+sway),limitEase);
    x.z=mix(x.z,clamp(x.z,pin.z-sway,pin.z+sway),limitEase);
    // Fabric bunches a little and never rises by itself; a hand can lift it part way.
    x.y=mix(x.y,max(x.y,pin.y+float(p.y)*r.restV*mix(CLOTH_LEAST_DROP,0.42f,drawnOut)),limitEase);
    output.write(float4(x,1),id);
}
kernel void cloth_commit(texture2d<float> previous [[texture(0)]],texture2d<float> corrected [[texture(1)]],
 texture2d<float,access::write> velocity [[texture(2)]],constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    float inverse=1.0f/max(u.impulse.w,0.0001f);
    float3 x=corrected.read(id).xyz,v=(x-previous.read(id).xyz)*inverse;
    // Internal friction between neighbouring threads: bends slowly die away and
    // grid-scale zigzags, which no real fabric can hold, are removed at once.
    int2 p=int2(id);
    float3 mean=0;float count=0;
    const int2 around[4]={int2(1,0),int2(-1,0),int2(0,1),int2(0,-1)};
    for(int i=0;i<4;i++) {
        int2 n=p+around[i];
        if(!sameCloth(p,n))continue;
        mean+=(corrected.read(uint2(n)).xyz-previous.read(uint2(n)).xyz)*inverse;count+=1;
    }
    float settling=p.x/CLOTH_PANEL_COLUMNS==int(u.life.x)/CLOTH_PANEL_COLUMNS?u.life.w:0.0f;
    if(count>0)v=mix(v,mean/count,mix(0.42f,0.85f,settling));
    // Wind alone never moves fabric faster than the calm limit. Anything quicker is a fold
    // snapping through, which whips along the hem as a ripple, so it's held back. The full
    // speed is only for fabric being carried: a held panel, or right around a moving hand.
    // A cursor that's merely resting frees nothing.
    float2 fromHand=x.xy-u.pointer.zw*extentOf(u);
    float sweeping=saturate(length(u.impulse.xy)/300.0f)*saturate(u.animation.z)*exp(-dot(fromHand,fromHand)/(2.0f*150.0f*150.0f));
    if(u.life.z>0.001f&&p.x/CLOTH_PANEL_COLUMNS==int(u.life.x)/CLOTH_PANEL_COLUMNS) {
        // Only fabric near the hand follows it quickly; further off, nothing gets a shock.
        ClothRig rig=clothRig(extentOf(u),u.animation.x);
        float2 fabric=float2(p-int2(u.life.xy))*float2(rig.restU,rig.restV);
        sweeping=max(sweeping,exp(-dot(fabric,fabric)/(2.0f*90.0f*90.0f))*u.life.z);
    }
    float speed=length(v),top=mix(mix(CLOTH_CALM_SPEED,55.0f,settling),CLOTH_TOP_SPEED,sweeping);
    if(speed>top)v*=top/speed;
    velocity.write(float4(id.y==0?float3(0):v,0),id);
}
struct ClothVertex { float4 position [[position]]; float3 normal; float3 world; float2 rest; float speed; float edge; float2 shadow; float shadowDepth; };
int2 meshCorner(uint vertex_id,int columns,int rows) {
    uint cell=vertex_id/6u,corner=vertex_id%6u;
    uint width=uint(columns-1);
    int2 base=int2(int(cell%width),int(cell/width));
    // Alternating diagonals: a fold's silhouette never turns into a row-spaced sawtooth.
    const int2 regular[6]={int2(0,0),int2(1,0),int2(0,1),int2(0,1),int2(1,0),int2(1,1)};
    const int2 flipped[6]={int2(0,0),int2(1,0),int2(1,1),int2(0,0),int2(1,1),int2(0,1)};
    return base+(((base.x+base.y)&1)==0?regular[corner]:flipped[corner]);
}
constant float3 clothSun=float3(-0.7071f,-0.2500f,-0.6614f);
// Window Curtains has a higher sun, on the other side: its beams fall down and to the left.
// A point at depth z lies on the sun ray that crosses z=0 at x.xy+z*windowRayRise.
constant float2 windowRayRise=float2(0.8065f,-0.9678f);
float2 windowRayPlane(float3 x) { return x.xy+x.z*windowRayRise; }
float2 windowShadowCoord(float2 plane,float2 extent) { return (plane+extent*0.5f)/(extent*2.0f); }
float2 clothShadowCoord(float3 x,float2 extent,float style) {
    if(style>11.5f&&style<12.5f)return windowShadowCoord(windowRayPlane(x),extent);
    // Project along the sun onto the plane z=0, then fit a window with margins.
    float2 projected=x.xy-x.z*clothSun.xy/clothSun.z;
    return (projected+extent*0.25f)/(extent*1.5f);
}
float clothShadowDepth(float3 x) { return saturate((x.z+160.0f)/320.0f); }
bool clothSeam(int2 cell) {
    return (cell.x%CLOTH_PANEL_COLUMNS)==CLOTH_PANEL_COLUMNS-1;
}
// One B-spline pass over the mesh when drawing: the solver may leave a tight fold with a
// one-cell zigzag, which no woven cloth has and which reads as a comb along the crease.
float3 clothDrawn(texture2d<float> position,int2 p) {
    if(p.y==0)return position.read(uint2(p)).xyz;
    const float weight[3]={0.25f,0.5f,0.25f};
    float3 sum=0;float total=0;
    for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++) {
        int2 n=p+int2(x,y);
        if(!sameCloth(p,n))continue;
        sum+=position.read(uint2(n)).xyz*weight[x+1]*weight[y+1];total+=weight[x+1]*weight[y+1];
    }
    return sum/total;
}
// Normals are smoothed both ways: a sharp change between two columns, as at a sewn hem,
// would otherwise be interpolated into a row of teeth by the triangles of each cell.
float3 clothDrawnNormal(texture2d<float> position,int2 p) {
    float3 n=clothNormal(position,p)*0.34f;
    const int2 around[4]={int2(1,0),int2(-1,0),int2(0,1),int2(0,-1)};
    for(int i=0;i<4;i++)if(sameCloth(p,p+around[i]))n+=clothNormal(position,p+around[i])*0.165f;
    float m=length(n);
    return m>0.0001f?n/m:float3(0,0,1);
}
vertex ClothVertex cloth_vertex(uint vid [[vertex_id]],texture2d<float> position [[texture(0)]],texture2d<float> velocity [[texture(1)]],constant Params &u [[buffer(0)]]) {
    ClothVertex out;
    int2 p=meshCorner(vid,CLOTH_COLUMNS,CLOTH_ROWS);
    uint cell=vid/6u;int2 base=int2(int(cell%uint(CLOTH_COLUMNS-1)),int(cell/uint(CLOTH_COLUMNS-1)));
    float2 extent=extentOf(u);
    if(clothSeam(base)) {out.position=offscreenVertex;return out;}
    float3 x=clothDrawn(position,p);
    out.world=x;
    ClothRig rig=clothRig(extent,u.animation.x);
    out.rest=float2(float(p.x)*rig.restU,float(p.y)*rig.restV);
    out.normal=clothDrawnNormal(position,p);
    out.speed=length(velocity.read(uint2(p)).xyz);
    int local=p.x%CLOTH_PANEL_COLUMNS;
    out.edge=float(min(local,CLOTH_PANEL_COLUMNS-1-local));
    out.shadow=clothShadowCoord(x,extent,u.animation.x);
    out.shadowDepth=clothShadowDepth(x);
    out.position=toClip(x.xy+float2(x.z*0.10f,0),extent);
    return out;
}
vertex float4 cloth_shadow_vertex(uint vid [[vertex_id]],texture2d<float> position [[texture(0)]],constant Params &u [[buffer(0)]]) {
    uint cell=vid/6u;int2 base=int2(int(cell%uint(CLOTH_COLUMNS-1)),int(cell/uint(CLOTH_COLUMNS-1)));
    if(clothSeam(base))return offscreenVertex;
    float3 x=clothDrawn(position,meshCorner(vid,CLOTH_COLUMNS,CLOTH_ROWS));
    float2 uv=clothShadowCoord(x,extentOf(u),u.animation.x);
    return float4(uv.x*2.0f-1.0f,1.0f-uv.y*2.0f,clothShadowDepth(x),1);
}
// Percentage-closer soft shadows: the penumbra widens with the gap between
// occluder and receiver, and with how fast this piece of fabric is moving. Still
// cloth therefore throws razor-sharp sun lines; a gust smears them apart.
float clothSunlight(depth2d<float> map,float2 uv,float receiver,float speed,float2 pixel) {
    constexpr sampler nearest(coord::normalized,address::clamp_to_edge,filter::nearest);
    float angle=interleavedGradient(pixel)*6.2831853f;
    float2x2 spin(float2(cos(angle),sin(angle)),float2(-sin(angle),cos(angle)));
    const float2 disk[12]={float2(-0.326f,-0.406f),float2(-0.840f,-0.074f),float2(-0.696f,0.457f),float2(-0.203f,0.621f),float2(0.962f,-0.195f),float2(0.473f,-0.480f),float2(0.519f,0.767f),float2(0.185f,-0.893f),float2(0.507f,0.064f),float2(0.896f,0.412f),float2(-0.322f,-0.933f),float2(-0.792f,-0.598f)};
    float bias=0.012f,search=0.012f,blockers=0,count=0;
    for(int i=0;i<12;i++) {
        float d=map.sample(nearest,uv+spin*disk[i]*search);
        if(d<receiver-bias){blockers+=d;count+=1;}
    }
    if(count<0.5f)return 1.0f;
    float gap=receiver-blockers/count;
    float penumbra=clamp(gap*0.030f+smoothstep(30.0f,420.0f,speed)*0.016f,0.0006f,0.030f);
    float lit=0;
    for(int i=0;i<12;i++) {
        float d=map.sample(nearest,uv+spin*disk[i]*penumbra);
        lit+=d<receiver-bias?0.0f:1.0f;
    }
    return lit/12.0f;
}
// The sun reaches the fabric through a real window behind it: bright panes split by
// mullions, thrown onto the cloth at the sun's angle. Depth shifts where the light
// lands, so every pane edge bends across every fold. Edges are razor sharp where
// cloth hangs still and close, and spread with distance and with fabric speed.
float windowLight(float3 x,float2 extent,float softness) {
    float2 q=x.xy-x.z*clothSun.xy/clothSun.z;
    float slanted=q.x+q.y*0.60f;
    // Two sashes, three panes high, sized from the room so any window shape works.
    float paneWidth=extent.x*0.205f,paneHeight=extent.y*0.300f,mullion=max(extent.x*0.011f,9.0f);
    float2 origin=float2(-extent.x*0.015f,-extent.y*0.13f);
    float2 cell=(float2(slanted,q.y)-origin)/float2(paneWidth,paneHeight);
    float2 index=floor(cell);
    if(index.x<0||index.x>2||index.y<0||index.y>2)return 0.0f;
    float2 inside=(fract(cell)-0.5f)*float2(paneWidth,paneHeight);
    float2 half_=float2(paneWidth,paneHeight)*0.5f-mullion*0.5f;
    float2 edge=half_-abs(inside);
    float lit=smoothstep(-softness,softness,edge.x)*smoothstep(-softness,softness,edge.y);
    // A little of the sky's unevenness from pane to pane.
    return lit*(0.82f+0.18f*sin(index.x*2.1f+index.y*3.7f+0.5f));
}
fragment float4 cloth_backdrop(ClothVertex in [[stage_in]],depth2d<float> shadow [[texture(0)]],constant Params &u [[buffer(0)]]) {
    // What shows through gaps between panels: the window itself, further back.
    float2 extent=extentOf(u);
    float3 wall=float3(in.position.xy/(u.geometry.xy*u.blur.z)*extent,-120.0f);
    return float4(0.11f+0.89f*windowLight(wall,extent,1.4f),0.5f,0,0);
}
vertex ClothVertex cloth_backdrop_vertex(uint vid [[vertex_id]]) {
    ClothVertex out;
    float2 p=float2((vid<<1)&2,vid&2);
    out.position=float4(p*2.0f-1.0f,0.5f,1);
    return out;
}
fragment float4 cloth_fragment(ClothVertex in [[stage_in]],depth2d<float> shadow [[texture(0)]],texture2d<float> detail [[texture(1)]],constant Params &u [[buffer(0)]]) {
    // High-definition surface, fixed to the fabric so it moves with it and never swims:
    // long hanging creases and slubby threads, mipmapped so it can never shimmer.
    constexpr sampler detailSampler(coord::normalized,address::repeat,filter::linear,mip_filter::linear,max_anisotropy(8));
    float4 surface=detail.sample(detailSampler,in.rest/1024.0f);
    float2 r=in.rest;
    float3 n=normalize(normalize(in.normal)+float3(surface.xy,0));
    float threads=0.5f+0.5f*sin(r.x*3.4f)*sin(r.y*3.4f);
    float pixelSize=max(fwidth(r.x),fwidth(r.y));
    threads=mix(0.5f,threads,1.0f-smoothstep(0.5f,1.5f,pixelSize)); // Fade before it can moire.
    float motion=smoothstep(40.0f,520.0f,in.speed);
    // Optical depth through sheer fabric grows at grazing angles (Beer-Lambert), which
    // is why backlit folds show dark lines. Moving fabric spreads those lines out.
    float facing=max(abs(n.z),mix(0.30f,0.54f,motion));
    float density=0.70f*(1.0f+surface.z*0.30f+(threads-0.5f)*0.14f);
    float transmitted=exp(-density/facing);
    float2 extent=extentOf(u);
    float soft=0.7f+abs(in.world.z)*0.030f+motion*15.0f;
    float pane=windowLight(in.world,extent,soft);
    // Fabric nearer the sun also shades this layer, with its own soft shadow edge.
    float sun=clothSunlight(shadow,in.shadow,in.shadowDepth,in.speed,in.position.xy);
    float landing=abs(dot(n,clothSun));
    // Daylight fills the room softly; direct sun is much harder and much brighter.
    float room=0.10f+0.26f*landing;
    float direct=pane*mix(0.40f,1.0f,sun)*(0.14f+1.0f*landing*landing*landing);
    float glow=(room+direct)*(1.0f-transmitted)*1.12f;
    // The free side edge thins out over a few threads. It curls into a roll narrower than
    // the mesh can show cleanly, and a hard start there would draw a comb of triangles.
    float coverage=smoothstep(1.0f,7.0f+motion*2.4f,in.edge);
    return float4(glow*coverage,coverage*0.5f,0,mix(1.0f,transmitted*mix(0.6f,1.0f,sun),coverage));
}

// ---------------------------------------------------------------------------
// Window Curtains: the camera looks at a bright window through hanging sheer fabric.
// Where no fabric hangs you see the window itself, razor sharp. Through fabric, light is
// refracted by the folds and diffused by the weave: a screen-space subsurface transmission
// that samples the window at an offset set by the surface normal, more blurred the more
// fabric the light must cross. Hard sunbeams then cross the room: every pixel marches a
// real volume, asking at each depth whether the sun gets through the window bars, the
// branches outside and the cloth itself. Straight sun rays make the beam edges knife sharp.
// ---------------------------------------------------------------------------
float2 windowSunPosition(float2 extent) { return float2(extent.x*0.70f,extent.y*0.17f); }
// Distance inside the window's glass: positive in a pane, negative on bars or wall.
float windowPanes(float2 p,float2 extent,thread float &box,thread float2 &q) {
    float2 low=float2(extent.x*0.075f,extent.y*0.045f),high=float2(extent.x*0.925f,extent.y*0.86f);
    float2 size=high-low;
    q=(p-low)/size;
    float2 outer=abs(p-(low+high)*0.5f)-size*0.5f;
    box=length(max(outer,0.0f))+min(max(outer.x,outer.y),0.0f);
    float mullionX=extent.x*0.0052f,mullionY=extent.x*0.0044f;
    float2 inPane=abs(fract(float2(q.x*3.0f,q.y*2.0f))-0.5f)*size/float2(3.0f,2.0f);
    float2 toBar=size/float2(3.0f,2.0f)*0.5f-inPane;
    return min(min(toBar.x-mullionX,toBar.y-mullionY),-box-extent.x*0.009f);
}
// Light spilling onto the wall and into the window reveal.
float windowWall(float box,float2 p,float2 extent) {
    float toSun=length((p-windowSunPosition(extent))/extent.y);
    return 0.022f+0.070f*exp(-box*box/(2.0f*70.0f*70.0f))*(0.4f+0.6f*exp(-toSun*toSun*2.0f));
}
// Bars stay dark but catch a thin lit bevel on the side facing the sun.
float windowOverGlass(float bar,float glass) {
    return mix(0.035f+0.13f*exp(-abs(bar+1.5f)*0.55f),glass,smoothstep(-0.7f,0.7f,bar));
}
float windowScene(float2 p,float2 extent,float t) {
    float box;float2 q;
    float bar=windowPanes(p,extent,box,q);
    if(box>0.0f)return windowWall(box,p,extent);
    float2 sun=windowSunPosition(extent);
    float toSun=length((p-sun)/extent.y);
    // Sky: a bright gradient, soft moving cloud, and an overexposed sun with a wide bloom.
    float cloud=fractalNoise(float3(p/460.0f+float2(t*0.006f,0),t*0.012f),4,71u);
    float sky=0.13f+0.07f*(1.0f-q.y)+cloud*0.22f;
    float disc=exp(-toSun*toSun*150.0f)*3.4f+exp(-toSun*toSun*16.0f)*0.72f+exp(-toSun*toSun*3.0f)*0.20f;
    // A tree outside: soft leafy masses rising from below, swaying very slightly. The fabric
    // diffuses them into the uneven glow that makes the panes read as a real view.
    float2 leaf=p+float2(sin(t*0.13f)*3.0f,0);
    float tree=smoothstep(-0.05f,0.13f,fractalNoise(float3(leaf/150.0f,t*0.008f),4,251u)+(q.y-0.62f)*0.62f+(0.5f-abs(q.x-0.5f))*-0.20f);
    float glass=(sky+disc)*mix(1.0f,0.30f,tree);
    return windowOverGlass(bar,glass);
}
kernel void window_paint(texture2d<float,access::write> output [[texture(0)]],constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    float2 extent=extentOf(u);
    float2 p=(float2(id)+0.5f)/float2(output.get_width(),output.get_height())*extent;
    output.write(float4(windowScene(p,extent,u.clock.x)),id);
}
// Branches outside the window, as seen from the sun: a 1D strip across the beam direction,
// because anything along a sun ray lands on the same line. Hard thresholds give the beams
// their distinct widths and clean edges, and the slow sway keeps them alive.
// Exactly perpendicular to the beam as seen on screen, parallax included, which is
// what keeps a beam's edge knife sharp however deep the air it crosses.
constant float2 windowBeamAcross=float2(0.8077f,0.5896f);
constant float WINDOW_STRIP_RANGE=3200.0f;
kernel void window_branches(texture2d<float,access::write> output [[texture(0)]],constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    float c=(float(id.x)+0.5f)/float(output.get_width())*WINDOW_STRIP_RANGE,t=u.clock.x;
    float wide=gradientNoise(float3(c/96.0f,t*0.05f,2.0f),211u);
    float fine=gradientNoise(float3(c/31.0f,t*0.08f,5.0f),223u);
    // Some edges are razor sharp (near branches), some spread a little (distant ones).
    float softness=mix(0.004f,0.040f,smoothstep(0.0f,0.45f,gradientNoise(float3(c/70.0f,1.0f,t*0.02f),229u)));
    float gate=smoothstep(-softness,softness,wide+fine*0.45f+0.04f);
    float cut=smoothstep(-0.010f,0.010f,gradientNoise(float3(c/14.0f,t*0.03f,9.0f),227u)+0.30f);
    // Inside a beam the light is never flat: thin leaf shadows run the length of it.
    float streaks=0.66f+0.62f*gradientNoise(float3(c/19.0f,t*0.04f,4.0f),239u)+0.40f*gradientNoise(float3(c/6.5f,t*0.05f,6.0f),241u);
    output.write(float4(gate*mix(0.35f,1.0f,cut)*saturate(streaks)),id);
}
// Whether the sun reaches this ray: through the window opening, past the branches.
// plane is where the ray crosses z=0; the window itself sits further back along it. The
// branches cut the hard edges. The opening only limits how far the light reaches, so it
// is soft: a sharp outline would draw a squiggle across every pleat of the fabric.
// blur widens the branch shadows: 0 is direct sun, higher is light that has been scattered.
float windowSunGate(float2 plane,float2 extent,texture2d<float> strip,float blur) {
    constexpr sampler s(coord::normalized,address::repeat,filter::linear,mip_filter::linear);
    float box;float2 q;
    windowPanes(plane+windowRayRise*140.0f,extent,box,q);
    float opening=smoothstep(10.0f,-70.0f,box);
    if(opening<=0.0f)return 0.0f;
    float across=dot(plane,windowBeamAcross);
    float branch=strip.sample(s,float2(across/WINDOW_STRIP_RANGE,0.5f),level(blur)).x;
    // Further boughs at another scale split the broad shafts into unequal beams.
    float bough=strip.sample(s,float2((across*0.63f+900.0f)/WINDOW_STRIP_RANGE,0.5f),level(blur)).x;
    return opening*branch*mix(0.50f,1.0f,bough);
}
fragment float4 window_backdrop(ClothVertex in [[stage_in]],texture2d<float> window [[texture(0)]],constant Params &u [[buffer(0)]]) {
    constexpr sampler baked(coord::normalized,address::clamp_to_edge,filter::linear,mip_filter::nearest);
    float2 extent=extentOf(u),uv=in.position.xy/(u.geometry.xy*u.blur.z),p=uv*extent;
    float box;float2 q;
    float bar=windowPanes(p,extent,box,q);
    float light=box>0.0f?windowWall(box,p,extent):windowOverGlass(bar,window.sample(baked,uv,level(0.0f)).x);
    return float4(light,0.5f,0,0);
}
fragment float4 window_cloth_fragment(ClothVertex in [[stage_in]],texture2d<float> window [[texture(0)]],depth2d<float> shadow [[texture(1)]],texture2d<float> detail [[texture(2)]],texture2d<float> strip [[texture(3)]],constant Params &u [[buffer(0)]]) {
    constexpr sampler detailSampler(coord::normalized,address::repeat,filter::linear,mip_filter::linear,max_anisotropy(8));
    constexpr sampler sceneSampler2(coord::normalized,address::clamp_to_edge,filter::linear,mip_filter::linear);
    float4 surface=detail.sample(detailSampler,in.rest/1024.0f);
    float3 n=normalize(normalize(in.normal)+float3(surface.xy,0));
    float motion=smoothstep(40.0f,520.0f,in.speed);
    float2 extent=extentOf(u),uv=in.position.xy/(u.geometry.xy*u.blur.z);
    // How much fabric the light crosses: more through folds seen at grazing angles.
    float facing=max(abs(n.z),mix(0.22f,0.42f,motion));
    float crossing=0.74f*(1.0f+surface.z*0.32f)/facing;
    // Refraction: each fold is a lens that bends the window behind it.
    float2 bend=n.xy*(16.0f+crossing*26.0f)/extent;
    // Diffusion: the longer the path through the weave, the wider the light has spread.
    float spread=clamp(2.6f+crossing*1.5f+motion*1.2f,2.0f,7.2f);
    float through=window.sample(sceneSampler2,uv+bend,level(spread)).x*0.72f
        +window.sample(sceneSampler2,uv+bend*2.1f,level(spread+1.3f)).x*0.28f;
    // Forward scattering towards the sun brightens folds that lean into the light.
    float2 toSun=normalize(windowSunPosition(extent)-in.world.xy);
    float lean=dot(n.xy,toSun);
    float forward=0.74f+0.42f*smoothstep(-0.7f,0.8f,lean);
    float sun=clothSunlight(shadow,in.shadow,in.shadowDepth,in.speed,in.position.xy);
    float direct=exp(-crossing*1.15f); // The small part that passes straight between threads.
    float glow=through*(1.0f-direct)*forward*mix(0.62f,1.0f,sun)*0.78f;
    // Direct sun on the fabric never stays a beam. The weave scatters it, so what you see is
    // a broad soft glow through the cloth, its edges gone, brighter where the folds thin it.
    float diffused=windowSunGate(windowRayPlane(in.world),extent,strip,6.5f)*mix(0.40f,1.0f,sun);
    float wrap=0.35f+0.65f*saturate(dot(n,float3(0.5014f,-0.6017f,-0.6217f))*-1.0f+0.25f);
    glow+=diffused*wrap*(1.0f-direct)*0.40f;
    float coverage=smoothstep(1.0f,7.0f+motion*2.4f,in.edge);
    return float4(glow*coverage,coverage*0.5f,0,mix(1.0f,direct,coverage));
}
// Volumetric sunbeams. Each pixel looks through a room of hazy air; at every depth along
// that view the air scatters light only if the sun actually reaches it. The cloth divides
// the light: air the sun reaches through a gap gets the knife-sharp beam, whereas air behind
// fabric gets only a faint, wide glow of what the weave scattered. Only air in front of the
// curtains counts: anything behind them is already seen through the fabric itself.
kernel void window_shafts(depth2d<float> shadow [[texture(0)]],texture2d<float> strip [[texture(1)]],texture2d<float> history [[texture(2)]],texture2d<float,access::write> output [[texture(3)]],
 constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    constexpr sampler compare(coord::normalized,address::clamp_to_edge,filter::linear,compare_func::less);
    float2 size=float2(output.get_width(),output.get_height());
    float2 extent=extentOf(u),p=(float2(id)+0.5f)/size*extent;
    const int steps=36;
    const float nearest=40.0f,furthest=500.0f;
    float jitter=fract(interleavedGradient(float2(id))+u.life.y*0.61803398f);
    float light=0;
    for(int i=0;i<steps;i++) {
        float z=mix(nearest,furthest,(float(i)+jitter)/float(steps));
        float2 plane=windowRayPlane(float3(p-float2(z*0.10f,0),z));
        float sharp=windowSunGate(plane,extent,strip,0.0f);
        float scattered=windowSunGate(plane,extent,strip,6.5f);
        if(sharp<=0.0f&&scattered<=0.0f)continue;
        float open=shadow.sample_compare(compare,windowShadowCoord(plane,extent),clothShadowDepth(float3(0,0,z))-0.010f);
        // Beer-Lambert: the air thins the beam as it travels away from the window.
        light+=mix(scattered*0.42f,sharp,open)*exp(-(z-nearest)/330.0f);
    }
    light*=2.0f/float(steps);
    output.write(float4(u.impulse.z>0.5f?light:mix(history.read(id).x,light,0.22f)),id);
}
// Dust hanging in the air: two layers of slowly drifting specks, visible only inside a beam.
float beamDust(float2 p,float t) {
    float sum=0;
    for(int layer=0;layer<2;layer++) {
        float scale=layer==0?44.0f:71.0f;
        float2 q=(p+float2(sin(t*0.07f+float(layer))*24.0f-t*(3.0f+float(layer)*2.0f),t*(5.0f-float(layer)*2.0f)))/scale;
        float2 cell=floor(q),local=fract(q);
        uint2 c=uint2(cell+8192.0f);
        uint seed=233u+uint(layer)*11u;
        if(randomCell(c,seed)<0.62f)continue;
        float2 centre=float2(randomCell(c,seed+1u),randomCell(c,seed+2u))*0.8f+0.1f;
        float distance=length(local-centre)*scale,radius=0.7f+randomCell(c,seed+3u)*1.1f;
        float twinkle=0.55f+0.45f*sin(t*(0.5f+randomCell(c,seed+4u))+randomCell(c,seed+5u)*6.28f);
        sum+=exp(-distance*distance/(radius*radius))*twinkle;
    }
    return sum;
}

// ---------------------------------------------------------------------------
// Seafloor: true pool caustics, seen at a low angle. A sum of real gravity waves makes the
// surface: the same everywhere, so no part of the floor is ever left empty. A fine mesh of
// light rays is refracted through it, and brightness is the ratio of each patch of light's
// area before and after refraction. The floor lies below the waves' focus, so the light
// sheet has already folded: thin bright lines with wide pale bands held between them. The
// floor recedes up the window, so its pattern grows fine towards the top. It reaches well
// past the window on every side, so light arrives at the edges from outside. A click drops
// a stone: only that makes ripples, solved by a wave equation added to the surface.
// ---------------------------------------------------------------------------
constant float SEA_MARGIN=150.0f,SEA_TILT=0.46f;
constant int SEA_WAVES=26;
// A flat floor seen through a tilted camera: a true homography, in both directions.
float2 seaFloorAt(float2 screen,float2 extent) {
    float up=extent.y-screen.y,g=1.0f-SEA_TILT*up/extent.y;
    return float2(extent.x*0.5f+(screen.x-extent.x*0.5f)/g,extent.y-up/g);
}
float2 seaScreenAt(float2 floor_,float2 extent) {
    float back=extent.y-floor_.y,g=1.0f/(1.0f+SEA_TILT*back/extent.y);
    return float2(extent.x*0.5f+(floor_.x-extent.x*0.5f)*g,extent.y-back*g);
}
// The piece of floor that can ever be seen, plus the margin. Keep in step with fluid.m.
float2 seaSize(float2 extent) { return extent/(1.0f-SEA_TILT)+2.0f*SEA_MARGIN; }
float2 seaOrigin(float2 extent) { float2 size=seaSize(extent);return float2(extent.x*0.5f-size.x*0.5f,extent.y+SEA_MARGIN-size.y); }
kernel void sea_water(texture2d<float,access::read> input [[texture(0)]],texture2d<float,access::write> output [[texture(1)]],
 constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    if(u.impulse.z>0.5f) {output.write(float4(0),id);return;}
    int2 size=int2(output.get_width(),output.get_height()),p=int2(id);
    float2 state=input.read(id).xy;
    float2 left=input.read(uint2(max(p-int2(1,0),int2(0)))).xy,right=input.read(uint2(min(p+int2(1,0),size-1))).xy;
    float2 up=input.read(uint2(max(p-int2(0,1),int2(0)))).xy,down=input.read(uint2(min(p+int2(0,1),size-1))).xy;
    float2 extent=extentOf(u),domain=seaSize(extent),spacing=domain/float2(size),inverse=1.0f/(spacing*spacing);
    float laplacian=(left.x+right.x-2.0f*state.x)*inverse.x+(up.x+down.x-2.0f*state.x)*inverse.y;
    float viscous=(left.y+right.y-2.0f*state.y)*inverse.x+(up.y+down.y-2.0f*state.y)*inverse.y;
    float dt=u.impulse.w;
    float velocity=(state.y+150.0f*150.0f*laplacian*dt+7.0f*viscous*dt)*exp(-0.55f*dt);
    float height=state.x+velocity*dt;
    // A click is a dropped stone: stroke.xy is where, on the floor, and stroke.z how far
    // the water there moves in this step. Its brief bobbing is what sends out the rings.
    if(u.stroke.z!=0.0f) {
        float2 delta=(float2(id)+0.5f)/float2(size)*domain+seaOrigin(extent)-u.stroke.xy;
        height-=u.stroke.z*exp(-dot(delta,delta)/(2.0f*17.0f*17.0f));
    }
    // Ripples leave through the margin, unseen, rather than bouncing back.
    float border=min(min(float(p.x),float(p.y)),min(float(size.x-1-p.x),float(size.y-1-p.y)));
    float absorb=exp(-dt*14.0f*(1.0f-smoothstep(0.0f,26.0f,border)));
    output.write(float4(height*absorb,velocity*absorb,0,0),id);
}
kernel void sea_surface(texture2d<float,access::read> ripples [[texture(0)]],texture2d<float,access::write> output [[texture(1)]],
 constant Params &u [[buffer(0)]],constant float4 *waves [[buffer(1)]],uint2 id [[thread_position_in_grid]]) {
    float2 extent=extentOf(u);
    float2 p=(float2(id)+0.5f)/float2(output.get_width(),output.get_height())*seaSize(extent)+seaOrigin(extent);
    float t=u.clock.x,h=0;
    for(int i=0;i<SEA_WAVES;i++) {
        float4 w=waves[i]; // Wave vector, angular frequency, amplitude.
        h+=w.w*sin(dot(w.xy,p)-w.z*t+float(i)*2.399963f);
    }
    output.write(float4(h+ripples.read(id).x),id);
}
constant int CAUSTIC_COLUMNS=720,CAUSTIC_ROWS=500;
struct CausticVertex { float4 position [[position]]; float2 before; float2 after; };
vertex CausticVertex caustic_vertex(uint vid [[vertex_id]],texture2d<float> water [[texture(0)]],constant Params &u [[buffer(0)]]) {
    CausticVertex out;
    int2 p=meshCorner(vid,CAUSTIC_COLUMNS,CAUSTIC_ROWS);
    float2 extent=extentOf(u),domain=seaSize(extent);
    float2 uv=float2(p)/float2(CAUSTIC_COLUMNS-1,CAUSTIC_ROWS-1);
    Surface s=splineSurface(water,uv,domain/float2(water.get_width(),water.get_height()));
    float3 normal=normalize(float3(-s.gradient,1.0f));
    // Sunlight enters the water at a slant and refracts at the surface (n = 1.33).
    float3 ray=refract(normalize(float3(0.10f,0.13f,-0.986f)),normal,1.0f/1.33f);
    const float depth=450.0f;
    float2 start=uv*domain+seaOrigin(extent);
    float2 landing=start+ray.xy/max(-ray.z,0.2f)*depth;
    out.before=start;
    out.after=landing;
    float2 seen=seaScreenAt(landing,extent);
    out.position=float4(seen.x/extent.x*2.0f-1.0f,1.0f-seen.y/extent.y*2.0f,0.5f,1);
    return out;
}
fragment float4 caustic_fragment(CausticVertex in [[stage_in]]) {
    float2 ax=dfdx(in.before),ay=dfdy(in.before),bx=dfdx(in.after),by=dfdy(in.after);
    float before=abs(ax.x*ay.y-ax.y*ay.x),after=abs(bx.x*by.y-bx.y*by.x);
    float intensity=min(before/max(after,0.00001f),12.0f);
    return float4(intensity,intensity,0,0);
}
float seafloorLight(float2 uv,float2 extent,texture2d<float> water,texture2d<float> caustics) {
    constexpr sampler s(coord::normalized,address::clamp_to_edge,filter::linear,mip_filter::linear);
    float2 domain=seaSize(extent);
    float2 onFloor=(seaFloorAt(uv*extent,extent)-seaOrigin(extent))/domain;
    Surface surface=splineSurface(water,onFloor,domain/float2(water.get_width(),water.get_height()));
    // Seen through the same moving surface, so the whole pattern wobbles with the water.
    float2 seen=uv+surface.gradient*7.0f/extent;
    // The sun is a disc, not a point, so even perfect focus has a little width.
    float focused=caustics.sample(s,seen,level(0.0f)).x*0.55f+caustics.sample(s,seen,level(1.0f)).x*0.45f;
    float scattered=caustics.sample(s,seen,level(3.5f)).x;
    // Folded sheets of light are only a little brighter than the water, and the lines
    // where they crease a little brighter again, so this stays quiet behind everything.
    float sheet=smoothstep(0.85f,1.9f,focused);
    float burn=max(focused-1.7f,0.0f);
    burn=burn/(1.0f+burn*0.14f);
    // Light scattered through the water gives it body, so it never reads as lines on black.
    float haze=caustics.sample(s,seen,level(5.5f)).x;
    return 0.030f+haze*0.030f+scattered*0.040f+sheet*0.055f+burn*0.056f;
}

// ---------------------------------------------------------------------------
// Rain: thin motion-blurred streaks. Near drops come from weather.h, whose
// trajectories drive the runoff simulation; far drops are GPU-only depth.
// Every near impact throws ballistic beads and a crown along the bar edge.
// ---------------------------------------------------------------------------
constant uint RAIN_LANES=64,RAIN_FAR=340,RAIN_BEADS=3;
// Where a far drop on this slanted line stops: the same walk weather.h takes for near drops.
// The cursor is an umbrella, a dome of radius 64 that keeps the rain off whatever is under it.
float rainStop(float top,float slope,float bottom,constant Params &u) {
    float zoom=max(u.geometry.z,0.1f),scale=1.0f/sqrt(1.0f+slope*slope),y=-140.0f;
    float4 composer=u.composer/zoom,footer=u.footer/zoom,side=u.footerSide/zoom;
    float radius=64.0f*u.animation.z;
    float2 center=u.pointer.zw*extentOf(u);
    for(int i=0;i<24&&y<bottom;i++) {
        float2 p=float2(top+slope*y,y);
        float d=min(barDistance(p,composer,u.blur.x/zoom),min(barDistance(p,footer,u.blur.y/zoom),barDistance(p,side,u.blur.y/zoom)));
        if(radius>=1.0f)d=min(d,length(p-center)-radius);
        if(d<0.5f)return y;
        y+=d*scale;
    }
    return bottom;
}
struct RainVertex { float4 position [[position]]; float2 q; float brightness; float aspect; };
float2 rainQuadCorner(uint vid) { return float2(float(vid&1u)*2.0f-1.0f,float((vid>>1)&1u)*2.0f-1.0f); }
vertex RainVertex rain_streak_vertex(uint vid [[vertex_id]],uint instance [[instance_id]],constant Params &u [[buffer(0)]]) {
    RainVertex out;
    float2 extent=extentOf(u),corner=rainQuadCorner(vid);
    float x,head,speed,width,brightness,slope;
    if(instance<RAIN_LANES) {
        float4 drop=u.rain[instance];
        if(drop.w==0.0f||drop.z>=0.0f) {out.position=offscreenVertex;return out;}
        speed=abs(drop.w);
        slope=u.lean[instance];
        head=drop.y+drop.z*speed;
        x=drop.x+slope*(head-drop.y);
        float seed=randomCell(uint2(instance,3),41u);
        width=0.95f+seed*0.65f;
        brightness=1.15f+seed*0.45f;
    } else {
        uint i=instance-RAIN_LANES;
        float seed=randomCell(uint2(i,11),53u);
        speed=620.0f+seed*340.0f;
        float period=(extent.y+260.0f)/speed;
        float t=u.clock.x+randomCell(uint2(i,19),59u)*period;
        uint cycle=uint(t/period);
        float local=t-float(cycle)*period;
        // Slightly steeper than the near rain, which reads as depth. The lane is where the
        // line crosses the top of the window, reaching far enough upwind to fill it.
        slope=u.stroke.x*0.90f;
        float upwind=slope*(extent.y+130.0f);
        float top=randomCell(uint2(i,cycle),61u)*(extent.x+upwind+120.0f)-upwind-60.0f;
        head=local*speed-130.0f;
        x=top+slope*head;
        if(head>rainStop(top,slope,extent.y+130.0f,u)+4.0f) {out.position=offscreenVertex;return out;}
        width=0.50f+seed*0.26f;
        brightness=0.42f+seed*0.46f;
    }
    float length=speed/34.0f;
    float tail=head-length;
    float2 axis=normalize(float2(slope,1.0f));
    float2 side=float2(axis.y,-axis.x);
    float along=(corner.y+1.0f)*0.5f;
    float padding=1.2f;
    float2 point=float2(x,head)-axis*(1.0f-along)*(head-tail)+side*corner.x*(width+padding);
    out.position=toClip(point,extent);
    out.q=float2(corner.x*(width+padding)/width,along);
    out.brightness=brightness;
    out.aspect=width;
    return out;
}
fragment float4 rain_streak_fragment(RainVertex in [[stage_in]]) {
    // Gaussian cross-section, brightest at the leading end, tapering along the blur.
    float taper=in.q.y*in.q.y;
    float value=exp(-in.q.x*in.q.x*1.1f)*taper*in.brightness;
    return float4(value,value,0,0);
}
vertex RainVertex rain_splash_vertex(uint vid [[vertex_id]],uint instance [[instance_id]],constant Params &u [[buffer(0)]]) {
    RainVertex out;
    uint lane=instance/RAIN_BEADS,kind=instance%RAIN_BEADS;
    float4 drop=u.rain[lane];
    float age=drop.z;
    float2 extent=extentOf(u),corner=rainQuadCorner(vid);
    if(drop.w<=0.0f||age<0.0f||age>0.34f) {out.position=offscreenVertex;return out;}
    uint cycle=uint(drop.x*16.0f);
    float strength=0.72f+0.45f*randomCell(uint2(lane,cycle),71u);
    // Secondary droplets: launched from the impact, under real gravity.
    float a=randomCell(uint2(lane*7u+kind,cycle),73u);
    float lean=(a*2.0f-1.0f);
    float2 launch=float2(lean*105.0f+u.lean[lane]*70.0f,-(45.0f+randomCell(uint2(lane*11u+kind,cycle),79u)*105.0f))*strength;
    const float gravity=1250.0f;
    float2 p=float2(drop.x,drop.y)+launch*age+float2(0,0.5f*gravity*age*age);
    float2 velocity=launch+float2(0,gravity*age);
    if(p.y>drop.y+1.0f) {out.position=offscreenVertex;return out;}
    float speed=max(length(velocity),1.0f);
    float2 axis=velocity/speed,side=float2(axis.y,-axis.x);
    float radius=0.65f+0.75f*randomCell(uint2(lane*13u+kind,cycle),83u);
    float stretch=radius+speed/120.0f;
    out.position=toClip(p+axis*corner.y*(stretch+1.2f)+side*corner.x*(radius+1.2f),extent);
    out.q=float2(corner.x*(radius+1.2f)/radius,corner.y*(stretch+1.2f)/stretch);
    out.aspect=-1.0f;
    out.brightness=0.80f*(1.0f-smoothstep(0.16f,0.34f,age));
    return out;
}
fragment float4 rain_splash_fragment(RainVertex in [[stage_in]]) {
    float value=exp(-dot(in.q,in.q)*1.25f)*in.brightness;
    return float4(value,value,0,0);
}
// Water that landed on the bar: a bead clinging to the edge, pulling a thin wet wake.
// Position, velocity, size and strength come from the droplet simulation in weather.h.
struct RunoffDroplet { float4 motion; float4 shape; };
vertex RainVertex rain_runoff_vertex(uint vid [[vertex_id]],uint instance [[instance_id]],constant RunoffDroplet *droplets [[buffer(1)]],constant Params &u [[buffer(0)]]) {
    RainVertex out;
    RunoffDroplet d=droplets[instance];
    if(d.shape.y<=0.0f) {out.position=offscreenVertex;return out;}
    float2 velocity=d.motion.zw;
    float speed=length(velocity);
    float2 axis=speed>0.01f?velocity/speed:float2(1,0),side=float2(axis.y,-axis.x);
    float radius=d.shape.x,wake=min(speed*0.13f,58.0f)+0.01f;
    // q.y runs from the tail of the wake (negative) to the front of the bead (+radius).
    float2 corner=rainQuadCorner(vid);
    float along=corner.y>0.0f?radius+1.2f:-(radius+wake+1.2f);
    float across=corner.x*(radius+1.2f);
    out.position=toClip(d.motion.xy+axis*along+side*across,extentOf(u));
    out.q=float2(across/radius,along/radius);
    out.aspect=wake/radius;
    out.brightness=d.shape.y;
    return out;
}
fragment float4 rain_runoff_fragment(RainVertex in [[stage_in]]) {
    float2 q=in.q;
    // A flattened bead whose upper-left catches the light, not a flat dot.
    float bead=exp(-dot(q,q)*1.15f)*(0.82f+0.34f*saturate(-q.x*0.6f+q.y*0.5f));
    float tail=saturate(-q.y/max(in.aspect,0.01f));
    float wake=q.y<0.0f?exp(-q.x*q.x*5.5f)*(1.0f-tail)*(1.0f-tail)*0.34f*step(tail,1.0f):0.0f;
    float value=(bead+wake)*in.brightness;
    return float4(value,value,0,0);
}

float leavesField(float2 p,float2 extent,float time,float2 pointer,float reaction) {
    float field=0;
    for(uint i=0;i<18;i++) {
        float seed=randomCell(uint2(i,71),219u),depth=0.5f+0.5f*seed;
        float speed=15.0f+depth*23.0f;
        float y=fmod(time*speed+seed*(extent.y+80.0f),extent.y+80.0f)-40.0f;
        float x=(float(i)+0.5f)/18.0f*extent.x+sin(time*(0.4f+seed*0.3f)+seed*32.0f)*32.0f;
        float2 center=float2(x,y),delta=center-pointer;
        // Each leaf is caught by the air around the cursor: blown aside and set spinning.
        float caught=exp(-dot(delta,delta)/(2.0f*95.0f*95.0f))*reaction;
        center+=delta/max(length(delta),1.0f)*caught*78.0f;
        if(any(abs(p-center)>float2(22.0f)))continue;
        float angle=sin(time*0.7f+seed*18.0f)*0.85f+seed*6.28f+caught*(seed>0.5f?2.6f:-2.6f);
        float2 q=p-center;
        q=float2(cos(angle)*q.x-sin(angle)*q.y,sin(angle)*q.x+cos(angle)*q.y);
        float tumble=0.25f+0.75f*abs(cos(time*(0.7f+seed)+seed*24.0f+caught*4.0f));
        float size=9.0f+depth*6.0f;
        float longitudinal=q.y/size;
        // A tapered almond silhouette with a visible midrib and subtle veins.
        float halfWidth=max(0.0f,1.0f-longitudinal*longitudinal)*size*0.43f*tumble;
        float silhouette=(1.0f-smoothstep(halfWidth-0.7f,halfWidth+0.7f,abs(q.x)))*(1.0f-smoothstep(0.94f,1.02f,abs(longitudinal)));
        float midrib=1.0f-smoothstep(0.3f,0.9f,abs(q.x));
        float veins=pow(0.5f+0.5f*cos(q.y*1.2f+abs(q.x)*1.1f),8.0f);
        float shading=0.35f+0.4f*tumble+0.15f*q.x/max(halfWidth,1.0f);
        field+=silhouette*(shading-midrib*0.19f-veins*0.08f)*depth;
    }
    return min(field,1.0f);
}

// ---------------------------------------------------------------------------
// Drift: dust motes carried by a divergence-free (curl-noise) airflow, lit only
// where they cross moving shafts of light. RGBA: x, y, depth, phase.
// ---------------------------------------------------------------------------
constant uint MOTE_COUNT=300;
float lightShafts(float2 p,float2 extent,float t) {
    float s=(p.x+p.y*0.55f)/extent.x;
    float a=exp(-pow((s-0.28f-0.05f*sin(t*0.05f))/0.11f,2.0f));
    float b=exp(-pow((s-0.72f+0.06f*sin(t*0.037f+1.0f))/0.18f,2.0f));
    float c=exp(-pow((s-1.12f-0.04f*sin(t*0.043f+2.0f))/0.08f,2.0f));
    return a+b*0.7f+c*0.8f;
}
kernel void mote_step(texture2d<float,access::read> input [[texture(0)]],texture2d<float,access::write> output [[texture(1)]],
 constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    float2 extent=extentOf(u);
    if(u.impulse.z>0.5f) {
        output.write(float4(randomCell(id,3u)*extent.x,randomCell(id,5u)*extent.y,randomCell(id,7u),randomCell(id,11u)*6.28f),id);
        return;
    }
    float4 m=input.read(id);
    float t=u.clock.x,scale=240.0f,e=1.5f;
    float3 q=float3(m.xy/scale,t*0.045f+m.z*0.9f);
    // Curl of a scalar stream function: incompressible, so motes never clump or thin out.
    float dx=gradientNoise(q+float3(e/scale,0,0))-gradientNoise(q-float3(e/scale,0,0));
    float dy=gradientNoise(q+float3(0,e/scale,0))-gradientNoise(q-float3(0,e/scale,0));
    float2 flow=float2(dy,-dx)/(2.0f*e)*scale*0.5f*(22.0f+m.z*22.0f);
    flow.y-=5.0f+m.z*7.0f; // Gentle buoyancy.
    // The cursor stirs the air: motes get dragged along and swirl around it.
    float2 delta=m.xy-u.pointer.zw*extent;
    float near=exp(-dot(delta,delta)/(2.0f*105.0f*105.0f))*u.animation.z;
    flow+=u.impulse.xy*near*0.55f+float2(-delta.y,delta.x)*near*(0.42f+length(u.impulse.xy)*0.0016f);
    float2 x=m.xy+flow*u.impulse.w;
    if(x.y<-16.0f)x.y+=extent.y+32.0f;
    if(x.y>extent.y+16.0f)x.y-=extent.y+32.0f;
    if(x.x<-16.0f)x.x+=extent.x+32.0f;
    if(x.x>extent.x+16.0f)x.x-=extent.x+32.0f;
    output.write(float4(x,m.z,m.w+u.impulse.w*(0.4f+m.z*0.5f)),id);
}
struct MoteVertex { float4 position [[position]]; float2 q; float brightness; float softness; };
vertex MoteVertex mote_vertex(uint vid [[vertex_id]],uint instance [[instance_id]],texture2d<float> state [[texture(0)]],constant Params &u [[buffer(0)]]) {
    MoteVertex out;
    float4 m=state.read(uint2(instance,0));
    float2 extent=extentOf(u);
    // Depth of field: motes away from the focal plane open into soft bokeh discs.
    float defocus=abs(m.z-0.45f)*2.0f;
    float radius=0.95f+defocus*defocus*6.5f+m.z*0.6f;
    float2 corner=rainQuadCorner(vid);
    out.position=toClip(m.xy+corner*(radius+1.2f),extent);
    out.q=corner*(radius+1.2f)/radius;
    // A slowly tumbling flake catches the light on and off; it never blinks.
    float glint=0.5f+0.5f*sin(m.w);
    // The cursor also holds a small light, so nearby dust lights up as it passes.
    float2 toCursor=m.xy-u.pointer.zw*extent;
    float held=exp(-dot(toCursor,toCursor)/(2.0f*85.0f*85.0f))*u.animation.z*1.7f;
    float lit=0.10f+lightShafts(m.xy,extent,u.clock.x)*1.15f+held;
    float energy=1.0f/(1.0f+radius*radius*0.28f);
    out.brightness=lit*(0.30f+glint*glint*0.70f)*energy*5.2f;
    out.softness=smoothstep(1.5f,6.0f,radius);
    return out;
}
fragment float4 mote_fragment(MoteVertex in [[stage_in]]) {
    float r=length(in.q);
    float core=1.0f-smoothstep(mix(0.45f,0.82f,in.softness),1.0f,r);
    float rim=in.softness*exp(-pow((r-0.86f)/0.10f,2.0f))*0.35f;
    float value=(core+rim)*in.brightness;
    return float4(value,value,0,0);
}

// ---------------------------------------------------------------------------
// Pudding: a soft-body shell on a plate. The plate is the input bar: when the bar
// moves the pudding is shoved and wobbles. Each surface point is tied to its rest
// shape by a spring that weakens towards the top, coupled to its neighbours, so
// kicks become real travelling waves. Rendered with glossy window reflections,
// subsurface glow and a caramel cap. Positions are relative to the plate centre.
// ---------------------------------------------------------------------------
constant int PUDDING_AROUND=128,PUDDING_RINGS=64;
struct PuddingRig { float2 base; float radius,height; };
PuddingRig puddingRig(constant Params &u) {
    float2 extent=extentOf(u);
    float4 bar=regionOf(u.composer,u);
    PuddingRig r;
    bool hasBar=bar.z>0&&bar.w>0;
    float span=hasBar?bar.z:extent.x*0.8f;
    r.radius=clamp(min(span*0.40f,extent.y*0.40f),70.0f,360.0f);
    r.height=r.radius*0.76f;
    r.base=float2(hasBar?bar.x+bar.z*0.5f:extent.x*0.5f,(hasBar?bar.y:extent.y*0.94f)+16.0f);
    return r;
}
float3 puddingRest(int2 p,PuddingRig r) {
    float phi=float(p.y)/float(PUDDING_RINGS-1)*1.5707963f; // 0 at the crown, pi/2 at the plate.
    float theta=float(p.x)/float(PUDDING_AROUND)*6.2831853f;
    // A flan: flat top rolling into sides that flare out towards the plate.
    float radial=pow(sin(phi),0.42f),up=pow(cos(phi),0.62f);
    float flare=mix(0.72f,1.0f,1.0f-up);
    float ring=r.radius*radial*flare;
    return float3(ring*cos(theta),-r.height*up,ring*sin(theta));
}
constant float puddingTilt=0.42f;
float3 puddingView(float3 x) {
    float c=cos(puddingTilt),s=sin(puddingTilt);
    return float3(x.x,x.y*c+x.z*s,-x.y*s+x.z*c);
}
kernel void pudding_step(texture2d<float,access::read> position [[texture(0)]],texture2d<float,access::read> velocity [[texture(1)]],
 texture2d<float,access::write> nextPosition [[texture(2)]],texture2d<float,access::write> nextVelocity [[texture(3)]],
 constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    PuddingRig r=puddingRig(u);
    int2 p=int2(id);
    float3 rest=puddingRest(p,r);
    if(u.impulse.z>0.5f||p.y==PUDDING_RINGS-1) {
        nextPosition.write(float4(rest,1),id);nextVelocity.write(float4(0),id);return;
    }
    float3 x=position.read(id).xyz,v=velocity.read(id).xyz;
    float3 offset=x-rest;
    float3 neighbours=0;float count=0;
    const int2 around[4]={int2(1,0),int2(-1,0),int2(0,1),int2(0,-1)};
    for(int i=0;i<4;i++) {
        int2 n=p+around[i];
        n.x=(n.x+PUDDING_AROUND)%PUDDING_AROUND;
        if(n.y<0||n.y>=PUDDING_RINGS)continue;
        neighbours+=position.read(uint2(n)).xyz-puddingRest(n,r);count+=1;
    }
    float top=1.0f-float(p.y)/float(PUDDING_RINGS-1);
    float dt=u.impulse.w;
    float3 accel=-offset*mix(620.0f,120.0f,top)+(neighbours/max(count,1.0f)-offset)*30000.0f;
    accel-=v*mix(5.0f,2.1f,top);
    // Inertia: the body lags the plate, more so higher up, so kicks shear it into a lean.
    float lever=top*top*(3.0f-2.0f*top);
    accel+=float3(u.stroke.x,u.stroke.y,u.stroke.w)*lever;
    // Held: the material under the cursor is pulled with it, weighted so a soft region
    // stretches like taffy and drags its neighbours. life.xy is the grabbed spot, clock.zw
    // the pull and life.z whether it is held. Letting go simply stops the pull, and the
    // stored stretch throws it back into a wobble. Hovering does nothing at all.
    float grab=u.life.z;
    if(grab>0.001f) {
        float2 away=puddingView(rest).xy-u.life.xy;
        const float spread=108.0f;
        float weight=exp(-dot(away,away)/(2.0f*spread*spread))*grab;
        float2 target=rest.xy+u.clock.zw*weight;
        accel.xy+=(target-x.xy)*weight*3200.0f;
        accel-=v*weight*9.0f;
    }
    v+=accel*dt;x+=v*dt;
    float3 limit=x-rest;
    float reach=length(limit);
    float longest=r.radius*(u.life.z>0.001f?1.7f:0.42f);
    if(reach>longest)x=rest+limit*longest/reach;
    if(!all(isfinite(x))||!all(isfinite(v))){x=rest;v=0;}
    nextPosition.write(float4(x,1),id);
    nextVelocity.write(float4(v,0),id);
}
struct PuddingVertex { float4 position [[position]]; float3 normal; float3 rest; float2 parameter; };
int wrapAround(int x) { return (x+PUDDING_AROUND)%PUDDING_AROUND; }
vertex PuddingVertex pudding_vertex(uint vid [[vertex_id]],texture2d<float> position [[texture(0)]],constant Params &u [[buffer(0)]]) {
    PuddingVertex out;
    PuddingRig r=puddingRig(u);
    int2 p=meshCorner(vid,PUDDING_AROUND+1,PUDDING_RINGS);
    int2 w=int2(wrapAround(p.x),p.y);
    float3 x=position.read(uint2(w)).xyz;
    float3 l=position.read(uint2(wrapAround(w.x-1),w.y)).xyz,right=position.read(uint2(wrapAround(w.x+1),w.y)).xyz;
    float3 upper=position.read(uint2(w.x,max(w.y-1,0))).xyz,lower=position.read(uint2(w.x,min(w.y+1,PUDDING_RINGS-1))).xyz;
    float3 n=cross(right-l,lower-upper);
    if(w.y==0)n=float3(0,-1,0);
    if(dot(n,x-float3(0,-r.height*0.35f,0))<0)n=-n;
    float3 viewed=puddingView(x);
    out.normal=puddingView(normalize(n));
    out.rest=puddingRest(w,r);
    out.parameter=float2(float(p.x)/float(PUDDING_AROUND),float(p.y)/float(PUDDING_RINGS-1));
    float2 extent=extentOf(u);
    out.position=toClip(r.base+viewed.xy,extent,0.5f-viewed.z/(r.radius*4.0f));
    return out;
}
fragment float4 pudding_fragment(PuddingVertex in [[stage_in]],constant Params &u [[buffer(0)]]) {
    // It sits on the bar: anything below the plate's middle does not exist.
    PuddingRig rig=puddingRig(u);
    float scale=u.blur.z;
    if(in.position.y/scale/max(u.geometry.z,0.1f)>rig.base.y+regionOf(u.composer,u).w*0.25f)discard_fragment();
    float3 n=normalize(in.normal),view=float3(0,0,1);
    float nv=saturate(dot(n,view));
    float3 key=normalize(float3(-0.48f,-0.62f,0.62f));
    float wrap=saturate((dot(n,key)+0.45f)/1.45f);
    // Light scatters through the custard, so thin rims and the lower body glow.
    float rim=pow(1.0f-nv,2.6f);
    float lower=smoothstep(0.35f,1.0f,in.parameter.y);
    float custard=0.17f+wrap*wrap*0.40f+rim*0.20f+lower*0.14f;
    // Caramel: a darker, glassier cap whose edge runs down the sides in drips.
    float angle=in.parameter.x*6.2831853f;
    float drips=sin(angle*3.0f+1.2f)*0.05f+sin(angle*7.0f+0.4f)*0.035f+sin(angle*13.0f+2.1f)*0.020f;
    float cap=1.0f-smoothstep(-0.008f,0.008f,in.parameter.y-(0.40f+drips));
    float body=mix(custard,custard*0.50f+0.055f,cap);
    float gloss=mix(0.62f,1.0f,cap);
    float3 mirror=reflect(-view,n);
    // A mullioned window mirrored across the wet surface: this is what reads as glossy.
    float2 m=mirror.xy/max(mirror.z+0.65f,0.2f);
    // A long softbox bent across the surface, plus the faint echo of a second light.
    float2 pane=m-float2(-0.22f,-0.40f);
    pane.x+=pane.y*0.55f;
    float2 box=abs(pane)-float2(0.20f,0.052f);
    float boxDistance=length(max(box,0.0f))+min(max(box.x,box.y),0.0f)-0.035f;
    float window=1.0f-smoothstep(-0.006f,0.010f,boxDistance);
    float2 echo=m-float2(0.34f,-0.10f);
    window+=exp(-dot(echo,echo)/0.0045f)*0.30f;
    float tight=pow(saturate(dot(n,normalize(key+view))),90.0f);
    float fresnel=0.04f+0.96f*pow(1.0f-nv,5.0f);
    float occlusion=mix(1.0f,0.45f,smoothstep(0.82f,1.0f,in.parameter.y));
    float L=(body*occlusion+(window*0.92f+tight*0.65f)*gloss+fresnel*0.26f);
    return float4(L,1.0f,0,1.0f);
}

// ---------------------------------------------------------------------------
// Contours: a slowly evolving terrain evaluated into a height grid, drawn with
// constant-width isolines (distance is value over gradient), heavier index
// contours every fifth level, and faint hill shading for relief.
// ---------------------------------------------------------------------------
kernel void contour_step(texture2d<float,access::write> output [[texture(0)]],constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    if(any(float2(id)>=u.clock.zw))return;
    float2 extent=extentOf(u);
    float2 p=(float2(id)+0.5f)/u.clock.zw*extent;
    float t=u.clock.x*0.035f;
    float3 q=float3(p/620.0f,t);
    float3 warp=float3(gradientNoise(q+float3(5.2f,1.3f,0),2u),gradientNoise(q+float3(1.7f,9.2f,0),4u),0);
    float h=fractalNoise(float3(p/620.0f+warp.xy*0.9f,t),4,8u);
    // The cursor raises a low, broad swell in the terrain, so nearby lines lean gently
    // around it the way real ground would, adding a ring or two at most.
    float2 delta=p-u.pointer.zw*extent;
    h+=exp(-dot(delta,delta)/(2.0f*120.0f*120.0f))*u.animation.z*0.055f;
    output.write(float4(h),id);
}
float contourLines(float2 uv,float2 extent,texture2d<float> grid) {
    Surface s=splineSurface(grid,uv,extent/float2(grid.get_width(),grid.get_height()));
    const float levels=46.0f;
    float phase=s.height*levels;
    float slope=max(length(s.gradient)*levels,0.0006f);
    float nearest=floor(phase+0.5f);
    float distance=abs(phase-nearest)/slope;
    bool index=fmod(abs(nearest),5.0f)<0.5f;
    float halfWidth=index?0.85f:0.42f;
    float aa=0.5f*max(fwidth(uv.x)*extent.x,fwidth(uv.y)*extent.y);
    float line=1.0f-smoothstep(halfWidth-aa,halfWidth+aa,distance);
    // Very steep ground packs lines together; fade them rather than letting them moire.
    line*=1.0f-smoothstep(0.10f,0.32f,slope)*(index?0.2f:0.85f);
    float3 normal=normalize(float3(-s.gradient*520.0f,1.0f));
    float relief=saturate(dot(normal,normalize(float3(-0.6f,-0.6f,0.5f))));
    return line*(index?1.05f:0.62f)+relief*relief*0.14f;
}

// ---------------------------------------------------------------------------
// Fluted glass: fixed cylindrical ribs refract moving lights behind the pane using
// Snell's law and Fresnel transmission. The cursor is one more light behind the
// glass, so it bends through the ribs while the ribs themselves stay put.
// ---------------------------------------------------------------------------
float glassBackdrop(float2 p,float2 extent,float t,float2 pointer,float reaction) {
    float2 q=p/extent.y;
    float light=0;
    for(int i=0;i<6;i++) {
        float f=float(i);
        float2 center=float2(0.5f*extent.x/extent.y+sin(t*(0.043f+f*0.009f)+f*2.4f)*0.58f*extent.x/extent.y,0.46f+cos(t*(0.037f+f*0.011f)+f*1.3f)*0.42f);
        float size=0.15f+0.08f*sin(f*1.9f);
        float2 d=q-center;
        light+=exp(-dot(d,d)/(size*size*2.2f))*(0.75f+0.35f*sin(f*3.1f));
    }
    float2 d=(p-pointer)/110.0f;
    return light+exp(-dot(d,d))*reaction*0.9f;
}
float flutedGlass(float2 p,float2 extent,float t,float2 pointer,float reaction) {
    const float pitch=27.0f,index=1.52f,thickness=74.0f;
    float rib=fract(p.x/pitch),centered=(rib-0.5f)*2.0f;
    float incidence=asin(clamp(centered*0.93f,-1.0f,1.0f));
    float refracted=asin(sin(incidence)/index);
    float bend=tan(incidence-refracted)*thickness;
    float2 sample=float2(p.x+bend,p.y);
    // Dispersion-free but slightly blurred through the depth of the glass.
    float backdrop=glassBackdrop(sample,extent,t,pointer,reaction)*0.6f
        +glassBackdrop(sample+float2(0,7.0f),extent,t,pointer,reaction)*0.2f
        +glassBackdrop(sample-float2(0,7.0f),extent,t,pointer,reaction)*0.2f;
    // Uneven light behind the pane: a smoke-like veil, sampled once per pixel.
    backdrop*=1.05f+1.2f*gradientNoise(float3(sample/extent.y*2.4f,t*0.05f),41u);
    float cosine=cos(incidence);
    float r0=pow((index-1.0f)/(index+1.0f),2.0f);
    float reflectance=r0+(1.0f-r0)*pow(1.0f-cosine,5.0f);
    float transmitted=backdrop*(1.0f-reflectance)*(0.42f+0.58f*cosine);
    // Grooves between ribs stay dark, with a thin bright lip on the lit side.
    float groove=smoothstep(0.0f,0.05f,min(rib,1.0f-rib));
    float lip=exp(-pow((rib-0.13f)/0.035f,2.0f))*(0.10f+saturate(backdrop)*0.34f);
    float sheen=exp(-pow((rib-0.30f)/0.14f,2.0f))*saturate(backdrop)*0.20f;
    return transmitted*groove+lip+sheen;
}

// ---------------------------------------------------------------------------
// Clouds: single-scattering volume render through a slab of Perlin-Worley noise,
// with a shadow ray towards the sun at every step, Beer-Powder extinction and a
// Henyey-Greenstein silver lining. Jittered and accumulated over time.
// ---------------------------------------------------------------------------
float worleyTiled(float3 p,int cells) {
    float3 g=p*float(cells);
    int3 c=int3(floor(g));
    float nearest=1.0f;
    for(int z=-1;z<=1;z++)for(int y=-1;y<=1;y++)for(int x=-1;x<=1;x++) {
        int3 n=c+int3(x,y,z);
        int3 w=((n%cells)+cells)%cells;
        uint h=uint(w.x)*73856093u^uint(w.y)*19349663u^uint(w.z)*83492791u;
        float3 feature=float3(randomCell(uint2(h,1),7u),randomCell(uint2(h,2),7u),randomCell(uint2(h,3),7u));
        nearest=min(nearest,length(float3(n)+feature-g));
    }
    return 1.0f-saturate(nearest);
}
float periodicNoise(float3 p,int period) {
    float3 g=p*float(period),i=floor(g),f=g-i,w=f*f*f*(f*(f*6.0f-15.0f)+10.0f);
    int3 c=int3(i);
    float values[8];
    for(int k=0;k<8;k++) {
        int3 o=int3(k&1,(k>>1)&1,(k>>2)&1);
        int3 wrapped=(((c+o)%period)+period)%period;
        values[k]=dot(latticeGradient(wrapped,period),f-float3(o));
    }
    return mix(mix(mix(values[0],values[1],w.x),mix(values[2],values[3],w.x),w.y),mix(mix(values[4],values[5],w.x),mix(values[6],values[7],w.x),w.y),w.z);
}
float tiledNoise2(float2 p,int2 period,uint seed) {
    float2 i=floor(p),f=p-i,w=f*f*f*(f*(f*6.0f-15.0f)+10.0f);
    int2 c=int2(i);
    float v[4];
    for(int k=0;k<4;k++) {
        int2 o=int2(k&1,k>>1);
        int2 wrapped=(((c+o)%period)+period)%period;
        v[k]=dot(latticeGradient(int3(wrapped,0),seed).xy,f-float2(o));
    }
    return mix(mix(v[0],v[1],w.x),mix(v[2],v[3],w.x),w.y);
}
float fabricCrease(float2 uv) {
    return tiledNoise2(uv*float2(30.0f,3.0f),int2(30,3),43u)+tiledNoise2(uv*float2(78.0f,8.0f),int2(78,8),47u)*0.34f
        +tiledNoise2(uv*float2(190.0f,19.0f),int2(190,19),49u)*0.10f;
}
// Baked once: crease slope in XY, thread density in Z. It tiles over 1024 fabric pixels.
kernel void cloth_detail(texture2d<float,access::write> output [[texture(0)]],uint2 id [[thread_position_in_grid]]) {
    float2 size=float2(output.get_width(),output.get_height()),uv=(float2(id)+0.5f)/size,e=1.0f/size;
    float crease=fabricCrease(uv);
    float2 slope=float2(fabricCrease(uv+float2(e.x,0))-crease,fabricCrease(uv+float2(0,e.y))-crease)*size.x/1024.0f*7.0f;
    float slub=tiledNoise2(uv*float2(420.0f,22.0f),int2(420,22),53u)*0.5f+tiledNoise2(uv*float2(24.0f,400.0f),int2(24,400),59u)*0.5f;
    output.write(float4(slope,slub,1),id);
}
kernel void cloud_noise(texture3d<float,access::write> output [[texture(0)]],uint3 id [[thread_position_in_grid]]) {
    float3 p=(float3(id)+0.5f)/float(output.get_width());
    float perlin=periodicNoise(p,4)*0.5f+periodicNoise(p,8)*0.25f+periodicNoise(p,16)*0.125f+0.5f;
    float w1=worleyTiled(p,4),w2=worleyTiled(p,8),w3=worleyTiled(p,16),w4=worleyTiled(p,24);
    float billow=w1*0.625f+w2*0.25f+w3*0.125f;
    float shape=saturate(perlin*0.55f+billow*0.72f-0.18f);
    output.write(float4(shape,w2*0.625f+w3*0.25f+w4*0.125f,w3,1),id);
}
constexpr sampler cloudSampler(coord::normalized,address::repeat,filter::linear);
float cloudDensity(float3 p,float t,float2 extent,texture3d<float> noise,bool detail) {
    float altitude=p.y/extent.y;
    float3 q=p/1500.0f+float3(-t*0.0030f,t*0.0004f,t*0.0012f);
    float4 base=noise.sample(cloudSampler,q);
    // Weather: larger banks of cloud separated by open sky, heavier towards the top.
    float3 w=p/3600.0f+float3(-t*0.0016f,0,t*0.0007f);
    float weather=noise.sample(cloudSampler,w).x;
    float coverage=saturate(weather*2.1f-0.28f)*mix(1.0f,0.80f,smoothstep(0.1f,1.0f,altitude));
    float threshold=1.0f-coverage*0.80f;
    float shape=saturate((base.x-threshold)/max(1.0f-threshold,0.05f));
    if(shape<=0.0f||!detail)return shape;
    float erosion=noise.sample(cloudSampler,q*4.1f+float3(t*0.0026f,0,0)).y*0.72f+noise.sample(cloudSampler,q*13.0f).z*0.28f;
    return saturate((shape-(1.0f-shape)*erosion*0.78f)/0.70f);
}
constexpr sampler cloudWakeSampler(coord::normalized,address::clamp_to_edge,filter::linear);
// The cursor clears a real trail through the cloud deck, like an aircraft's dissipation
// trail: it stays open behind the cursor, spreads a little and slowly fills back in.
kernel void cloud_wake(texture2d<float,access::read> input [[texture(0)]],texture2d<float,access::write> output [[texture(1)]],
 constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    int2 size=int2(output.get_width(),output.get_height()),p=int2(id);
    float2 extent=extentOf(u);
    float2 position=(float2(id)+0.5f)/float2(size)*extent,delta=position-u.pointer.zw*extent;
    float average=(input.read(uint2(max(p-int2(1,0),int2(0)))).x+input.read(uint2(min(p+int2(1,0),size-1))).x
        +input.read(uint2(max(p-int2(0,1),int2(0)))).x+input.read(uint2(min(p+int2(0,1),size-1))).x)*0.25f;
    float value=mix(input.read(id).x,average,0.10f)*exp(-u.impulse.w/8.0f);
    value=max(value,exp(-dot(delta,delta)/(2.0f*62.0f*62.0f))*u.animation.z);
    output.write(float4(value),id);
}
kernel void cloud_march(texture3d<float> noise [[texture(0)]],texture2d<float> history [[texture(1)]],
 texture2d<float,access::write> output [[texture(2)]],texture2d<float> wake [[texture(3)]],constant Params &u [[buffer(0)]],uint2 id [[thread_position_in_grid]]) {
    float2 extent=extentOf(u),size=float2(output.get_width(),output.get_height());
    float2 uv=(float2(id)+0.5f)/size;
    float2 xy=uv*extent;
    float clearing=wake.sample(cloudWakeSampler,uv).x;
    float t=u.clock.x;
    const float depth=520.0f;const int steps=30;
    const float3 sun=normalize(float3(-0.62f,-0.56f,0.46f));
    float jitter=fract(interleavedGradient(float2(id))+u.life.y*0.61803398f);
    float stepLength=depth/float(steps);
    float transmittance=1.0f,light=0.0f;
    float cosine=dot(sun,float3(0,0,1));
    const float g1=0.62f,g2=-0.18f;
    float hg1=(1.0f-g1*g1)/pow(1.0f+g1*g1-2.0f*g1*cosine,1.5f);
    float hg2=(1.0f-g2*g2)/pow(1.0f+g2*g2-2.0f*g2*cosine,1.5f);
    float phase=(mix(hg2,hg1,0.55f))*0.8f;
    for(int i=0;i<steps&&transmittance>0.008f;i++) {
        float3 p=float3(xy,(float(i)+jitter)*stepLength-depth*0.5f);
        float density=cloudDensity(p,t,extent,noise,true)*(1.0f-clearing);
        if(density<=0.002f)continue;
        float sigma=density*0.040f;
        // March to the sun through the same volume for self-shadowing.
        float optical=0;
        float shadowStep=26.0f;
        for(int s=1;s<=6;s++) {
            optical+=cloudDensity(p+sun*shadowStep*(float(s)-0.5f),t,extent,noise,s<=2)*shadowStep;
            shadowStep*=1.26f;
        }
        float beer=exp(-optical*0.040f);
        float powder=1.0f-exp(-optical*0.085f);
        float direct=beer*(0.25f+0.75f*powder)*phase*2.6f;
        float ambient=mix(0.20f,0.05f,saturate(p.z/depth+0.5f))+density*0.02f;
        float extinction=exp(-sigma*stepLength);
        light+=transmittance*(1.0f-extinction)*(direct+ambient);
        transmittance*=extinction;
    }
    float2 result=float2(light,1.0f-transmittance);
    if(u.impulse.z<0.5f) result=mix(history.read(id).xy,result,0.16f);
    output.write(float4(result,0,1),id);
}

struct Vertex { float4 position [[position]]; float2 uv; };
vertex Vertex fluid_vertex(uint id [[vertex_id]]) {
    float2 p=float2((id<<1)&2,id&2);
    return {float4(p*2.0f-1.0f,0,1),float2(p.x,1.0f-p.y)};
}
// A camera's optics scatter a little light around anything bright.
float fieldGlow(texture2d<float> field,float2 uv) {
    constexpr sampler mipSampler(coord::normalized,address::clamp_to_edge,filter::linear,mip_filter::linear);
    return field.sample(mipSampler,uv,level(3.0f)).x*0.35f+field.sample(mipSampler,uv,level(4.5f)).x*0.35f+field.sample(mipSampler,uv,level(6.0f)).x*0.30f;
}
fragment float4 fluid_fragment(Vertex in [[stage_in]],texture2d<float,access::read> cells [[texture(0)]],texture2d<float,access::read> highlight [[texture(1)]],texture2d<float> grid [[texture(2)]],texture2d<float> field [[texture(3)]],texture2d<float> extra [[texture(4)]],constant Params &u [[buffer(0)]]) {
    float2 logical=in.uv*u.geometry.xy;
    uint style=uint(u.animation.x);
    bool lightMode=u.clock.y>0.5f;
    float zoom=max(u.geometry.z,0.1f);
    float2 p=logical/zoom,extent=extentOf(u),pointer=u.pointer.zw*extent;
    constexpr sampler fieldSampler(coord::normalized,address::clamp_to_edge,filter::linear);
    float value=0,limit=0.30f,fadeFloor=0,frame=0.035f;
    if(style==2u) { // Conway Life retains exact B3/S23 topology.
        float2 lattice=in.uv*u.clock.zw;
        uint2 cell=min(uint2(lattice),uint2(u.clock.zw)-1);
        float4 state=cells.read(cell);
        float2 q=abs(fract(lattice)-0.5f);
        float aa=max(fwidth(q.x),fwidth(q.y))*0.5f;
        float pixels=1.0f-smoothstep(0.31f-aa,min(0.49f,0.46f+aa),max(q.x,q.y));
        float heat=u.animation.y>0.5f?clamp(highlight.read(cell).x,0.0f,1.0f):0.0f;
        // Living cells flare near the cursor, and the empty grid around it faintly shows.
        value=(state.z*mix(0.62f,1.30f,heat)+(1.0f-state.z)*heat*0.20f)*pixels*1.7f;
    } else if(style==3u) { // Liquid.
        constexpr sampler waterSampler(coord::normalized,address::clamp_to_edge,filter::linear);
        float disturbance=u.animation.y>0.5f?grid.sample(waterSampler,in.uv).x:0.0f;
        value=liquidField(p,u.clock.x,disturbance);
        limit=0.20f;
    } else if(style==4u) { // Contours.
        value=contourLines(in.uv,extent,grid)*1.9f;
        fadeFloor=0.18f;
    } else if(style==5u) { // Drift: motes plus the faint haze that shows the shafts themselves.
        float2 s=field.sample(fieldSampler,in.uv).xy;
        float haze=lightShafts(p,extent,u.clock.x)*(lightMode?0.12f:0.30f);
        value=s.x*2.1f+haze;
        fadeFloor=0.35f;limit=0.34f;
    } else if(style==6u) { // Fluted glass.
        value=flutedGlass(p,extent,u.clock.x,pointer,u.animation.z)*1.75f;
        fadeFloor=0.25f;limit=0.34f;frame=0.012f;
    } else if(style==7u) { // Curtains.
        float L=field.sample(fieldSampler,in.uv).x;
        float glow=fieldGlow(field,in.uv);
        float bright=L+glow*0.30f;
        bright=bright/(1.0f+bright*0.30f); // A filmic shoulder keeps the hottest panes from clipping.
        value=lightMode?saturate(1.0f-bright*1.3f)*1.5f:bright*3.3f;
        fadeFloor=0.62f;limit=lightMode?0.28f:0.46f;frame=0.006f;
    } else if(style==12u) { // Window Curtains.
        float L=field.sample(fieldSampler,in.uv).x;
        float shafts=extra.sample(fieldSampler,in.uv).x;
        // Air is never even: slow drifting haze.
        float haze=0.78f+0.55f*fractalNoise(float3(p/230.0f+float2(u.clock.x*0.012f,-u.clock.x*0.008f),u.clock.x*0.02f),2,257u);
        constexpr sampler wide(coord::normalized,address::clamp_to_edge,filter::linear,mip_filter::linear);
        float near=extra.sample(wide,in.uv,level(2.5f)).x;
        float spread=extra.sample(wide,in.uv,level(3.8f)).x*0.55f+extra.sample(wide,in.uv,level(5.2f)).x*0.45f;
        // Where fabric hangs across the view a beam is seen through or against it, so the
        // weave spreads it wide and takes some of its light: soft rays, never a hard edge.
        // Beams stay sharp only in clear air, so the cloth visibly divides the light.
        float covered=saturate((field.sample(fieldSampler,in.uv).y-0.5f)/0.10f); // Backdrop G is 0.5.
        shafts=mix(shafts+near*0.30f,spread*0.95f,covered)*haze;
        // Sunbeams, and the dust that only shows inside them.
        float beams=shafts*3.2f+shafts*beamDust(p,u.clock.x)*2.4f;
        float bright=L+fieldGlow(field,in.uv)*0.30f+beams;
        limit=lightMode?0.30f:0.56f;
        // A film shoulder that arrives exactly at the brightest allowed value, so the sun
        // burns out smoothly and never clips to a flat shape.
        float top=limit/max(u.tint.a,0.001f);
        value=lightMode?saturate(1.0f-bright/(1.0f+bright*0.42f)*1.5f)*1.5f:top*(1.0f-exp(-bright*3.7f/top));
        fadeFloor=0.78f;frame=0.006f;
    } else if(style==13u) { // Seafloor.
        float L=seafloorLight(in.uv,extent,grid,field);
        value=lightMode?saturate(L*1.15f)*1.5f:L*3.4f;
        // Edge to edge, top to bottom.
        fadeFloor=1.0f;limit=lightMode?0.26f:0.34f;frame=0.0002f;
    } else if(style==8u) { // Rain.
        value=field.sample(fieldSampler,in.uv).x*3.6f;
        fadeFloor=0.70f;limit=0.60f;
    } else if(style==9u) { // Clouds.
        float2 s=field.sample(fieldSampler,in.uv).xy;
        value=lightMode?s.y*saturate(1.05f-s.x*1.5f)*1.8f:s.x*3.3f;
        fadeFloor=0.32f;limit=lightMode?0.30f:0.56f;frame=0.02f;
    } else if(style==10u) { // Falling leaves.
        value=leavesField(p,extent,u.clock.x,pointer,u.animation.z)*2.0f;
        fadeFloor=0.45f;limit=0.20f;
    } else if(style==11u) { // Pudding.
        float2 s=field.sample(fieldSampler,in.uv).xy;
        float glow=fieldGlow(field,in.uv);
        value=lightMode?s.y*saturate(1.05f-s.x*1.1f)*2.5f:(s.x+glow*0.10f)*3.3f;
        fadeFloor=0.90f;limit=lightMode?0.40f:0.62f;
    }
    float edge=smoothstep(0.0f,frame,min(min(in.uv.x,1-in.uv.x),min(in.uv.y,1-in.uv.y)));
    float verticalFade=mix(1.0f-clamp(in.uv.y,0.0f,1.0f),1.0f,fadeFloor);
    float alpha=value*u.tint.a*edge*verticalFade;
    float2 halfSize=u.geometry.xy*0.5f;
    float radius=u.geometry.w;
    float2 rounded=abs(logical-halfSize)-halfSize+radius;
    float sdf=length(max(rounded,0.0f))+min(max(rounded.x,rounded.y),0.0f)-radius;
    alpha*=1.0f-smoothstep(-0.5f,0.5f,sdf);
    // Dither before the 8-bit surface: long soft gradients must not show banding.
    alpha+=(interleavedGradient(in.position.xy)-0.5f)/255.0f*step(0.002f,alpha);
    return float4(u.tint.rgb,clamp(alpha,0.0f,limit));
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
// A wide blur for behind a chat or terminal: separable, sampling every fourth logical pixel.
kernel void strong_blur(texture2d<float,access::sample> source [[texture(0)]],texture2d<float,access::write> output [[texture(1)]],
 constant float2 &direction [[buffer(1)]],uint2 id [[thread_position_in_grid]]) {
    float2 uv=(float2(id)+0.5f)/float2(output.get_width(),output.get_height());
    float4 sum=0;float total=0;
    for(int i=-9;i<=9;i++){
        float weight=exp(-float(i*i)/32.0f);
        sum+=source.sample(sceneSampler,uv+direction*float(i))*weight;total+=weight;
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
 texture2d<float> blurred [[texture(1)]],texture2d<float> strong [[texture(2)]],constant Params &u [[buffer(0)]]) {
    float2 p=in.uv*u.geometry.xy;
    // Only behind the bars themselves: the space between the footer's groups stays clear.
    float mask=max(regionMask(p,u.composer,u.blur.x,false,u.geometry.z),
                   max(regionMask(p,u.footer,u.blur.y,true,u.geometry.z),regionMask(p,u.footerSide,u.blur.y,true,u.geometry.z)));
    float4 sharp=scene.sample(sceneSampler,in.uv);
    float4 soft=blurred.sample(sceneSampler,in.uv); // premultiplied Gaussian
    float4 result=mix(float4(sharp.rgb*sharp.a,sharp.a),soft,mask);
    // Behind a conversation or terminal the whole scene melts into soft, dimmer light.
    // A little of the sharp picture stays in the mix, so fine shapes such as glass ribs still read.
    if(u.blur.w>0.0f) {
        float4 frosted=mix(strong.sample(sceneSampler,in.uv),float4(sharp.rgb*sharp.a,sharp.a),0.34f)*0.90f;
        result=mix(result,frosted,u.blur.w);
    }
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
