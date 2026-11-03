// Window Curtains panels the user can take hold of. Plain geometry with no GPU, so the rules
// can be tested alone. Keep the home layout equal to clothRig in fluid.metal.
//
// Each panel hangs from hooks on a rail. `left` and `right` are the end hooks, and `hook` is
// the one the user last took hold of, `at` of the way along the panel (0 to 1). Hooks can
// bunch up or spread apart between those three, within limits, as real curtain hooks do.
// All x positions here are logical pixels; fluid.m stores them as fractions of the width.
#pragma once
#include <math.h>
#include <stdbool.h>
#include <simd/simd.h>
#define DEL_CURTAIN_PANELS 3
#define DEL_CURTAIN_PANEL_COLUMNS 120
#define DEL_CURTAIN_ROWS 180
#define DEL_CURTAIN_RAIL_Y (-6.0f)
// How close hooks can bunch, and how far they can spread, relative to where they hang at rest.
#define DEL_CURTAIN_BUNCH 0.46f
#define DEL_CURTAIN_SPREAD 1.12f
// How far past its resting ends each panel's rail travel reaches, as a share of its width.
#define DEL_CURTAIN_TRAVEL 0.22f
typedef struct { float left, hook, right, at; } DelCurtainRail;
typedef struct { float left, right; } DelCurtainHome;

