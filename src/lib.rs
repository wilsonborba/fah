#[cfg(all(
    feature = "hosted-direct",
    not(all(target_os = "linux", target_arch = "x86_64"))
))]
compile_error!("hosted-direct is reviewed only for coherent Linux x86_64 DMA");

#[path = "../lib/app.rs"]
pub mod app;
#[path = "../lib/core/mod.rs"]
pub mod core;
#[path = "../lib/presentation/mod.rs"]
pub mod presentation;

#[path = "../lib/dal/mod.rs"]
pub mod dal;

#[path = "../lib/domain/mod.rs"]
pub mod domain;
