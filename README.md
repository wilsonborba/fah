<p align="center">
  <img src="assets/logo.png" alt="FAH Logo" width="220" />
</p>

<h1 align="center">FAH</h1>

<p align="center">
  <strong>Userspace Intel xHCI USB Host Controller Driver in Rust via Linux VFIO</strong>
</p>

---

## 1. Overview

Low-level hardware research, bootROM diagnostics, and custom exploit delivery (such as `checkm8` for legacy Apple silicon) often require dedicated microcontrollers like a Raspberry Pi Pico or an Arduino with a USB Host Shield. 

This requirement exists because standard desktop operating systems (Linux, macOS, Windows) manage USB controllers through default kernel drivers (`xhci_hcd`). Operating system drivers automatically send device discovery packets, assign bus addresses, and initiate standard enumeration sequences. These automatic requests break fragile hardware states during low-level recovery procedures.

**FAH** is an open-source userspace driver written in Rust. It unbinds a physical Intel xHCI host controller from the Linux kernel and takes exclusive hardware ownership using Linux VFIO (`vfio-pci`). 

By controlling the physical xHCI controller directly from userspace, FAH provides the same low-level USB packet control as an external microcontroller: directly from an Intel PC, without extra hardware.

---

## 2. Key Objectives

* **Pico Equivalence**: Provide microsecond transaction control, raw packet crafting, and custom setup packet sequencing directly on Intel PC hardware.
* **Exclusive Controller Ownership**: Disconnect Linux kernel drivers from the target controller to prevent automatic enumeration and unwanted zero-length packets.
* **Direct Register Access**: Memory-map xHCI operational and runtime registers directly into userspace memory via VFIO BAR regions.
* **IOMMU DMA Allocator**: Allocate physical DMA buffers and manage command/transfer rings with hardware safety through the IOMMU.
* **Pure Rust Implementation**: Build a safe, maintainable driver leveraging modern Rust systems programming.

---

## 3. Architecture Roadmap

FAH separates low-level hardware interaction from user command parsing:

* **Core**: Common error definitions, logging, and hardware constants.
* **Data Access Layer (DAL)**: Linux VFIO ioctl bindings, PCI BAR mapping, IOMMU DMA allocator, and MSI-X interrupt event handling.
* **Domain**: xHCI specification implementation, command and transfer ring buffers, TRB packet builders, and USB descriptor decoders.
* **Presentation (CLI)**: Command-line interface providing commands to inspect hardware, bind controllers, and issue raw USB sequences.

---

## 4. Prerequisites

* **Hardware**: Intel PC with VT-d / IOMMU enabled in BIOS/UEFI.
* **Operating System**: Linux kernel with VFIO support enabled (`vfio`, `vfio_iommu_type1`, `vfio_pci`).
* **Toolchain**: Rust 1.80+ (`cargo`, `rustc`).

---

## 5. Prior Art & References

* [Project Cascadia](https://github.com/rzxvx/Project-Cascadia)
* [checkm8-a5](https://github.com/a1exdandy/checkm8-a5)
* [drivercraft/CrabUSB](https://github.com/drivercraft/CrabUSB)
* [rust-vmm/vfio](https://github.com/rust-vmm/vfio)
* [rust-osdev/xhci](https://github.com/rust-osdev/xhci)

---

## 6. License

Dual-licensed under either Apache License, Version 2.0 or MIT license at your option.
