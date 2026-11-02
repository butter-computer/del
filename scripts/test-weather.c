// clang scripts/test-weather.c -o /tmp/del-test-weather && /tmp/del-test-weather
#include "../crates/app/src/weather.h"
#include <assert.h>
#include <stdio.h>
#include <string.h>
int main(void) {
    simd_float2 extent={800,600},radii={14,8};
    simd_float4 composer={120,300,560,150},footer={180,490,180,32},side={500,490,180,32};
    simd_float3 none={0,0,0};
    // Distance to the rounded bar: on its edge, outside the corner, inside, and no bar at all.
    assert(fabsf(del_rain_bar((simd_float2){400,300},composer,14))<0.001f);
    assert(fabsf(del_rain_bar((simd_float2){110,375},composer,14)-10)<0.001f);
    assert(del_rain_bar((simd_float2){400,375},composer,14)<0);
    assert(del_rain_bar((simd_float2){400,375},(simd_float4){0},14)>1e8f);
    assert(del_rain_bar((simd_float2){120,300},composer,14)>0); // The corner is rounded away.
    // The groups under the bar are separate: rain falls through the space between them.
    simd_float4 nothing={0};
    simd_float2 inGap=del_rain_land(430-0.4f*506,0.4f,640,nothing,footer,side,radii,none);
    assert(inGap.y==640);
    simd_float2 onLeft=del_rain_land(260-0.4f*490,0.4f,640,nothing,footer,side,radii,none);
    simd_float2 onRight=del_rain_land(600-0.4f*490,0.4f,640,nothing,footer,side,radii,none);
    assert(onLeft.y<500&&onRight.y<500);
    assert(del_rain_surface((simd_float4){onLeft.x,onLeft.y,0,1},nothing,footer,side,radii)==1);
    assert(del_rain_surface((simd_float4){onRight.x,onRight.y,0,1},nothing,footer,side,radii)==2);
    // Rain leans: every drop is a clearly slanted, straight line.
    for(float t=0;t<200;t+=0.37f)assert(del_rain_slope(t)>0.3f&&del_rain_slope(t)<0.5f);
    // A slanted drop aimed past the bar's right end meets nothing, and one aimed at it from
    // upwind strikes its left wall, not a point underneath it.
    simd_float2 wall=del_rain_land(120-0.4f*375,0.4f,640,composer,(simd_float4){0},(simd_float4){0},radii,none);
    assert(fabsf(wall.x-120)<0.1f&&fabsf(wall.y-375)<0.1f);
    simd_float2 past=del_rain_land(700,0.4f,640,composer,(simd_float4){0},(simd_float4){0},radii,none);
    assert(past.y==640);
    // A slanted drop never ends inside or passes through the bar.
    for(float top=-300;top<900;top+=3.7f) {
        simd_float2 land=del_rain_land(top,0.4f,640,composer,footer,side,radii,none);
        assert(del_rain_bar(land,composer,14)>-0.05f);
        for(float y=-60;y<land.y;y+=2)assert(del_rain_bar((simd_float2){top+0.4f*y,y},composer,14)>-0.05f);
    }
    unsigned impacts=0,misses=0,resting=0,columns=0;float seen=-1;
    for(unsigned lane=0;lane<DEL_RAIN_COUNT;lane++) {
        simd_float4 previous=del_rain_drop(lane,0,extent,composer,footer,side,radii,none,NULL);
        for(unsigned frame=1;frame<=3600;frame++) {
            simd_float4 drop=del_rain_drop(lane,frame/120.0f,extent,composer,footer,side,radii,none,NULL);
            assert(isfinite(drop.x)&&isfinite(drop.y)&&isfinite(drop.z)&&isfinite(drop.w));
            assert(drop.x>=-40&&drop.x<extent.x+1+0.5f*(extent.y+100)); // Only a miss ends outside the window.
            if(lane==0&&drop.x!=seen){seen=drop.x;columns++;} // Drops don't reuse a column.
            assert(!del_rain_impact(drop,drop)); // Paused frames cannot retrigger.
            if(del_rain_impact(previous,drop)) {
                impacts++;
                // Every impact is on the outside of a bar: its top, a corner or the upwind wall.
                simd_float2 at={drop.x,drop.y};
                assert(fminf(fabsf(del_rain_bar(at,composer,14)),fminf(fabsf(del_rain_bar(at,footer,8)),fabsf(del_rain_bar(at,side,8))))<0.1f);
                assert(del_rain_surface(drop,composer,footer,side,radii)>=0);
                assert(drop.z<1.0f/120.0f+0.00001f);
            }
            if(drop.w<0)misses++;
            if(drop.w==0)resting++;
            previous=drop;
        }
        simd_float4 empty=del_rain_drop(lane,10,extent,(simd_float4){0},(simd_float4){0},(simd_float4){0},radii,none,NULL);
        assert(empty.w<=0); // No input bar means no tactile impacts.
    }
    assert(impacts>100&&misses>0&&resting>0);
    assert(columns>3);
    // Every impact is reported exactly once, however the frames fall.
    for(unsigned lane=0;lane<DEL_RAIN_COUNT;lane++) {
        unsigned fine=0,coarse=0;
        simd_float4 a=del_rain_drop(lane,0,extent,composer,footer,side,radii,none,NULL),b=a;
        for(unsigned frame=1;frame<=7200;frame++) {
            simd_float4 d=del_rain_drop(lane,frame/240.0f,extent,composer,footer,side,radii,none,NULL);
            if(del_rain_impact(a,d))fine++;
            a=d;
            if(frame%4==0){if(del_rain_impact(b,d))coarse++;b=d;}
        }
        assert(fine==coarse);
    }
    // The umbrella: drops over the cursor stop on its dome, and that is not a bar impact.
    simd_float3 umbrella={400,200,64};
    unsigned sheltered=0;
    for(unsigned lane=0;lane<DEL_RAIN_COUNT;lane++) {
        simd_float4 d=del_rain_drop(lane,3,extent,composer,footer,side,radii,umbrella,NULL);
        // Nothing passes through the dome; anything that stops on it is not a bar impact.
        if(d.w!=0&&simd_distance((simd_float2){d.x,d.y},(simd_float2){400,200})<64.5f) {
            sheltered++;
            assert(d.y<=264.5f);
            assert(del_rain_surface(d,composer,footer,side,radii)==-1);
        }
    }
    assert(sheltered>0);
    // The outline is one continuous path, with a smooth tangent at every join.
    for(float side=-1;side<=1;side+=2) {
        DelRunoffPath previous=del_runoff_path(0,side,composer,14);
        assert(previous.point.y==composer.y);
        bool detached=false;
        for(float s=0.25f;s<1200&&!detached;s+=0.25f) {
            DelRunoffPath p=del_runoff_path(s,side,composer,14);
            assert(simd_distance(p.point,previous.point)<0.5f);
            assert(simd_distance(p.tangent,previous.tangent)<0.1f);
            // The bead stays outside the bar's rounded rectangle.
            simd_float2 out=p.point+p.normal*2.0f;
            bool inside=out.x>composer.x+14&&out.x<composer.x+composer.z-14&&out.y>composer.y&&out.y<composer.y+composer.w;
            assert(!inside);
            detached=p.detached;
            previous=p;
        }
        assert(detached);
    }
    // A landed drop runs along the edge, round the corner, down the side, then lets go.
    static DelRunoff pool[DEL_RUNOFF_COUNT];static DelRunoffDroplet out[DEL_RUNOFF_COUNT];
    simd_float4 rects[3]={composer,footer,side};
    del_runoff_spawn(pool,0,(simd_float2){600,composer.y},composer,14,1.5f);
    float startX=600,furthestY=0;bool fell=false;int frames=0;
    for(;frames<120*60&&pool[0].live;frames++) {
        del_runoff_step(pool,1.0f/120,rects,radii,600,out);
        assert(out[0].motion.x>=startX-2); // Always outward, towards the nearer end.
        furthestY=fmaxf(furthestY,out[0].motion.y);
        if(pool[0].falling)fell=true;
    }
    assert(fell&&!pool[0].live&&furthestY>=composer.y+composer.w);
    // Two beads that meet become one larger bead; a full bar never loses water.
    memset(pool,0,sizeof(pool));
    del_runoff_spawn(pool,0,(simd_float2){500,composer.y},composer,14,1.5f);del_runoff_spawn(pool,0,(simd_float2){501,composer.y},composer,14,1.5f);
    del_runoff_step(pool,1.0f/120,rects,radii,600,out);
    int alive=0;float largest=0;
    for(int i=0;i<DEL_RUNOFF_COUNT;i++)if(pool[i].live){alive++;largest=fmaxf(largest,pool[i].radius);}
    assert(alive==1&&largest>2.0f);
    // A drop that strikes the side wall starts its run from there, not back on the top edge.
    memset(pool,0,sizeof(pool));
    del_runoff_spawn(pool,0,(simd_float2){composer.x,composer.y+70},composer,14,1.5f);
    del_runoff_step(pool,1.0f/120,rects,radii,600,out);
    assert(fabsf(pool[0].position.x-composer.x)<4&&fabsf(pool[0].position.y-(composer.y+70))<4);
    printf("%u rain impacts: rounded collision geometry, timing, misses and paused frames passed.\n",impacts);
}
