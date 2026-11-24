//! WDK build configuration.
//!
//! `configure_wdk_binary_build` emits the linker arguments a KMDF driver needs
//! (`/DRIVER /SUBSYSTEM:NATIVE /ENTRY:FxDriverEntry`, `ntoskrnl`, `WdfDriverEntry`,
//! ...) and locates the WDK content root.  It is the only build-script logic a
//! driver needs.
fn main() -> Result<(), wdk_build::ConfigError> {
    wdk_build::configure_wdk_binary_build()
}
