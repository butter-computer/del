// Rain trajectories shared by rendering and the runoff simulation, so every visible
// splash leaves exactly one bead. Logical coordinates keep density and fall
// speed independent of display scale. Keep DEL_RAIN_COUNT equal to the size of
// Params.rain in fluid.metal.
#pragma once
#include <math.h>
#include <stdbool.h>
#include <stdint.h>
#include <simd/simd.h>
#define DEL_RAIN_COUNT 64
#define DEL_RAIN_MIN_SPEED 1050.0f
#define DEL_RAIN_MAX_SPEED 1650.0f
static inline float del_weather_random(uint32_t id) {
    uint32_t h=id*374761393u+217u*2246822519u;
    h=(h^(h>>13))*1274126177u;h^=h>>16;
    return (float)(h&0xffffffu)/16777216.0f;
}
// Signed distance to a rounded bar; negative inside. A bar with no area is never hit.
static inline float del_rain_bar(simd_float2 p,simd_float4 rect,float radius) {
    if(rect.z<=0||rect.w<=0)return 1e9f;
    float r=fminf(radius,fminf(rect.z,rect.w)*0.5f);
    float qx=fabsf(p.x-(rect.x+rect.z*0.5f))-rect.z*0.5f+r,qy=fabsf(p.y-(rect.y+rect.w*0.5f))-rect.w*0.5f+r;
    float ax=fmaxf(qx,0),ay=fmaxf(qy,0);
    return sqrtf(ax*ax+ay*ay)+fminf(fmaxf(qx,qy),0)-r;
}
// Showers build and ease over tens of seconds: the rhythm is never a metronome.
static inline float del_rain_intensity(float time) {
    return 0.52f+0.30f*sinf(time*0.17f)+0.18f*sinf(time*0.41f+1.3f);
}
// How far the rain leans, in x per unit of fall: about 22 degrees, with slow gusts.
static inline float del_rain_slope(float time) {
    return 0.40f+0.045f*sinf(time*0.23f)+0.025f*sinf(time*0.61f+0.7f);
}
// March one straight slanted drop until it meets a bar or the umbrella, and report where.
// The same walk runs on the GPU for the far rain, so both layers share one collision.
static inline float del_rain_scene(simd_float2 p,simd_float4 composer,simd_float4 footer,simd_float4 side,simd_float2 radii,simd_float3 shelter) {
    float d=fminf(del_rain_bar(p,composer,radii.x),fminf(del_rain_bar(p,footer,radii.y),del_rain_bar(p,side,radii.y)));
    if(shelter.z>=1)d=fminf(d,simd_distance(p,(simd_float2){shelter.x,shelter.y})-shelter.z);
    return d;
}
static inline simd_float2 del_rain_land(float top,float slope,float bottom,simd_float4 composer,simd_float4 footer,simd_float4 side,simd_float2 radii,simd_float3 shelter) {
    float scale=1.0f/sqrtf(1+slope*slope),y=-60;
    for(int i=0;i<48&&y<bottom;i++) {
        simd_float2 p={top+slope*y,y};
        float d=del_rain_scene(p,composer,footer,side,radii,shelter);
        if(d<0.02f)return p;
        y+=d*scale;
    }
    return (simd_float2){top+slope*bottom,bottom};
}
// x, y of where the drop stops, seconds relative to that moment, and signed fall speed
// (+ hits something, - misses, 0 means this lane sits out the current cycle). Each drop
// is one straight line: its lean is fixed when it starts, so gusts only change later
// drops, and every lane re-rolls its position each cycle. `lean` receives that slope.
// shelter is the mouse-reactive umbrella: x, y and radius (0 when there is none).
static inline simd_float4 del_rain_drop(unsigned lane,float time,simd_float2 extent,
    simd_float4 composer,simd_float4 footer,simd_float4 side,simd_float2 radii,simd_float3 shelter,float *lean) {
    float bottom=extent.y+40;
    float period=(bottom+60)/DEL_RAIN_MIN_SPEED+0.35f+del_weather_random(lane)*1.1f;
    float shifted=time+del_weather_random(lane+131)*period;
    uint32_t cycle=(uint32_t)floorf(shifted/period);
    float local=shifted-(float)cycle*period;
    uint32_t key=lane*977u+cycle*7919u;
    float start=time-local,slope=del_rain_slope(start);
    if(lean)*lean=slope;
    // Spread lanes along the top edge, reaching far enough upwind to cover the whole window.
    float upwind=slope*(bottom+60);
    float top=(lane+0.05f+0.9f*del_weather_random(key))/DEL_RAIN_COUNT*(extent.x+upwind)-upwind;
    simd_float2 land=del_rain_land(top,slope,bottom,composer,footer,side,radii,shelter);
    float speed=DEL_RAIN_MIN_SPEED+(DEL_RAIN_MAX_SPEED-DEL_RAIN_MIN_SPEED)*del_weather_random(key+73);
    float age=local-(land.y+60)/speed;
    if(del_weather_random(key+211)>del_rain_intensity(start))return (simd_float4){land.x,land.y,age,0};
    return (simd_float4){land.x,land.y,age,land.y<bottom?speed:-speed};
}
// Which bar a drop stopped on: 0 composer, 1 and 2 the two groups under it, -1 neither (the
// umbrella, or a miss). Those groups are separate: the space between them is open air.
static inline int del_rain_surface(simd_float4 drop,simd_float4 composer,simd_float4 footer,simd_float4 side,simd_float2 radii) {
    if(drop.w<=0)return -1;
    simd_float2 p={drop.x,drop.y};
    if(del_rain_bar(p,footer,radii.y)<0.05f)return 1;
    if(del_rain_bar(p,side,radii.y)<0.05f)return 2;
    if(del_rain_bar(p,composer,radii.x)<0.05f)return 0;
    return -1;
}
// One drop crosses from falling to landed exactly once, in the frame it lands.
// Its speed identifies the drop: the landing point can move under it with the umbrella.
static inline bool del_rain_impact(simd_float4 previous,simd_float4 current) {
    return current.w>0&&fabsf(previous.w)==current.w&&previous.z<0&&current.z>=0;
}

