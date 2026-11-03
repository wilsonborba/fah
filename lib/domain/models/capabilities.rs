//! Reviewed interface contract. Availability and evidence are separate claims.
use serde::Serialize;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Availability {
    Implemented,
    Planned,
    Unsupported,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Boundary {
    LibraryProfile,
    XhciInterface,
    DeviceDependent,
}
#[derive(Debug, Clone, Serialize)]
pub struct Capability {
    pub operation: &'static str,
    pub availability: Availability,
    pub boundary: Boundary,
    pub evidence: &'static [&'static str],
    pub precondition: &'static str,
    pub effect: &'static str,
    pub specification: &'static str,
}
pub const SPECIFICATION: &str = "https://www.intel.com/content/dam/www/public/us/en/documents/technical-specifications/extensible-host-controler-interface-usb-xhci.pdf";
pub fn registry() -> Vec<Capability> {
    use Availability::*;
    use Boundary::*;
    vec![
        Capability {
            operation: "inspect_ports",
            availability: Implemented,
            boundary: LibraryProfile,
            evidence: &["compiled", "fake_tested"],
            precondition: "initialized owner",
            effect: "read status; no W1C acknowledgement",
            specification: "4.19",
        },
        Capability {
            operation: "address_bsr",
            availability: Implemented,
            boundary: XhciInterface,
            evidence: &["compiled", "fake_tested"],
            precondition: "reset port and enabled slot",
            effect: "address context with SET_ADDRESS suppressed",
            specification: "4.6.5",
        },
        Capability {
            operation: "control_complete",
            availability: Implemented,
            boundary: LibraryProfile,
            evidence: &["compiled", "fake_tested"],
            precondition: "prepared EP0; compliant lengths and directions",
            effect: "complete SETUP/DATA/STATUS sequence",
            specification: "4.11.2.2",
        },
        Capability {
            operation: "control_plan",
            availability: Implemented,
            boundary: LibraryProfile,
            evidence: &["compiled", "fake_tested"],
            precondition: "validated full stage plan",
            effect: "submit declared stages under the sole owner",
            specification: "4.11.2.2",
        },
        Capability {
            operation: "control_plan_compatibility",
            availability: Implemented,
            boundary: DeviceDependent,
            evidence: &["compiled", "fake_tested"],
            precondition: "explicit device_compatibility profile; full plan",
            effect: "preserve wire fields independently from data TRB length/direction; device response unproven",
            specification: "4.11.2.2 non-compliant-device notes",
        },
        Capability {
            operation: "split_control_transaction",
            availability: Unsupported,
            boundary: LibraryProfile,
            evidence: &["static_review"],
            precondition: "additional state-machine research required",
            effect: "not exposed; not a claim that the electrical USB interface forbids it",
            specification: "4.11.2.2",
        },
        Capability {
            operation: "abort_endpoint",
            availability: Implemented,
            boundary: XhciInterface,
            evidence: &["compiled", "fake_tested"],
            precondition: "pending endpoint work and live command/event path",
            effect: "Stop Endpoint confirmation, collateral aborts and managed dequeue rebase",
            specification: "4.6.9 / 4.6.10",
        },
        Capability {
            operation: "reset_endpoint_preserve",
            availability: Implemented,
            boundary: XhciInterface,
            evidence: &["compiled", "fake_tested"],
            precondition: "Halted endpoint; explicit managed recovery",
            effect: "Reset Endpoint with TSP=1 then managed rebase",
            specification: "4.6.8",
        },
        Capability {
            operation: "reset_endpoint_state",
            availability: Implemented,
            boundary: XhciInterface,
            evidence: &["compiled", "fake_tested"],
            precondition: "Halted endpoint; device protocol actions remain explicit",
            effect: "Reset Endpoint with TSP=0 then managed rebase",
            specification: "4.6.8",
        },
        Capability {
            operation: "arbitrary_data_pid",
            availability: Unsupported,
            boundary: XhciInterface,
            evidence: &["static_review"],
            precondition: "no per-packet PID override in supported xHCI TRBs",
            effect: "no DATA0/DATA1 signal selection API",
            specification: "4.6.8 / 4.11.2.2",
        },
        Capability {
            operation: "direct_dpdm",
            availability: Unsupported,
            boundary: XhciInterface,
            evidence: &["static_review"],
            precondition: "physical PHY line driving is outside xHCI",
            effect: "no Pico electrical-equivalence claim",
            specification: "xHCI programming interface",
        },
        Capability {
            operation: "raw_pointer_ring_mutation",
            availability: Unsupported,
            boundary: LibraryProfile,
            evidence: &["static_review"],
            precondition: "one authority over rings, contexts and lifetimes",
            effect: "CLI accepts typed operations, not caller-provided DMA pointers",
            specification: "4.9 / 6.4",
        },
    ]
}