static inline float del_curtain_clamp(float x,float low,float high) { return fminf(fmaxf(x,low),high); }
static inline DelCurtainHome del_curtain_home(int panel,float width) {
    float margin=width*0.012f,gap=width*0.105f;
    float span=(width-2*margin-2*gap)/DEL_CURTAIN_PANELS;
    float left=margin+panel*(span+gap);
    return (DelCurtainHome){left,left+span};
}
static inline DelCurtainRail del_curtain_rest(int panel,float width) {
    DelCurtainHome home=del_curtain_home(panel,width);
    return (DelCurtainRail){home.left,(home.left+home.right)*0.5f,home.right,0.5f};
}
// Where the hook a share `s` along the panel currently sits.
static inline float del_curtain_hook_x(DelCurtainRail rail,float s) {
    if(s<=rail.at)return rail.left+(rail.hook-rail.left)*s/fmaxf(rail.at,0.001f);
    return rail.hook+(rail.right-rail.hook)*(s-rail.at)/fmaxf(1-rail.at,0.001f);
}
// Move the held hook, dragging or bunching the rest, and stop at this panel's limits.
static inline DelCurtainRail del_curtain_move(DelCurtainRail rail,float wanted,DelCurtainHome home) {
    float span=home.right-home.left,low=home.left-span*DEL_CURTAIN_TRAVEL,high=home.right+span*DEL_CURTAIN_TRAVEL;
    float leftSpan=rail.at*span,rightSpan=(1-rail.at)*span;
    float hook=del_curtain_clamp(wanted,low+leftSpan*DEL_CURTAIN_BUNCH,high-rightSpan*DEL_CURTAIN_BUNCH);
    rail.hook=hook;
    rail.left=fmaxf(del_curtain_clamp(rail.left,hook-leftSpan*DEL_CURTAIN_SPREAD,hook-leftSpan*DEL_CURTAIN_BUNCH),low);
    rail.right=fminf(del_curtain_clamp(rail.right,hook+rightSpan*DEL_CURTAIN_BUNCH,hook+rightSpan*DEL_CURTAIN_SPREAD),high);
    return rail;
}
// Fabric hanging `drop` below its hook has this much sideways play before it pulls the hook.
// A hold high up has almost none, so it slides the rail at once. Lifting a low hold gives more,
// since the fabric is no longer hanging straight down, but only up to what folds can absorb.
static inline float del_curtain_play(float drop,float below) {
    float taut=sqrtf(fmaxf(drop*drop-below*below,0));
    float slack=12.0f+drop*0.20f;
    return fminf(fmaxf(taut,slack),50.0f+drop*0.30f);
}
// One frame of a held curtain. `target` is where the cursor wants this point of fabric, `drop`
// how far down the fabric it is, and `natural` where it was hanging when taken hold of: its x
// relative to its hook, and its y. Limits count from there, so simply taking hold never moves
// anything. The rail moves only once the fabric pulls tight, immediately, without a speed cap or easing. Returns where the fabric may go.
static inline simd_float2 del_curtain_hold(DelCurtainRail *rail,simd_float2 target,float drop,simd_float2 natural,DelCurtainHome home) {
    float top=DEL_CURTAIN_RAIL_Y;
    simd_float2 held=target;
    held.y=del_curtain_clamp(held.y,fminf(top+drop*0.50f,natural.y),fmaxf(top+drop,natural.y));
    float play=del_curtain_play(drop,held.y-top);
    float away=held.x-natural.x; // Where this hook would be, if the fabric hung as it did.
    float wanted=rail->hook;
    if(away>rail->hook+play)wanted=away-play;
    if(away<rail->hook-play)wanted=away+play;
    *rail=del_curtain_move(*rail,wanted,home);
    held.x=natural.x+del_curtain_clamp(away,rail->hook-play,rail->hook+play);
    return held;
}
// Letting go. Stored stretch must not be released all at once, or it whips through the panel
// as a ripple. The grip eases off while the held point sinks to where it would hang, and air
// slows the panel as it settles, which is why sheer fabric floats down rather than snapping.
#define DEL_CURTAIN_GRIP_SECONDS 1.2f
#define DEL_CURTAIN_SETTLE_SECONDS 6.0f
static inline float del_curtain_grip(float age) {
    float left=fmaxf(1-age/DEL_CURTAIN_GRIP_SECONDS,0);
    return left*left; // The solver still holds firmly until the grip is small.
}
static inline float del_curtain_settle(float age) {
    // A steady fade: if the extra air ended at any noticeable moment, the fabric would visibly
    // wake up then, seconds after being let go.
    return fmaxf(1-age/DEL_CURTAIN_SETTLE_SECONDS,0);
}
static inline simd_float3 del_curtain_sink(simd_float3 point,simd_float3 hanging,float age,float dt) {
    float ramp=fminf(age/0.25f,1); // A hand opening, not a sudden drop.
    return point+(hanging-point)*(1-expf(-dt*4.2f*ramp));
}
// The passing hand, the hover response, stands down while a curtain is held. It must not come
// back by itself where the cursor happens to rest, or a dent appears there out of nowhere. It
// waits until the cursor has actually moved, then responds on that frame.
static inline float del_curtain_hand(float hand,bool holding,bool moved,float dt) {
    return holding||!moved?fmaxf(hand-dt*8.0f,0):1.0f;
}
// Clamp force, never cursor position: fast pointer motion stays responsive without whipping cloth.
static inline simd_float2 del_curtain_limit_vector(simd_float2 value,float limit) {
    float speed=simd_length(value);
    return speed>limit?value*(limit/speed):value;
}
// The piece of fabric under the cursor, or false over a gap. Particles are RGBA floats as the
// cloth solver stores them. Layers can overlap, so the nearest one to the viewer wins ties.
static inline bool del_curtain_pick(const float *cloth,int columns,int rows,simd_float2 at,int *column,int *row) {
    const float reach=26.0f;
    float best=1e9f;bool found=false;
    for(int r=3;r<rows;r++)for(int c=0;c<columns;c++) {
        const float *p=cloth+(r*columns+c)*4;
        float dx=p[0]+p[2]*0.10f-at.x,dy=p[1]-at.y;
        float distance=sqrtf(dx*dx+dy*dy);
        if(distance>=reach)continue;
        float score=distance-p[2]*0.08f;
        if(score<best){best=score;*column=c;*row=r;found=true;}
    }
    return found;
}