// ---------------------------------------------------------------------------
// Runoff: the water left behind by each landed drop. It beads on the bar's top edge,
// creeps outward, merges with beads it meets (bigger beads move faster), accelerates
// round the rounded corner, runs down the side and finally lets go and falls.
// ---------------------------------------------------------------------------
#define DEL_RUNOFF_COUNT 64
typedef struct { simd_float4 motion, shape; } DelRunoffDroplet; // x,y,vx,vy / radius,brightness
typedef struct {
    bool live, falling;
    int surface;
    float side, distance, speed, radius, age;
    simd_float2 position, velocity;
} DelRunoff;
typedef struct { simd_float2 point, tangent, normal; bool detached; } DelRunoffPath;
// Distance is measured along the bar's outline, starting from the middle of its top edge.
static inline DelRunoffPath del_runoff_path(float distance,float side,simd_float4 rect,float radius) {
    float r=fminf(radius,fminf(rect.z,rect.w)*0.5f),cx=rect.x+rect.z*0.5f;
    float flat=rect.z*0.5f-r,arc=(float)M_PI_2*r,wall=rect.w-2*r;
    DelRunoffPath p={.detached=false};
    if(distance<flat) {
        p.point=(simd_float2){cx+side*distance,rect.y};p.tangent=(simd_float2){side,0};p.normal=(simd_float2){0,-1};
    } else if(distance<flat+arc&&r>0) {
        float a=(distance-flat)/r;
        p.normal=(simd_float2){side*sinf(a),-cosf(a)};
        p.point=(simd_float2){cx+side*flat,rect.y+r}+p.normal*r;
        p.tangent=(simd_float2){side*cosf(a),sinf(a)};
    } else if(distance<flat+arc+wall) {
        p.point=(simd_float2){cx+side*rect.z*0.5f,rect.y+r+distance-flat-arc};
        p.tangent=(simd_float2){0,1};p.normal=(simd_float2){side,0};
    } else {
        // Under the lower corner, surface tension gives out partway round.
        float a=r>0?(distance-flat-arc-wall)/r:1;
        p.normal=(simd_float2){side*cosf(a),sinf(a)};
        p.point=(simd_float2){cx+side*flat,rect.y+rect.w-r}+p.normal*r;
        p.tangent=(simd_float2){-side*sinf(a),cosf(a)};
        p.detached=a>0.75f;
    }
    return p;
}
static inline void del_runoff_spawn(DelRunoff *pool,int surface,simd_float2 point,simd_float4 rect,float radius,float size) {
    float x=point.x,center=rect.x+rect.z*0.5f,side=x<center?-1:1;
    float r=fminf(radius,fminf(rect.z,rect.w)*0.5f),flat=rect.z*0.5f-r,wall=rect.w-2*r;
    float along=fabsf(x-center)-flat,below=point.y-(rect.y+r),distance;
    // Measure along the outline, whether it landed on the top, a corner or a side wall.
    if(along<=0)distance=fabsf(x-center);
    else if(below<0)distance=flat+atan2f(along,-below)*r;
    else if(below<wall)distance=flat+(float)M_PI_2*r+below;
    else distance=flat+(float)M_PI_2*r+wall+atan2f(below-wall,fmaxf(along,0.001f))*r;
    int slot=-1;float closest=1e9f;int nearest=-1;
    for(int i=0;i<DEL_RUNOFF_COUNT;i++) {
        if(!pool[i].live){if(slot<0)slot=i;continue;}
        if(pool[i].falling||pool[i].surface!=surface||pool[i].side!=side)continue;
        float gap=fabsf(pool[i].distance-distance);
        if(gap<closest){closest=gap;nearest=i;}
    }
    if(slot<0) { // A full bar: the water joins the nearest bead rather than vanishing.
        if(nearest>=0)pool[nearest].radius=fminf(sqrtf(pool[nearest].radius*pool[nearest].radius+size*size),3.6f);
        return;
    }
    pool[slot]=(DelRunoff){.live=true,.falling=false,.surface=surface,.side=side,.distance=distance,.speed=8,.radius=size,.age=0};
}
static inline void del_runoff_step(DelRunoff *pool,float dt,simd_float4 rects[3],simd_float2 radii,float bottom,DelRunoffDroplet *out) {
    for(int i=0;i<DEL_RUNOFF_COUNT;i++) {
        DelRunoff *d=&pool[i];
        out[i].shape=(simd_float4){0,0,0,0};
        if(!d->live)continue;
        d->age+=dt;
        simd_float4 rect=rects[d->surface];
        if(!d->falling&&(rect.z<=0||rect.w<=0)){d->live=false;continue;}
        if(d->falling) {
            d->velocity.y+=1700*dt;
            d->position+=d->velocity*dt;
            if(d->position.y>bottom)d->live=false;
        } else {
            float r=d->surface==0?radii.x:radii.y;
            DelRunoffPath p=del_runoff_path(d->distance,d->side,rect,r);
            // Gravity acts along the outline, plus a small creep along the flat top.
            float drive=p.tangent.y*1000+(p.tangent.y<0.05f?(16+9*d->radius*d->radius):0);
            float drag=p.tangent.y<0.05f?0.85f:3.0f;
            d->speed=fmaxf(d->speed+(drive-drag*d->speed)*dt,4);
            d->distance+=d->speed*dt;
            p=del_runoff_path(d->distance,d->side,rect,r);
            d->position=p.point+p.normal*d->radius*0.75f;
            d->velocity=p.tangent*d->speed;
            if(p.detached)d->falling=true;
        }
        out[i].motion=(simd_float4){d->position.x,d->position.y,d->velocity.x,d->velocity.y};
        out[i].shape=(simd_float4){d->radius,fminf(d->age*9,1)*0.95f,0,0};
    }
    // Beads that touch become one bigger bead, keeping the combined water.
    for(int i=0;i<DEL_RUNOFF_COUNT;i++) {
        if(!pool[i].live||pool[i].falling)continue;
        for(int j=i+1;j<DEL_RUNOFF_COUNT;j++) {
            if(!pool[j].live||pool[j].falling||pool[j].surface!=pool[i].surface||pool[j].side!=pool[i].side)continue;
            if(fabsf(pool[i].distance-pool[j].distance)<pool[i].radius+pool[j].radius) {
                pool[i].radius=fminf(sqrtf(pool[i].radius*pool[i].radius+pool[j].radius*pool[j].radius),3.6f);
                pool[i].distance=fmaxf(pool[i].distance,pool[j].distance);
                pool[i].speed=fmaxf(pool[i].speed,pool[j].speed);
                pool[j].live=false;
            }
        }
    }
}
