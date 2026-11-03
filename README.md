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


## Capability and evidence inventory

<!-- FAH_CAPABILITIES_BEGIN -->
| Operation | CLI operations | Availability | Evidence | Coverage limits |
| --- | --- | --- | --- | --- |
| [metadata_queries](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/presentation/cli/handlers.rs) | `help`, `version`, `capabilities` | implemented | compiled, fake_tested | describe syntax/version/capabilities without controller acquisition |
| [session_state](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/presentation/cli/handlers.rs) | `state` | implemented | compiled, fake_tested, emulated_tested | observe session identity, ownership state and retained allocations |
| [cached_inventory](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/presentation/cli/handlers.rs) | `devices`, `requests` | implemented | compiled, fake_tested | cached results are not fresh device-presence evidence |
| [guest_acquire](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/dal/local/resources.rs) | `open` | implemented | compiled, fake_tested, emulated_tested | physical Intel acquisition intentionally unavailable |
| [controller_init](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/dal/local/controller.rs) | `init` | implemented | compiled, fake_tested, emulated_tested | initialization sends no discovery/descriptors/configuration |
| [coordinated_close](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/dal/local/teardown.rs) | `close`, `quit` | implemented | compiled, fake_tested, emulated_tested | failed proof retains resources/process for retry; SIGKILL is separate |
| [inspect_ports](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/usb.rs) | `ports` | implemented | compiled, fake_tested, emulated_tested | read fresh port status without W1C acknowledgement |
| [acknowledge_port](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/usb.rs) | `port_ack` | implemented | compiled, fake_tested | fake-tested; not a guest-tested W1C matrix |
| [reset_port](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/usb.rs) | `port_reset` | implemented | compiled, fake_tested, emulated_tested | normal reset guest-tested; warm-reset branch has compile/static coverage only |
| [enable_slot](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/usb.rs) | `slot_enable` | implemented | compiled, fake_tested, emulated_tested | enable exactly one slot after chosen-port reset |
| [address_bsr](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/usb.rs) | `address` | implemented | compiled, fake_tested, emulated_tested | BSR command bit/model behavior tested; no physical bus capture |
| [address_assigned](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/usb.rs) | `address` | implemented | compiled, fake_tested, emulated_tested | old TD replay explicitly excluded by independent trace |
| [descriptor_read](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/usb.rs) | `descriptor` | implemented | compiled, fake_tested, emulated_tested | issue only the requested descriptor read and retain raw bytes/parse errors |
| [ep0_packet_size](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/usb.rs) | `packet_size` | implemented | compiled, fake_tested, emulated_tested | set the explicitly selected speed-valid EP0 packet size |
| [select_configuration](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/usb.rs) | `configuration` | implemented | compiled, fake_tested, emulated_tested | send explicit SET_CONFIGURATION and update device state |
| [select_interface](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/usb.rs) | `interface` | implemented | compiled, fake_tested | fake-tested; nonzero alternate behavior is not guest-validated |
| [host_endpoints](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/usb.rs) | `endpoints` | implemented | compiled, fake_tested, emulated_tested | control/bulk/interrupt profile; hubs, streams and isochronous I/O not exposed |
| [control_complete](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/requests.rs) | `submit` | implemented | compiled, fake_tested, emulated_tested | reported code is the terminal TD event; intermediate events remain in emulator trace |
| [bulk_interrupt](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/requests.rs) | `submit` | implemented | compiled, fake_tested, emulated_tested | Low Speed bulk rejected; pending means submitted, not an electrical bus-active sensor |
| [control_plan](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/explicit.rs) | `control_plan` | implemented | compiled, fake_tested, emulated_tested | full plan only; arbitrary partial/delayed stages not exposed |
| [control_plan_compatibility](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/models/control_plan.rs) | `control_plan` | implemented | compiled, fake_tested, emulated_tested | guest case: wLength8, DATA18, actual8; other devices/field combinations unverified |
| [request_status](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/requests.rs) | `status` | implemented | compiled, fake_tested, emulated_tested | read a retained request state without a hardware poll |
| [request_poll_wait](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/requests.rs) | `poll`, `wait` | implemented | compiled, fake_tested, emulated_tested | deadline ends the wait; it does not stop DMA or USB traffic |
| [request_reclaim](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/requests.rs) | `reclaim` | implemented | compiled, fake_tested, emulated_tested | poll/wait may already retire a proven-complete streaming map; reclaim removes metadata |
| [logical_cancel](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/requests.rs) | `logical_cancel` | implemented | compiled, fake_tested, emulated_tested | does not submit Stop Endpoint or suppress late data |
| [abort_endpoint](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/recovery.rs) | `abort` | implemented | compiled, fake_tested, emulated_tested | endpoint-wide; old coherent ring quarantined until close; uncertain stop stays recovery-required |
| [reset_endpoint_preserve](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/recovery.rs) | `reset_endpoint` | implemented | compiled, fake_tested, emulated_tested | command/model tested; no physical DATA PID/USB3 sequence measurement |
| [reset_endpoint_state](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/recovery.rs) | `reset_endpoint` | implemented | compiled, fake_tested, emulated_tested | device protocol actions remain explicit; no arbitrary PID selection |
| [scoped_stall_recovery](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/recovery.rs) | `reset_endpoint` | implemented | compiled, fake_tested, emulated_tested | unknown controller/event/teardown faults still require coordinated close |
| [noop_command](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/services/explicit.rs) | `noop` | implemented | compiled, fake_tested, emulated_tested | no raw caller-provided DMA pointers or second command engine |
| [convenience_enumeration](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/presentation/cli/flows.rs) | `enumerate` | implemented | compiled, fake_tested, emulated_tested | configuration/endpoints only with configure=true; no timing guarantee |
| [ordered_batch](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/presentation/cli/flows.rs) | `batch` | implemented | compiled, fake_tested | fake-tested; preview validates syntax/shape, not unavailable execution state |
| [unsupported_queries](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/presentation/cli/handlers.rs) | `unsupported` | implemented | compiled, fake_tested, emulated_tested | return a capability error without approximate USB actions |
| [split_control_transaction](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/models/control_plan.rs) | — | unsupported | static_review | library profile limit; not a statement that the electrical interface forbids every such transaction |
| [arbitrary_data_pid](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/models/control_plan.rs) | — | unsupported | static_review | TSP reset/preserve is a distinct operation |
| [direct_dpdm](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/models/control_plan.rs) | — | unsupported | static_review | no Pico electrical-equivalence claim |
| [raw_pointer_ring_mutation](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/models/control_plan.rs) | — | unsupported | static_review | one authority must own rings, contexts and lifetimes |
| [timing_guarantees](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/domain/models/control_plan.rs) | — | unsupported | static_review | ordering/deadlines are software controls; physical timing remains unmeasured |
| [physical_intel_validation](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/presentation/cli/handlers.rs) | — | planned | unresolved | see #32 and #33; notebook controller takeover remains blocked |
| [ipad_a5_case_study](https://github.com/wilsonborba/fah/blob/4f0f03a/lib/presentation/cli/handlers.rs) | — | planned | unresolved | see #34; emulated USB success establishes no boot/exploitation capability |
<!-- FAH_CAPABILITIES_END -->


The table above is generated from `fah capabilities`; `python3 scripts/check-capabilities.py`
checks that it matches the executable. JSON includes implementation/test references,
coverage limits, the implementation baseline and exact guest artifact IDs. Reproduction
logs and independent traces are reviewed in [smoke #26](https://github.com/wilsonborba/fah/issues/26),
[explicit behavior #27](https://github.com/wilsonborba/fah/issues/27) and
[failure matrix #28](https://github.com/wilsonborba/fah/issues/28).

All guest results concern QEMU10.0.8's emulated controller. Physical validation remains
blocked on [target authorization #32](https://github.com/wilsonborba/fah/issues/32) and
[physical measurements #33](https://github.com/wilsonborba/fah/issues/33).
The [iPad/A5 capability assessment #34](https://github.com/wilsonborba/fah/issues/34)
remains pending. No result here establishes electrical Pico equivalence or that case study.
