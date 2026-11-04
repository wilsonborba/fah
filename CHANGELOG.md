# Changelog

## [0.2.0] - 2026-11-04

### Features
- **raw:** Define literal operations and xHCI transfer contracts (#18) ([c500af6](https://github.com/wilsonborba/fah/commit/c500af6e82f593f5dcb25a010c495379e8e12b76))
- **events:** Wake the sole consumer for acknowledged explicit drains (#74) ([b286534](https://github.com/wilsonborba/fah/commit/b28653427233bdec1f5aae3dbe538c10a1d6d8d0))
- **raw:** Expose literal operations through the sole controller owner (#68, #73, #75, #79) ([5bf464d](https://github.com/wilsonborba/fah/commit/5bf464deb85b70880922ee5b5713e1c40feae796))
- **iso:** Expose packet requests and explicit frame scheduling (#65) ([a2ff806](https://github.com/wilsonborba/fah/commit/a2ff8062eb0f41667c07dc5148ae67cd8e5619d0))
- **topology:** Expose hub contexts and explicit route addressing (#67) ([a59d54d](https://github.com/wilsonborba/fah/commit/a59d54d53e99b71d176b8279d3be217c284e1a29))
- **streams:** Configure primary arrays and per-stream owned rings (#66) ([ee35556](https://github.com/wilsonborba/fah/commit/ee35556e82920085b6e861a84fe92a9bc3580d87))
- **transfers:** Compose boundary-aware scatter-gather TDs (#76) ([58bff0d](https://github.com/wilsonborba/fah/commit/58bff0d56b5073e5a4b102f1aa774d4dfabbb546))
- **raw:** Classify completions and expose ordering controls (#69, #77) ([c54888c](https://github.com/wilsonborba/fah/commit/c54888c7e9dccf91a1eb2788ea22e9f0642fe8c1))
- **release:** Prepare v0.2.0 and require M8 guest evidence (#78) ([bc58d5e](https://github.com/wilsonborba/fah/commit/bc58d5e9ec6a7c36784f7ca02d73b8872fdb6c66))

### Fixes
- **tests:** Move the VM lab and test runners from scripts/ into tests/ (#64) ([c6b08ae](https://github.com/wilsonborba/fah/commit/c6b08ae88401a89cc7044c365e40bfdd30587c68))
- **tests:** Discover all lab cases during direct execution (#72) ([7a019ac](https://github.com/wilsonborba/fah/commit/7a019ac68e51617a1b074f591e419623cbf4301e))
- **release:** Derive lab and packaging branch from Cargo version (#71) ([804d307](https://github.com/wilsonborba/fah/commit/804d30724e1052aa5cd6bc542c864a975b957cae))
## [0.1.0] - 2026-11-03

### Features
- Add Rust application foundation and no-hardware CI ([3467d3e](https://github.com/wilsonborba/fah/commit/3467d3e9b1e4e06f6d96ea5a39759f63c2e29b47))
- Prepare pinned CrabUSB source with hosted direct backend ([22963c4](https://github.com/wilsonborba/fah/commit/22963c4a56296d965112507be223d256be26a740))
- Define session request and machine-result contracts ([fa8395a](https://github.com/wilsonborba/fah/commit/fa8395a7fba32951e70e38f40c6e3a647e72fc35))
- Add bounded IOVA reservations and mapping ownership ledger ([9cc8994](https://github.com/wilsonborba/fah/commit/9cc8994c74bbfcc9230793268f8c3372aded65ca))
- Implement VFIO KernelOp bridge with owned bounce mappings ([e67652f](https://github.com/wilsonborba/fah/commit/e67652fdfa2b823fd5eb4adde4b4ea4f0d516916))
- Validate VFIO regions and guest-only resource acquisition ([ce02975](https://github.com/wilsonborba/fah/commit/ce029754a7fdc88c687d486759e1fdee4fafaa88))
- **events:** Add single-consumer eventfd pump and VFIO IRQ binding (#9) ([e167404](https://github.com/wilsonborba/fah/commit/e167404f2004ceb8fba3150e0fdce6eae5e008a2))
- **session:** Own guest resources and explicit xHCI lifecycle (#11) ([6ad97d6](https://github.com/wilsonborba/fah/commit/6ad97d61c05ae5f4a490419728572cb8d84af1df))
- **ports:** Inspect root-port status without writes or enumeration (#12) ([35f3ca3](https://github.com/wilsonborba/fah/commit/35f3ca3e7f13ccbf97822553ca20b433544efd0a))
- **usb:** Separate port reset, slot enable and BSR addressing (#13) ([dd058a0](https://github.com/wilsonborba/fah/commit/dd058a06aa794c42065d4850cc4da509b279ddc2))
- **usb:** Expose descriptors and configuration as separate operations (#14) ([bc9b9ee](https://github.com/wilsonborba/fah/commit/bc9b9eed6652c96fa1bfd72fe2215cf84be1653e))
- **requests:** Own asynchronous buffers and preserve completion evidence (#15, #47) ([62c6a0e](https://github.com/wilsonborba/fah/commit/62c6a0e062299d5562e2e56fc1ea73908c05b8b8))
- **recovery:** Stop and rebase endpoint queues with confirmed abort scope (#16) ([0a4c45c](https://github.com/wilsonborba/fah/commit/0a4c45c13a170f2523e25f799ebf5a682a63e3f2))
- **lifecycle:** Coordinate resumable shutdown and gate DMA before start (#17, #48) ([db4ef6b](https://github.com/wilsonborba/fah/commit/db4ef6b66b73f29fe35998fae63746c8043d0af5))
- **contract:** Define full control plans and explicit capability boundaries (#18) ([cfbf546](https://github.com/wilsonborba/fah/commit/cfbf546e6d845e9e3b52abacf322f292132a6c95))
- **usb:** Publish explicit stage plans and managed endpoint reset ([d1c1c21](https://github.com/wilsonborba/fah/commit/d1c1c21651cbfed0df16ca74ab5fcd8f3fd65a3e))
- **cli:** Add persistent session shell and framed JSON input (#20) ([2d7da03](https://github.com/wilsonborba/fah/commit/2d7da03cf88cebebe0283bad1504c754ed7b9ef7))
- **cli:** Expose explicit USB operations and asynchronous wait events (#21) ([39b752b](https://github.com/wilsonborba/fah/commit/39b752b22a566bc0ea543165861983ff5757a327))
- **cli:** Add ordered batches and explicit convenience enumeration (#22) ([b392898](https://github.com/wilsonborba/fah/commit/b3928987e972f52178d287e2caae3045aa2bcc49))
- **capabilities:** Publish checked CLI inventory with scoped emulation evidence (#29) ([2af760c](https://github.com/wilsonborba/fah/commit/2af760c43d9235f1e29da4c22b7ae738c46e4faa))
- **release:** Assemble provenance-checked v0.1.0 preview with complete notices (#31, #59) ([0c559e6](https://github.com/wilsonborba/fah/commit/0c559e61747e3cae8d14e2055162df266d6332ff))

### Fixes
- **dma:** Distinguish coherent storage from streaming bounce maps (#37) ([7fab8f2](https://github.com/wilsonborba/fah/commit/7fab8f251734d99ad8168a798560c037d1460a0c))
- **backend:** Serialize hosted completion queues and repair unit target (#36, #38) ([6e837f0](https://github.com/wilsonborba/fah/commit/6e837f008e8cd56f31434d1005830b7b7c27e899))
- **tests:** Correct upstream GRF bitfield expectations (#39) ([14cf276](https://github.com/wilsonborba/fah/commit/14cf276460b743020c03e47300b0a7e8998697c6))
- **dma:** Patch dependency access and cycle publication contracts (#10) ([b641150](https://github.com/wilsonborba/fah/commit/b641150e771b509f035af6d411cdce95f3625318))
- **session:** Validate config access and read MMIO header by dword ([7da7a30](https://github.com/wilsonborba/fah/commit/7da7a30591f587069a089843d79b6c5315850b62))
- **dma:** Validate inclusive buffer end without overflow (#40) ([ef37a6b](https://github.com/wilsonborba/fah/commit/ef37a6b86ee56470eca862aafd2c34fbd359347d))
- **ci:** Pin and scope maintained backend regression dependencies (#42) ([3792997](https://github.com/wilsonborba/fah/commit/3792997f530862c274674fb2e44912540ad14f8c))
- **usb:** Initialize full-speed EP0 with eight-byte packets (#41) ([00d4384](https://github.com/wilsonborba/fah/commit/00d438499a4c6a780efdfdcb7bfd7f3daff2f504))
- **usb:** Correct SuperSpeedPlus EP0 default and test all speeds (#41) ([482533f](https://github.com/wilsonborba/fah/commit/482533fc362db078c759743032cd7f8584e7f4d7))
- **usb:** Validate descriptor EP0 packet encoding by speed (#43) ([e3c7020](https://github.com/wilsonborba/fah/commit/e3c702068e65d0d97da3567f025ee6dff74284ab))
- **usb:** Use highest DCI for Slot Context Entries (#44) ([bf55c23](https://github.com/wilsonborba/fah/commit/bf55c23d14535c4eed4d07ad8ecbfd6bc5f2fbaf))
- **usb:** Validate actual descriptor lengths before parsing (#45) ([37cfbc0](https://github.com/wilsonborba/fah/commit/37cfbc07d878b446a42ad770d7bb9dff8024aa61))
- **dma:** Respect segment boundaries and scope confirmed buffer reclamation (#46) ([087bcd4](https://github.com/wilsonborba/fah/commit/087bcd4e595075d0fed399665c12477c6d191442))
- **cli:** Retain ownership through interrupted or failed shutdown (#23, #50) ([f32092a](https://github.com/wilsonborba/fah/commit/f32092ac8a555789c0a0755ed34343782e9919bf))
- **vm:** Retain absent VGA region as optional metadata (#51) ([58a5cef](https://github.com/wilsonborba/fah/commit/58a5cef87c4b92970f953f5d4584f85897d49621))
- **xhci:** Enable DMA after reset before programming event table (#52) ([8b794e1](https://github.com/wilsonborba/fah/commit/8b794e1b2b136f6f59efc8465f27c588af9e1b4a))
- **vm:** Identify synthetic devices by endpoint capabilities (#53) ([f73ec85](https://github.com/wilsonborba/fah/commit/f73ec85ea54d6c2dba08771ee88cc864b184b88b))
- **vm:** Validate SCSI residue independently from USB completion (#54) ([7c17ebf](https://github.com/wilsonborba/fah/commit/7c17ebf556a66609956d59ea0cdad3431984bd56))
- **xhci:** Preserve empty EP0 dequeue across BSR address transition (#55) ([2ac932e](https://github.com/wilsonborba/fah/commit/2ac932e82df1c78ea870ebcb9ef6209bfb57a900))
- **xhci:** Encode TRB-aligned EP0 context pointer without overstrict setter (#56) ([e488158](https://github.com/wilsonborba/fah/commit/e488158e4cb7c65cce4dc94f0e395509d687c9f2))
- **recovery:** Resume only proven scoped Stall recovery with unchanged fault revision (#57) ([a0bf956](https://github.com/wilsonborba/fah/commit/a0bf956ac54d1db8d965e0c05889463baaf90c34))
- **session:** Namespace request identities across executable restarts (#58) ([65378e7](https://github.com/wilsonborba/fah/commit/65378e71be7c6ae871c322ef6f1c35442ec3711e))
- **deps:** Verify prepared trees read-only and apply patches outside any checkout (#60, #61) ([331c9c2](https://github.com/wilsonborba/fah/commit/331c9c215f915db7d021d9f51a1d6010d9e27b20))
- **release:** Verify sources first and rebuild the candidate cleanly with the pinned toolchain (#60, #61, #63) ([adf0066](https://github.com/wilsonborba/fah/commit/adf006678fd52626698f1c390788f35e2293d72a))
- **lab:** Keep VM state in target/, run evidence in docs/evidence and packages in target/fah-release (#62) ([c1f2a02](https://github.com/wilsonborba/fah/commit/c1f2a02862f811c2c35e249b03fbb69d9d45258e))

### Refactoring
- **usb:** Clarify legal address transitions ([f512c99](https://github.com/wilsonborba/fah/commit/f512c992601fca28c0af2e65452cb40d50d7f909))

### Documentation
- Define DMA access ownership and execution gates ([d7560f5](https://github.com/wilsonborba/fah/commit/d7560f54e1dbd677ae424fe5f8b5a4daacd12ec1))
- **vm:** Describe fixture commands, resource limits and evidence scope (#25) ([18c510a](https://github.com/wilsonborba/fah/commit/18c510a8254eb871c60f7a6305d6e9f8d766d5a7))
- Publish reproducible guest setup and complete explicit CLI protocol (#30) ([b61eb95](https://github.com/wilsonborba/fah/commit/b61eb95666002b5706d45d21f27cc03d7767ad07))
- **release:** Generate v0.1.0 changelog with git-cliff (#31) ([d2cb28a](https://github.com/wilsonborba/fah/commit/d2cb28aea5af949346d72b37fcf9f6bb80921bb5))
- **release:** Regenerate v0.1.0 changelog with git-cliff (#31) ([8e4a32a](https://github.com/wilsonborba/fah/commit/8e4a32ae59881eea1592fa6207d59814f945cb89))

### Validation
- **session:** Bound stalled backend futures without an executor ([b7cbf1a](https://github.com/wilsonborba/fah/commit/b7cbf1a032a876ca6795d681115217dfb95616df))
- **usb:** Verify reset and W1C writes with cooperative MMIO fixture ([23c5df2](https://github.com/wilsonborba/fah/commit/23c5df257b10da6eb507d8e65a47f85c4a4774e2))
- **vm:** Define marked emulated xHCI topology and translated VFIO probe (#24) ([02d618d](https://github.com/wilsonborba/fah/commit/02d618d4f211cdc32bb0b078795fa87ea70aa8f8))
- **vm:** Regress optional VGA metadata and propagate other failures (#51) ([67517e1](https://github.com/wilsonborba/fah/commit/67517e19661652de1fef3996df240527e732a55c))
- **vm:** Add isolated USB fixtures, payload, tracing and safe cleanup (#25) ([9dabbf6](https://github.com/wilsonborba/fah/commit/9dabbf673ecd76d0e2a60fa95df835ff9eccd8bc))
- **vm:** Exercise FAH control, bulk, interrupt and reopen in guest (#26) ([301825a](https://github.com/wilsonborba/fah/commit/301825af6b57318c5a78e41d261b6fa4e3d4abfe))
- **vm:** Independently trace explicit steps and compatibility control fields (#27) ([761ec60](https://github.com/wilsonborba/fah/commit/761ec609fddfebbc8fe5ea433dc23914d50c7526))
- **vm:** Exercise pending DMA, abort, signal and process-death matrix (#28) ([69665ac](https://github.com/wilsonborba/fah/commit/69665ac08971e71f7876b4c75e3f05679e2c1452))
- **release:** Require every named evidence case and pin changelog configuration (#31) ([2b8291d](https://github.com/wilsonborba/fah/commit/2b8291d95dbb66209b057daed40d922845405128))

### Maintenance
- Initial repository creation with logo and professional README ([64cdaee](https://github.com/wilsonborba/fah/commit/64cdaee49b717c9ef748f5def48e75e9e6f4f270))
