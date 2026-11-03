# FAH — explicit USB host-controller research

FAH is a Rust CLI that owns one USB xHCI controller through Linux VFIO. It keeps
controller, device and request state in a persistent session and exposes deliberate
USB preparation and transfer steps. Optional enumeration and batch commands compose
that same API.

This is an **emulated preview**. The implemented path is validated against QEMU's
xHCI, a virtual IOMMU and guest VFIO. Conventional control/bulk/interrupt I/O,
independent operation traces and eleven failure/recovery cases pass. Physical Intel
behavior, deterministic wire timing and complete DMA soundness remain unproven.
The executable accepts only the marked emulated guest and never binds a host
controller. [Physical work is blocked](https://github.com/wilsonborba/fah/milestone/7).

The iPad mini 1/A5 is a case study for the desired degree of USB control. Its
compatibility remains [pending #34](https://github.com/wilsonborba/fah/issues/34).
FAH establishes no iPad boot capability or full Pico equivalence. xHCI exposes
transfer/port controls; its interface does not provide direct D+/D− PHY line driving.

## Build and non-hardware checks

The tested build target is Linux x86_64 with Rust **1.91.1**, pinned by
`rust-toolchain.toml`. Source preparation needs Git, Python3, network access and
`git apply`; Cargo uses the committed lockfile.

```sh
git clone https://github.com/wilsonborba/fah.git
cd fah
python3 scripts/prepare-dependencies.py
cargo check --locked
cargo test --locked
cargo build --locked --release --features hosted-direct
./target/release/fah --help
./target/release/fah --version
./target/release/fah capabilities
```

`--help`, `--version` and the JSON capability report perform no acquisition.
Without `hosted-direct`, the library/contracts/report still build and the executable
rejects shell startup. The hosted build is Linux x86_64 only.

These are the normal validation commands, also used by CI:

```sh
cargo fmt --check
cargo clippy --locked --all-targets -- -D warnings
cargo clippy --locked --all-targets --features hosted-direct -- -D warnings
cargo test --locked
cargo test --locked --features hosted-direct
python3 scripts/test-backend.py
python3 -m unittest discover -s tests -p 'test_*.py'
python3 scripts/check-capabilities.py
```

Private backend tests use their own committed lockfile and the same maintained
DMA source. Successful compilation is separate from fake, guest and physical evidence.

## Pinned dependencies and maintenance

| Component | Selected source | Role | License |
| --- | --- | --- | --- |
| CrabUSB `crab-usb 0.9.1` | `e83b56b0672460b400105f3855222af58c03af87` | sole xHCI command/transfer/event engine | MIT |
| `vfio-ioctls 0.10.0` | `93bcf25f3f8196314a71173ae96749b269a65b19` | local VFIO ownership/mappings/IRQs, defaults disabled | Apache-2.0 OR BSD-3-Clause |
| `dma-api 0.7.3` | registry archive SHA256 `6ce802d0c429c6966bea151e8e78eb5906d5b5e2cdebd0e960097cbe3654a3db` | shared DMA/KernelOp contract | Apache-2.0 |
| `xhci 0.9.2` | Cargo.lock | register/TRB/context types, used inside the sole engine | MIT OR Apache-2.0 |

The maintained changes are `patches/crabusb-hosted-backend.patch` and
`patches/dma-api-vfio.patch`. Preparation verifies the exact source revision/archive,
checks the complete patched tree and refuses unknown edits. `.fah-cache/` is generated
source storage, not an upstream fork or an independent second controller owner.

`python3 scripts/prepare-dependencies.py --check` only verifies, with no network
access and no changes. It rebuilds the expected trees from the pinned revision, the
pinned archive and the patches in a temporary directory, then compares every file.
Only CrabUSB's Git metadata and the top-level `target/` and `Cargo.lock` written by
`scripts/test-backend.py` are excluded. A difference stops the command and stays in
place. Both modes give the same result from any working directory.

CrabUSB changes expose explicit steps, hosted synchronization, reviewed DMA accesses,
real endpoint stop/reset/rebase and resumable lifecycle support. The DMA patch uses
volatile scalar access, disables mapped-reference helpers and supports the VFIO
adapter's separately mapped bounce storage. `KernelOp` includes `DmaOp` **and delay**.
Physical coherency/ordering behavior remains a separate validation question.

To update a dependency, choose a new exact revision/checksum, review the old/new
contracts and rebase the maintained patches. Preserve local work when preparation
refuses an unknown tree. Update the relevant lockfiles and preparation pins, review
mapped-memory access/publication and teardown, run both build profiles/private tests,
and repeat the guest suites from the release branch. Only add a previous patch hash
to `patches/crabusb-previous.sha256` after reviewing that exact maintained version.
An upstream change or a green build alone does not validate the updated driver.

## Reproduce the isolated guest

The tested host is Debian13 x86_64 with accessible `/dev/kvm`, QEMU10.0.8,
Python3.13, BusyBox, kmod, `ldd`, `findmnt` and `dpkg-deb`. The machine type is pinned
to `pc-q35-10.0`; the guest kernel/config/modules are fingerprinted in
[`scripts/lab/kernel.json`](scripts/lab/kernel.json). The launcher needs at least
4GiB available host memory and 2GiB free disk before starting its **2GiB/2vCPU**
headless guest. It checks current headroom; this is not a guarantee against host crashes.

The topology is:

```text
host QEMU/KVM
  -> guest Linux + virtual Intel IOMMU
     -> emulated qemu-xhci 0000:00:02.0
        -> guest vfio-pci/type1 translated container
           -> one FAH engine
```

Boot/initramfs/serial are independent of USB. The fixed fixtures are an emulated
keyboard and USB storage backed by a newly generated8MiB regular file. No host USB,
block or PCI device is forwarded. Before guest binding, the helper checks the
firmware run marker, emulated PCI class/vendor/device and singleton IOMMU group.
No-IOMMU and physical-controller fallback are unavailable.

A build container can help compile Rust. Docker alone does not supply this separate
guest kernel, virtual PCI controller and virtual-IOMMU/VFIO topology.

### Obtain guest kernel inputs without a host installation

The lab can read an already installed matching kernel, or a privately extracted
Debian package. The latter changes only the selected cache directory:

```sh
export FAH_KERNEL_DIR="$HOME/.cache/fah-kernel-inputs"
mkdir -p "$FAH_KERNEL_DIR"
curl --fail --location --output "$FAH_KERNEL_DIR/linux-image.deb" \
  'https://security.debian.org/debian-security/pool/updates/main/l/linux-signed-amd64/linux-image-6.12.95+deb13-amd64_6.12.95-1_amd64.deb'
echo "5daee28d4837901ab400d778dfcd3c6d9281a9ff580d8872a94a661c6b39c717  $FAH_KERNEL_DIR/linux-image.deb" | sha256sum --check -
dpkg-deb --extract "$FAH_KERNEL_DIR/linux-image.deb" "$FAH_KERNEL_DIR/root"
# Debian's extracted package uses usr/lib; provide its private merged-/usr alias.
if [ ! -e "$FAH_KERNEL_DIR/root/lib" ]; then
  ln -s usr/lib "$FAH_KERNEL_DIR/root/lib"
fi
/sbin/depmod -b "$FAH_KERNEL_DIR/root" -a 6.12.95+deb13-amd64
```

This does not install the kernel, load host modules or change host boot settings.
Preparation verifies the kernel, config and all16 selected module fingerprints.
Package origin/checksum are also recorded in `kernel.json`; generated manifests
record the exact userspace libraries and executable copied into the initramfs.

### Run and collect evidence

VM preparation/execution is deliberately restricted to a **clean
`release/v0.1.0`** checkout. Build outputs/caches are ignored and do not dirty it.
After changing the release source, rebuild the executable and prepare a new image.
Build the payload from a clean release profile with the pinned toolchain; do not set
`RUSTUP_TOOLCHAIN`. Cargo reuses artifacts from any toolchain of the same version,
and a different installation (for example one with `rust-src`) embeds other paths.

```sh
git switch release/v0.1.0
python3 scripts/prepare-dependencies.py
cargo clean --release
cargo build --locked --release --features hosted-direct
python3 scripts/lab.py plan
python3 scripts/lab.py prepare --kernel-root "$FAH_KERNEL_DIR/root"
python3 scripts/lab.py run --suite feasibility
python3 scripts/lab.py run --suite fixture
python3 scripts/lab.py run --suite smoke
python3 scripts/lab.py run --suite smoke
python3 scripts/lab.py run --suite explicit
python3 scripts/lab.py run --suite faults
```

With the exact kernel already installed, omit `--kernel-root` to use `/` as the
read-only input root. The recipe uses a deterministic newc/gzip initramfs and records
all inputs. Each run boots fresh guest state and creates its own synthetic disk.

| Suite | Observes |
| --- | --- |
| `feasibility` | marked singleton guest VFIO group, translated IOMMU, region/IRQ metadata |
| `fixture` | guest Linux driver restoration and known512-byte synthetic sector |
| `smoke` | FAH control/short response, bulk data/CSW, keyboard interrupt, close/reopen/stale IDs |
| `explicit` | independent QEMU trace windows: passive steps, BSR, one non-normalized control plan, rejected operations and equivalent convenience flow |
| `faults` | eleven cases: timeout/cancel/late completion, real abort/restart, delayed event confirmation, pending close/EOF/signals, both endpoint resets, guest-process death and disconnect |

The fault suite pauses only the **guest child's event thread** for its uncertain-stop
case, resumes it before teardown, and kills only a guest FAH child for SIGKILL testing.
Its process-death result describes the observed guest VFIO/kernel behavior.

Artifacts persist on disk in `~/.cache/fah-lab/runs/<run>`: exact QEMU command,
source/image manifest, serial/CLI frames, PCI/VFIO metadata, `xhci.trace`, suite result
and, where applicable, `trace-validation.json` or `failure-matrix.json`. The emulator
trace is independent of FAH logging and is not a physical USB analyzer capture.

```sh
python3 scripts/lab.py stop --run <generated-run-directory-name>
python3 scripts/lab.py cleanup --run <generated-run-directory-name>
```

Stop validates that the recorded PID belongs to that generated QEMU. Cleanup requires
its marker and a stopped process; it refuses unrelated/reused PIDs and arbitrary paths.
Artifacts/builds belong outside `/tmp`; tmpfs-backed lab directories are rejected.

## Persistent CLI and JSON protocol

A shell stays open and owns all controller/device/request state. The stdin worker
only reads bounded input; one actor owns the engine. IRQ/event draining has one
consumer. Starting the shell does not open or initialize a controller.

```sh
printf 'version\nstate\ncapabilities\nquit\n' | ./target/release/fah shell --json
```

Input is JSON Lines, plus the aliases `help`, `version`, `capabilities`, `state`,
`ports`, `noop`, `init`, `close`, `quit`. Human mode renders the same results.
Limits are1MiB/line,128 queued input lines,128 active requests,1024 retained results,
128 concurrent waits and64 operations/batch. Unknown fields, malformed input,
invalid widths and stale IDs are errors before the corresponding backend mutation.

JSON stdout contains one frame/line. Diagnostics use stderr. Every frame includes
`kind` (`response` or `event`), `schema_version:1`, `operation_id`, `session_state`
and `outcome`. Accepted waits return:

```json
{"kind":"response","schema_version":1,"operation_id":7,"session_state":"initialized","outcome":{"status":"pending"}}
```

Their later `event` repeats that operation ID. `completed` contains the operation's
payload; a request payload has its own state, actual length and terminal completion
code. `failed` contains category/message and nullable `os_code`/`completion_code`.
`partial` contains `{ "payload": <confirmed progress>, "error": <failure> }`.
A timeout can therefore return retained pending work rather than claiming USB stopped.
Errors/partial outcomes produce exit status1; invalid top-level arguments use2.
Normal successful shutdown uses0. EOF and normal signals wait for coordinated close;
a failed close keeps the process/resources alive and retries instead of silently
unmapping them. SIGKILL has separate, emulated-only evidence.

IDs contain `session`, `generation`, and `sequence` (requests), or `slot`/`number`
(device/port identities). Read them from actual responses; never reuse sample IDs or
IDs from earlier sessions/processes. Preserve their exact unsigned64-bit precision.
They identify lifetimes and are not authentication credentials.

### Explicit operations

Inside the marked guest, send `open` and `init` separately:

```json
{"op":"open","target":"0000:00:02.0"}
{"op":"init"}
{"op":"ports"}
```

Use a fresh port ID returned by `ports` for `port_reset`, the returned new ID for
`slot_enable`, and that device ID for `address`. `bsr:true` prepares the default
address without issuing SET_ADDRESS; later `bsr:false` assigns it. Addressing
requires no outstanding device request and preserves EP0's current empty-ring position.
Descriptor read, EP0 packet-size update, configuration, interface selection and
host endpoint setup are separate operations. Inspection does not perform these steps.

The following syntax examples use **illustrative IDs**; replace the complete ID from
your own responses. The automated guest client in
[`scripts/lab/guest.py`](scripts/lab/guest.py) demonstrates extracting real IDs.

```json
{"op":"port_reset","port":{"session":123,"generation":1,"number":2},"warm":false}
{"op":"slot_enable","port":{"session":123,"generation":2,"number":2}}
{"op":"address","device":{"session":123,"generation":1,"slot":1},"bsr":true}
{"op":"descriptor","device":{"session":123,"generation":1,"slot":1},"request":{"kind":1,"index":0,"language":0,"length":8}}
```

`submit` supports ordinary complete control, bulk and interrupt requests. OUT bytes
must match length; IN storage is allocated by the owner. Configure host endpoints
before non-control submissions; Low Speed bulk, hubs/streams/isochronous transfers
and lifecycle-changing requests through the ordinary control path are unsupported.

`control_plan` supports declared complete stages. The reviewed compatibility profile
preserves independent header and DATA fields; the guest example is `wLength=8`,
DATA buffer18, actual8. Other device/parameter combinations require their own evidence.

```json
{"op":"control_plan","plan":{"device":{"session":123,"generation":1,"slot":1},"profile":"device_compatibility","setup":{"request_type":128,"request":6,"value":256,"index":0,"length":8},"data_stage":{"direction":"in","length":18},"status_direction":"out","data":[]}}
```

A status stage is required in this profile. Arbitrary partial/delayed transactions,
per-packet DATA0/DATA1 overrides, direct PHY driving and caller DMA-pointer writes
are unavailable. Typed `noop`, BSR and `reset_endpoint` are controller operations;
there is no parallel command engine.

### Requests, recovery and convenience

| Operation | Effect |
| --- | --- |
| `status` | inspect retained request metadata; no hardware poll |
| `poll` | observe completion and permit retirement of a proven-complete streaming map |
| `wait` | register an asynchronous deadline; other input remains usable |
| `logical_cancel` | mark intent; traffic/buffer ownership continues and late data is retained |
| `abort` | confirm Stop Endpoint, report collateral requests, retire proven-safe buffers and rebase |
| `reset_endpoint` | require Halted, select `preserve:true` (TSP1) or `false` (TSP0), then managed rebase |
| `reclaim` | remove already terminal result metadata; reject uncertain ownership |
| `close` | halt, disable BME/IRQ, join consumer, drop engine, reclaim DMA, release BAR/VFIO |

An abort affects the whole endpoint. Old coherent rings are quarantined until close,
with a64-rebuild/session budget. A failed/uncertain command retains resources and
requires recovery. Only a completely confirmed, specifically tracked Stall recovery
with an unchanged fault revision can resume the session; unrelated faults are not
cleared by an endpoint reset. Device protocol actions remain explicit.

First SIGINT while a CLI wait is active cancels the wait ticket only. Idle SIGINT or
SIGTERM requests coordinated close. Repeated signals never authorize early unmap.
Kernel/ioctl scheduling and physical process-death behavior are not timing guarantees.

`enumerate` is an optional compound operation. `configure:true` selects configuration
and host endpoints; without it only preparation/descriptors run. Each completed step
and failure prefix is returned. `batch` executes the same typed operations in order,
stops on its first failure and reports outstanding work. `submit` does not await;
`wait` registers an asynchronous event. A preview validates input shape, labels
execution state unknown and changes no backend state. Ordering is not a packet-timing
promise.

```json
{"op":"batch","preview":true,"operations":[{"op":"state"},{"op":"ports"}]}
```

## Architecture and evidence

- `src/` is the thin executable/library entry point.
- `lib/core` owns common settings/identity support.
- `lib/dal/local` owns VFIO, MMIO, DMA, IOVA, interrupt and teardown adapters.
- `lib/domain/models` and `lib/domain/services` enforce ownership, IDs and explicit transitions.
- `lib/presentation/cli` separates commands, handlers/flows, formatters and response frames.

One maintained CrabUSB backend owns all rings, contexts, slots, event routing and
buffer lifetimes. VFIO/MMIO is **local** access. The CLI never creates a second engine
or gives caller pointers to an uncoordinated raw ring.

The inventory below is generated from `fah capabilities`; `python3
scripts/check-capabilities.py` verifies agreement. JSON also includes implementation/test
references and exact guest run provenance. Source implementation baseline is4f0f03a;
release packages record their final source/binary fingerprints separately.

Reviewed evidence: [smoke #26](https://github.com/wilsonborba/fah/issues/26),
[explicit traces #27](https://github.com/wilsonborba/fah/issues/27),
[eleven-case fault matrix #28](https://github.com/wilsonborba/fah/issues/28).
Compiled/fake/emulated levels never imply a whole-driver soundness proof or physical
validation. Remaining gates are [isolated target #32](https://github.com/wilsonborba/fah/issues/32),
[physical measurements #33](https://github.com/wilsonborba/fah/issues/33) and
[case-study assessment #34](https://github.com/wilsonborba/fah/issues/34).

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

## References and license

- [xHCI specification1.22](https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/extensible-host-controler-interface-usb-xhci.pdf)
- [Linux VFIO](https://docs.kernel.org/driver-api/vfio.html)
- [QEMU USB emulation](https://www.qemu.org/docs/master/system/devices/usb.html)
- [Project Cascadia](https://github.com/rzxvx/Project-Cascadia), context for the pending iPad case study
- [CrabUSB](https://github.com/drivercraft/CrabUSB), [rust-vmm/vfio](https://github.com/rust-vmm/vfio), [rust-osdev/xhci](https://github.com/rust-osdev/xhci)

FAH is dual-licensed under [MIT](LICENSE-MIT) or [Apache-2.0](LICENSE-APACHE).
Dependencies retain their own licenses. Modified source patches and release packages
must carry their applicable upstream notices; dependency updates require renewed review.


## Build a versioned preview candidate

`python3 scripts/release.py audit` checks the locked normal/build dependency graph,
preserved upstream texts and reviewed omissions; `--write-notices` regenerates
`THIRD_PARTY_NOTICES.md`. The candidate includes the full license texts, modification
notice, source snapshot, patches, checksums and dependency provenance. See
[third-party notices](THIRD_PARTY_NOTICES.md) and [modifications](NOTICE).

On the clean release, after rebuilding/preparing and passing the six required fresh
runs, package their exact generated directories:

```text
python3 scripts/release.py candidate --evidence <feasibility-run> --evidence <fixture-run> --evidence <smoke-run-1> --evidence <smoke-run-2> --evidence <explicit-run> --evidence <faults-run>
```

Each evidence argument is an absolute path under the generated lab runs. Packaging
requires the same source commit and executable hash as every selected run. It verifies
the eleven trace windows/failure rows and refuses failed, stale or incomplete evidence.
Both `audit` and `candidate` first run the `--check` comparison above, before Cargo
metadata or license texts are read. The candidate refuses a `RUSTUP_TOOLCHAIN`
override, removes the release profile, rebuilds from the verified trees, repeats the
comparison and records it in `provenance.json`. The rebuilt executable must equal the
VM-tested one. Unknown dependency edits stop packaging.
Artifacts default to `~/.cache/fah-release/`; `SHA256SUMS`, sidecar archive checksum,
`provenance.json`, `dependency-licenses.json` and `RELEASE-NOTES.md` are included.
The binary is tested for the recorded Linux/userspace/toolchain; arbitrary-host byte
identity or ABI compatibility is not promised.

Regenerate the changelog with `git-cliff --tag v0.1.0 --output CHANGELOG.md` using
git-cliff2.10.1. The packaging tool never merges, tags or publishes. Final main/tag
promotion follows the explicitly authorized release workflow after guest validation;
publication of a hosted release remains a separate action.
