//! Native feedback for discrete, user-driven control changes.
pub fn tick() {
    #[cfg(all(target_os = "macos", not(test)))]
    {
        unsafe extern "C" { fn del_selection_haptic(); }
        // Called by GPUI input handlers on the main thread. Unsupported hardware is silent.
        unsafe { del_selection_haptic(); }
    }
}
