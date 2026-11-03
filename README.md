<p align="center">
  <img src="assets/logo.png" alt="FAH Logo" width="220" />
</p>

<h1 align="center">FAH</h1>

<p align="center">
  <strong>Userspace Intel xHCI USB Host Controller Driver in Rust via Linux VFIO</strong>
</p>

---

## 1. Overview

**FAH** is a research project for a Rust userspace USB host-controller driver on Linux. It studies how much USB control an existing Intel xHCI controller can provide through VFIO.

Linux normally manages these controllers through `xhci_hcd` and the USB core. This stack discovers devices and sends initialization requests. Some experiments require different request sequences or incomplete transfers.

The proposed design gives FAH exclusive access to a physical controller through `vfio-pci`. FAH would manage its registers, DMA memory, interrupts, and transfer queues. The existing controller hardware remains unchanged.

The research asks which operations can match a Raspberry Pi Pico USB host implementation. Equivalent connectors do not establish equivalent controller capabilities. Arbitrary packet generation, precise timing, and full Pico equivalence remain unverified.

### iPad mini 1: a case study

The Apple A5 iPad mini 1 is a study case, not the project's sole purpose. Its `checkm8-a5` boot process provides concrete USB requirements to compare with Intel xHCI capabilities. Project Cascadia provides the context for exploring native Linux on this device.

FAH also targets broader USB protocol research and controller diagnostics. Successful iPad exploitation or Linux boot is not an established project capability. Matching one case study would not prove full Pico equivalence.

### Current status

The Rust driver, explicit USB operations, persistent CLI and conservative shutdown path are implemented and fake-tested. Guest topology and synthetic-fixture checks pass; integrated FAH transfer validation is in progress. Physical Intel and iPad case-study behavior remain unverified.

Prepare pinned sources with `python3 scripts/prepare-dependencies.py`. Run `cargo test --locked --features hosted-direct` and build with `cargo build --locked --release --features hosted-direct`. The executable currently accepts only the marked emulated guest; host controller binding is unavailable.

---

## 2. Key Objectives

* **Capability Mapping**: Compare Pico operations with documented xHCI features. Separate hardware support, driver restrictions, and unverified behavior.
* **Exclusive Controller Ownership**: Study controller handoff from `xhci_hcd` to `vfio-pci`. Prevent Linux's normal USB enumeration on the selected controller.
* **Direct Register Access**: Map the controller's PCI BAR regions into userspace through VFIO.
* **IOMMU DMA Allocator**: Allocate buffers and map device-visible addresses through VFIO. Manage buffer alignment, access, and lifetime.
* **Interrupt Handling**: Connect supported interrupt modes to Linux `eventfd`. Check MSI-X, MSI, and INTx capabilities on the target controller.
* **Transaction Experiments**: Investigate custom SETUP sequences, incomplete transfers, and controlled resets. Measure timing instead of assuming microsecond guarantees.
* **Rust Implementation**: Reuse existing libraries where suitable. Keep unsafe hardware access explicit and document its requirements.

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
* **Toolchain**: Rust (`cargo`, `rustc`). The minimum version depends on the selected dependencies and remains to be established.

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


## Emulated guest lab

On clean `release/v0.1.0`, use `python3 scripts/lab.py plan`, then
`python3 scripts/lab.py prepare` and `python3 scripts/lab.py run --suite fixture`.
The launcher requires KVM, QEMU 10.0's `pc-q35-10.0`, and the exact Debian kernel/config
recorded in `scripts/lab/kernel.json`. It copies the installed kernel and selected
modules without changing the host boot configuration. Build the hosted release
binary first; no package installation or host driver binding is performed.

The guest has 2 GiB RAM, 2 vCPUs, a serial console, a virtual Intel IOMMU, an
emulated xHCI, keyboard and USB storage. Each run creates an 8 MiB synthetic disk.
The firmware marker, emulated PCI IDs and singleton guest IOMMU group are checked
before any guest ownership change. No host block/USB/PCI device is forwarded.

Artifacts persist in `~/.cache/fah-lab/runs/<run>`: serial output, commands,
source/image fingerprints, VFIO metadata, QMP PCI inventory, emulator trace and
results. Run `python3 scripts/lab.py stop --run <run>` to stop that generated QEMU;
`python3 scripts/lab.py cleanup --run <run>` removes its stopped disposable run.
Cleanup rejects unmarked directories and unrelated/reused process IDs.

The `fixture` suite restores the guest Linux xHCI driver and verifies the synthetic
sector through `/dev/sda` **inside the guest**. This validates the lab fixtures;
it does not substitute for FAH's separate transfer suites or physical validation.
