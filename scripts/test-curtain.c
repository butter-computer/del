// clang scripts/test-curtain.c -o /tmp/del-test-curtain && /tmp/del-test-curtain
#include "../crates/app/src/curtain.h"
#include <assert.h>
#include <stdio.h>
#include <stdlib.h>
static const float W=1100;
static void check_limits(DelCurtainRail r,DelCurtainHome h) {
    float span=h.right-h.left;
    assert(r.left>=h.left-span*DEL_CURTAIN_TRAVEL-0.01f);
    assert(r.right<=h.right+span*DEL_CURTAIN_TRAVEL+0.01f);
    assert(r.left<r.hook&&r.hook<r.right);
    assert(r.hook-r.left>=r.at*span*DEL_CURTAIN_BUNCH-0.01f);
    assert(r.right-r.hook>=(1-r.at)*span*DEL_CURTAIN_BUNCH-0.01f);
    assert(r.hook-r.left<=r.at*span*DEL_CURTAIN_SPREAD+0.01f);
    assert(r.right-r.hook<=(1-r.at)*span*DEL_CURTAIN_SPREAD+0.01f);
}
int main(void) {
    // At rest every hook hangs exactly where it always did, and the panels don't touch.
    for(int p=0;p<DEL_CURTAIN_PANELS;p++) {
        DelCurtainHome h=del_curtain_home(p,W);
        DelCurtainRail r=del_curtain_rest(p,W);
        for(float s=0;s<=1;s+=0.01f)assert(fabsf(del_curtain_hook_x(r,s)-(h.left+s*(h.right-h.left)))<0.01f);
        if(p>0)assert(h.left>del_curtain_home(p-1,W).right);
    }
    assert(fabsf(del_curtain_home(2,W).right-(W-W*0.012f))<0.01f);
    DelCurtainHome home=del_curtain_home(1,W);
    float span=home.right-home.left,mid=(home.left+home.right)*0.5f;
    // Dragging the middle hook bunches what's ahead of it and spreads what's behind it.
    DelCurtainRail r=del_curtain_move(del_curtain_rest(1,W),mid+30,home);
    assert(r.hook==mid+30&&r.right==home.right&&r.left>home.left);
    check_limits(r,home);
    // However far it is dragged, in any order, a panel stays inside its own limits.
    for(float at=0.02f;at<0.98f;at+=0.07f) { // Holds clamp to this range.
        r=del_curtain_rest(1,W);r.at=at;r.hook=del_curtain_hook_x(del_curtain_rest(1,W),at);
        for(int i=0;i<4000;i++) {
            r=del_curtain_move(r,(float)(rand()%4000)-1500,home);
            check_limits(r,home);
        }
        r=del_curtain_move(r,99999,home);
        assert(fabsf(r.right-(home.right+span*DEL_CURTAIN_TRAVEL))<0.01f);
        r=del_curtain_move(r,-99999,home);
        assert(fabsf(r.left-(home.left-span*DEL_CURTAIN_TRAVEL))<0.01f);
    }
    // Held high up the rail slides at once. Held low the fabric swings first and the rail stays
    // put until the fabric is pulled tight, and then it follows.
    float restV=(760.0f+70.0f)/(DEL_CURTAIN_ROWS-1);
    float top=8*restV,low=170*restV;
    DelCurtainRail high=del_curtain_rest(1,W),under=del_curtain_rest(1,W);
    simd_float2 heldHigh={mid+60,DEL_CURTAIN_RAIL_Y+top},heldLow={mid+60,DEL_CURTAIN_RAIL_Y+low};
    simd_float2 a=heldHigh,b=heldLow;
    for(int i=0;i<120;i++) {
        a=del_curtain_hold(&high,heldHigh,top,(simd_float2){0,DEL_CURTAIN_RAIL_Y+top},1/120.0f,home);
        b=del_curtain_hold(&under,heldLow,low,(simd_float2){0,DEL_CURTAIN_RAIL_Y+low},1/120.0f,home);
    }
    assert(high.hook>mid+30);          // Near the rail, the hook went with the hand.
    assert(fabsf(under.hook-mid)<0.5f); // Low down, it hasn't moved at all.
    assert(fabsf(b.x-(mid+60))<0.5f);   // The fabric itself has reached the hand.
    heldLow.x=mid+500;
    for(int i=0;i<240;i++)b=del_curtain_hold(&under,heldLow,low,(simd_float2){0,DEL_CURTAIN_RAIL_Y+low},1/120.0f,home);
    assert(under.hook>mid+40);          // Pulled tight, the rail comes along.
    // Whatever the cursor does: fabric stops at its limit and can never rise past half way.
    DelCurtainRail wild=del_curtain_rest(1,W);
    for(int i=0;i<6000;i++) {
        simd_float2 t={(float)(rand()%6000)-2500,(float)(rand()%4000)-2000};
        simd_float2 h=del_curtain_hold(&wild,t,low,(simd_float2){0,DEL_CURTAIN_RAIL_Y+low},1/120.0f,home);
        check_limits(wild,home);
        assert(h.y>=DEL_CURTAIN_RAIL_Y+low*0.5f-0.01f&&h.y<=DEL_CURTAIN_RAIL_Y+low+0.01f);
        assert(fabsf(h.x-wild.hook)<=50+low*0.30f+0.01f);
    }
    // Rail speed is limited, so a flick can't snap the hooks across.
    DelCurtainRail slow=del_curtain_rest(1,W);
    del_curtain_hold(&slow,(simd_float2){mid+800,DEL_CURTAIN_RAIL_Y+top},top,(simd_float2){0,DEL_CURTAIN_RAIL_Y+top},1/60.0f,home);
    assert(slow.hook-mid<=DEL_CURTAIN_RAIL_SPEED/60+0.01f);
    // Taking hold of fabric hanging well away from its hook, or below where it was expected,
    // moves nothing at all: neither the rail nor the fabric.
    for(float offset=-160;offset<=160;offset+=40)for(float sag=0;sag<=60;sag+=30) {
        DelCurtainRail still=del_curtain_rest(1,W);
        simd_float2 natural={offset,DEL_CURTAIN_RAIL_Y+low+sag};
        simd_float2 hanging={still.hook+offset,natural.y};
        simd_float2 kept=del_curtain_hold(&still,hanging,low,natural,1/120.0f,home);
        assert(still.hook==mid&&still.left==home.left&&still.right==home.right);
        assert(fabsf(kept.x-hanging.x)<0.001f&&fabsf(kept.y-hanging.y)<0.001f);
    }
    // The hand closes gradually, from nothing, with no step at either end.
    assert(del_curtain_grip_in(0)==0&&del_curtain_grip_in(DEL_CURTAIN_GRIP_IN_SECONDS)==1);
    for(float g=0;g<DEL_CURTAIN_GRIP_IN_SECONDS;g+=1/120.0f)assert(del_curtain_grip_in(g+1/120.0f)-del_curtain_grip_in(g)<0.05f);
    // After a hold, the hover hand stays away from a resting cursor however long it waits.
    float hand=1;
    for(int i=0;i<60;i++)hand=del_curtain_hand(hand,true,true,1/120.0f);
    assert(hand==0);
    for(int i=0;i<2400;i++)hand=del_curtain_hand(hand,false,false,1/120.0f);
    assert(hand==0);
    // Once the cursor moves it eases in over more than half a second, never all at once.
    int frames=0;
    while(hand<1){float before=hand;hand=del_curtain_hand(hand,false,true,1/120.0f);assert(hand-before<0.02f);frames++;}
    assert(frames>=70);
    // Picking: the fabric under the cursor; nothing over a gap; nearest layer in an overlap.
    enum{C=4,R=8};
    static float cloth[C*R*4];
    for(int r2=0;r2<R;r2++)for(int c=0;c<C;c++){float *p=cloth+(r2*C+c)*4;p[0]=100+c*10;p[1]=r2*10;p[2]=0;p[3]=1;}
    int col=-1,row=-1;
    assert(del_curtain_pick(cloth,C,R,(simd_float2){121,51},&col,&row)&&col==2&&row==5);
    assert(!del_curtain_pick(cloth,C,R,(simd_float2){500,50},&col,&row));
    assert(!del_curtain_pick(cloth,C,R,(simd_float2){120,-40},&col,&row)); // Pinned top rows.
    float *front=cloth+(6*C+3)*4;front[0]=121;front[1]=51;front[2]=60;
    assert(del_curtain_pick(cloth,C,R,(simd_float2){127,51},&col,&row)&&col==3&&row==6);
    puts("Curtain rails: rest layout, per-panel limits, hold height, speed and picking passed.");
}
