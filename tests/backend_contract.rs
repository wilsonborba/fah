#![cfg(feature = "hosted-direct")]

// Compile-only fixture. Calling this function would read MMIO registers.
#[allow(dead_code)]
fn actual_constructor_bound(
    registers: crab_usb::Mmio,
    kernel: &'static dyn crab_usb::KernelOp,
) -> crab_usb::err::Result<crab_usb::USBHost> {
    crab_usb::USBHost::new_xhci(registers, kernel)
}
