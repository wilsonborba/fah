//! Explicit device preparation; every effect passes through the same session.
use crate::domain::{
    models::{
        ControllerState, DeviceState, ErrorCategory, OperationError,
        ports::{PortId, PortSnapshot, Ports},
    },
    services::session::{ControllerBackend, Session},
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
pub trait DeviceBackend: ControllerBackend {
    fn port_count(&self) -> Result<u8, OperationError>;
    fn port_status(&self, number: u8) -> Result<u32, OperationError>;
    fn acknowledge_port(&mut self, number: u8, changes: u32) -> Result<(), OperationError>;
    fn reset_port(&mut self, number: u8, warm: bool) -> Result<(), OperationError>;
    fn enable_slot(&mut self) -> Result<u8, OperationError>;
    fn address_device(&mut self, slot: u8, port: u8, bsr: bool) -> Result<(), OperationError>;
}
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceId {
    pub session: u64,
    pub generation: u64,
    pub slot: u8,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceSnapshot {
    pub id: DeviceId,
    pub port: PortId,
    pub state: DeviceState,
    pub configuration: Option<u8>,
}
pub struct UsbSession<B: DeviceBackend> {
    pub session: Session<B>,
    pub(crate) recoverable_stall: Option<super::recovery::RecoverableStall>,
    ports: Option<Ports>,
    reset_ports: BTreeMap<u8, PortId>,
    devices: BTreeMap<u8, DeviceSnapshot>,
    next_generation: u64,
    configurations: BTreeMap<u8, BTreeMap<u8, Vec<u8>>>,
    interfaces: BTreeMap<(u8, u8), u8>,
    pub(crate) endpoints: BTreeMap<u8, Vec<EndpointSnapshot>>,
    pub(crate) records: BTreeMap<crate::domain::models::RequestId, super::requests::RequestRecord>,
}
impl<B: DeviceBackend> UsbSession<B> {
    pub fn new(backend: B, identity: u64) -> Self {
        Self {
            session: Session::new(backend, identity),
            recoverable_stall: None,
            ports: None,
            reset_ports: BTreeMap::new(),
            devices: BTreeMap::new(),
            next_generation: 1,
            configurations: BTreeMap::new(),
            interfaces: BTreeMap::new(),
            endpoints: BTreeMap::new(),
            records: BTreeMap::new(),
        }
    }
    pub fn initialize(&mut self) -> Result<(), OperationError> {
        self.session.initialize()?;
        let count = self.session.initialized_backend()?.port_count()?;
        self.ports = Some(Ports::new(self.session.identity(), count));
        Ok(())
    }
    pub fn inspect_ports(&mut self) -> Result<Vec<PortSnapshot>, OperationError> {
        let backend = self.session.initialized_backend()?;
        self.ports
            .as_mut()
            .ok_or_else(|| OperationError::validation("ports not initialized"))?
            .inspect(|number| backend.port_status(number))
    }
    pub(crate) fn fresh_port(&mut self, id: PortId) -> Result<PortSnapshot, OperationError> {
        self.ports
            .as_ref()
            .ok_or_else(|| OperationError::validation("ports not initialized"))?
            .validate(id)?;
        let raw = self.session.initialized_backend()?.port_status(id.number)?;
        let ports = self
            .ports
            .as_mut()
            .ok_or_else(|| OperationError::validation("ports not initialized"))?;
        ports.validate(id)?;
        ports.observe(id.number, raw)?;
        ports.validate(id).cloned()
    }
    pub fn acknowledge_port(
        &mut self,
        id: PortId,
        changes: u32,
    ) -> Result<PortSnapshot, OperationError> {
        if changes & !0x00fe0000 != 0 {
            return Err(OperationError::validation("invalid port change mask"));
        }
        self.fresh_port(id)?;
        let result = self
            .session
            .initialized_backend()?
            .acknowledge_port(id.number, changes);
        self.effect(result)?;
        self.fresh_port(id)
    }
    pub fn reset_port(&mut self, id: PortId, warm: bool) -> Result<PortSnapshot, OperationError> {
        let port = self.fresh_port(id)?;
        if !port.connected
            || self
                .devices
                .values()
                .any(|device| device.port.number == id.number)
        {
            return Err(OperationError::validation(
                "reset requires connected port with no owned slot",
            ));
        }
        let result = self
            .session
            .initialized_backend()?
            .reset_port(id.number, warm);
        self.effect(result)?;
        let ports = self.ports.as_mut().unwrap();
        ports.invalidate(id.number)?;
        let raw = self.session.initialized_backend()?.port_status(id.number)?;
        let snapshot = self.ports.as_mut().unwrap().observe(id.number, raw)?;
        self.reset_ports.insert(id.number, snapshot.id);
        Ok(snapshot)
    }
    pub fn enable_slot(&mut self, port: PortId) -> Result<DeviceSnapshot, OperationError> {
        let observed = self.fresh_port(port)?;
        if !observed.connected
            || !observed.enabled
            || self.reset_ports.get(&port.number) != Some(&port)
            || self
                .devices
                .values()
                .any(|device| device.port.number == port.number)
        {
            return Err(OperationError::validation(
                "slot allocation requires a confirmed reset and an unused port",
            ));
        }
        let generation = self.next_generation;
        let next = generation
            .checked_add(1)
            .ok_or_else(|| OperationError::validation("device generation exhausted"))?;
        let result = self.session.initialized_backend()?.enable_slot();
        let slot = self.effect(result)?;
        if slot == 0 || self.devices.contains_key(&slot) {
            self.session.recovery_required();
            return Err(OperationError::validation(
                "backend returned invalid or duplicate slot",
            ));
        }
        self.next_generation = next;
        let device = DeviceSnapshot {
            id: DeviceId {
                session: self.session.identity(),
                generation,
                slot,
            },
            port,
            state: DeviceState::SlotEnabled,
            configuration: None,
        };
        self.devices.insert(slot, device.clone());
        Ok(device)
    }
    pub fn device_snapshots(&self) -> Vec<DeviceSnapshot> {
        self.devices.values().cloned().collect()
    }
    pub fn device(&self, id: DeviceId) -> Result<&DeviceSnapshot, OperationError> {
        let device = self
            .devices
            .get(&id.slot)
            .filter(|device| device.id == id)
            .ok_or_else(|| OperationError::validation("unknown or stale device"))?;
        if self.session.state() != ControllerState::Initialized {
            return Err(OperationError::validation("controller not initialized"));
        }
        self.ports
            .as_ref()
            .ok_or_else(|| OperationError::validation("ports unavailable"))?
            .validate(device.port)?;
        Ok(device)
    }
    pub fn address_device(
        &mut self,
        id: DeviceId,
        bsr: bool,
    ) -> Result<DeviceSnapshot, OperationError> {
        let device = self.device(id)?.clone();
        self.fresh_port(device.port)?;
        if device.state != DeviceState::SlotEnabled
            && (device.state != DeviceState::DefaultAddress || bsr)
        {
            return Err(OperationError::validation("invalid address transition"));
        }
        if bsr
            && self
                .devices
                .values()
                .any(|device| device.id != id && device.state == DeviceState::DefaultAddress)
        {
            return Err(OperationError::validation(
                "another slot already owns address zero",
            ));
        }
        if self.records.iter().any(|(request, record)| {
            record.metadata.device == id
                && self
                    .session
                    .requests
                    .state(*request)
                    .is_ok_and(|state| state.device_may_access())
        }) {
            return Err(OperationError::validation(
                "address assignment requires all device requests to be terminal",
            ));
        }
        let result =
            self.session
                .initialized_backend()?
                .address_device(id.slot, device.port.number, bsr);
        self.effect(result)?;
        let device = self.devices.get_mut(&id.slot).unwrap();
        device.state = if bsr {
            DeviceState::DefaultAddress
        } else {
            DeviceState::Addressed
        };
        Ok(device.clone())
    }
    pub fn close(&mut self) -> Result<(), OperationError> {
        self.session.close()?;
        self.ports = None;
        self.devices.clear();
        self.reset_ports.clear();
        self.configurations.clear();
        self.interfaces.clear();
        self.endpoints.clear();
        self.records.clear();
        self.recoverable_stall = None;
        Ok(())
    }
    pub(crate) fn effect<T>(
        &mut self,
        result: Result<T, OperationError>,
    ) -> Result<T, OperationError> {
        if result.as_ref().is_err_and(|error| {
            !matches!(
                error.category,
                ErrorCategory::Validation | ErrorCategory::Unsupported
            )
        }) {
            self.session.recovery_required();
        }
        result
    }
}

use crate::domain::models::descriptors::{
    DescriptorRead, DescriptorRequest, validate_configuration,
};
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EndpointSnapshot {
    pub address: u8,
    pub transfer_type: String,
    pub direction: String,
    pub max_packet_size: u16,
    pub interval: u8,
}
pub trait ConfigurationBackend: DeviceBackend {
    fn descriptor(
        &mut self,
        slot: u8,
        request: DescriptorRequest,
    ) -> Result<Vec<u8>, OperationError>;
    fn ep0_packet_size(&mut self, slot: u8, size: u16) -> Result<(), OperationError>;
    fn set_configuration(&mut self, slot: u8, value: u8) -> Result<(), OperationError>;
    fn set_interface(
        &mut self,
        slot: u8,
        interface: u8,
        alternate: u8,
    ) -> Result<(), OperationError>;
    fn setup_endpoints(
        &mut self,
        slot: u8,
        interface: u8,
        alternate: u8,
        configuration: &[u8],
    ) -> Result<Vec<EndpointSnapshot>, OperationError>;
}
impl<B: ConfigurationBackend> UsbSession<B> {
    pub(crate) fn prepared_device(
        &mut self,
        id: DeviceId,
    ) -> Result<DeviceSnapshot, OperationError> {
        let device = self.device(id)?.clone();
        self.fresh_port(device.port)?;
        if !matches!(
            device.state,
            DeviceState::DefaultAddress | DeviceState::Addressed | DeviceState::Configured
        ) {
            return Err(OperationError::validation("EP0 is not prepared"));
        }
        Ok(device)
    }
    pub fn descriptor(
        &mut self,
        id: DeviceId,
        request: DescriptorRequest,
    ) -> Result<DescriptorRead, OperationError> {
        self.prepared_device(id)?;
        if request.length == 0 {
            return Err(OperationError::validation(
                "descriptor length must be nonzero",
            ));
        }
        let result = self
            .session
            .initialized_backend()?
            .descriptor(id.slot, request);
        let bytes = self.effect(result)?;
        let result = DescriptorRead::from_bytes(request, bytes);
        if let Some(value) = result.complete_configuration_value() {
            let configurations = self.configurations.entry(id.slot).or_default();
            if !configurations.contains_key(&value) && configurations.len() >= 16 {
                configurations.pop_first();
            }
            let crate::domain::models::descriptors::DescriptorSummary::Configuration {
                total_length,
                ..
            } = result.parsed.as_ref().unwrap()
            else {
                unreachable!()
            };
            configurations.insert(value, result.bytes[..*total_length as usize].to_vec());
        }
        Ok(result)
    }
    pub fn ep0_packet_size(&mut self, id: DeviceId, size: u16) -> Result<(), OperationError> {
        let device = self.prepared_device(id)?;
        let speed = self.fresh_port(device.port)?.speed_code;
        let valid = match speed {
            1 => matches!(size, 8 | 16 | 32 | 64),
            2 => size == 8,
            3 => size == 64,
            4 | 5 => size == 512,
            _ => false,
        };
        if !valid {
            return Err(OperationError::validation("unsupported EP0 packet size"));
        }
        let result = self
            .session
            .initialized_backend()?
            .ep0_packet_size(id.slot, size);
        self.effect(result)
    }
    pub fn set_configuration(
        &mut self,
        id: DeviceId,
        value: u8,
    ) -> Result<DeviceSnapshot, OperationError> {
        let device = self.prepared_device(id)?;
        if device.state == DeviceState::DefaultAddress {
            return Err(OperationError::validation(
                "configuration requires an assigned address",
            ));
        }
        if self
            .endpoints
            .get(&id.slot)
            .is_some_and(|endpoints| !endpoints.is_empty())
        {
            return Err(OperationError::validation(
                "remove host endpoints before configuration changes",
            ));
        }
        let result = self
            .session
            .initialized_backend()?
            .set_configuration(id.slot, value);
        self.effect(result)?;
        self.interfaces.retain(|(slot, _), _| *slot != id.slot);
        let device = self.devices.get_mut(&id.slot).unwrap();
        device.configuration = if value == 0 { None } else { Some(value) };
        device.state = if value == 0 {
            DeviceState::Addressed
        } else {
            DeviceState::Configured
        };
        Ok(device.clone())
    }
    pub fn set_interface(
        &mut self,
        id: DeviceId,
        interface: u8,
        alternate: u8,
    ) -> Result<(), OperationError> {
        if self.prepared_device(id)?.state != DeviceState::Configured {
            return Err(OperationError::validation(
                "interface selection requires configuration",
            ));
        }
        if self
            .endpoints
            .get(&id.slot)
            .is_some_and(|endpoints| !endpoints.is_empty())
        {
            return Err(OperationError::validation(
                "teardown host endpoints before changing interface selection",
            ));
        }
        let result = self
            .session
            .initialized_backend()?
            .set_interface(id.slot, interface, alternate);
        self.effect(result)?;
        self.interfaces.insert((id.slot, interface), alternate);
        Ok(())
    }
    pub fn setup_endpoints(
        &mut self,
        id: DeviceId,
        interface: u8,
        alternate: u8,
    ) -> Result<Vec<EndpointSnapshot>, OperationError> {
        let device = self.prepared_device(id)?;
        let value = device.configuration.ok_or_else(|| {
            OperationError::validation("select configuration before endpoint setup")
        })?;
        if self
            .interfaces
            .get(&(id.slot, interface))
            .copied()
            .unwrap_or(0)
            != alternate
        {
            return Err(OperationError::validation(
                "select the interface alternate before host setup",
            ));
        }
        let configuration = self
            .configurations
            .get(&id.slot)
            .and_then(|configs| configs.get(&value))
            .cloned()
            .ok_or_else(|| {
                OperationError::validation("explicit full configuration descriptor required")
            })?;
        validate_configuration(&configuration).map_err(OperationError::validation)?;
        let port = self.fresh_port(device.port)?;
        validate_selected_endpoints(&configuration, interface, alternate, port.speed_code)?;
        let result = self.session.initialized_backend()?.setup_endpoints(
            id.slot,
            interface,
            alternate,
            &configuration,
        );
        let endpoints = self.effect(result)?;
        self.endpoints.insert(id.slot, endpoints.clone());
        Ok(endpoints)
    }
}
fn validate_selected_endpoints(
    bytes: &[u8],
    interface: u8,
    alternate: u8,
    speed: u8,
) -> Result<(), OperationError> {
    let mut offset = 9;
    let mut selected = false;
    let mut found = false;
    while offset < bytes.len() {
        let length = bytes[offset] as usize;
        let kind = bytes[offset + 1];
        if kind == 4 {
            selected = bytes[offset + 2] == interface && bytes[offset + 3] == alternate;
            found |= selected;
        }
        if kind == 5 && selected {
            let transfer_type = bytes[offset + 3] & 3;
            if transfer_type == 1 || (speed == 2 && transfer_type == 2) {
                return Err(OperationError {
                    category: ErrorCategory::Unsupported,
                    message: "endpoint transfer type unavailable for this speed/profile".into(),
                    os_code: None,
                    completion_code: None,
                });
            }
        }
        offset += length;
    }
    if !found {
        return Err(OperationError::validation(
            "requested interface/alternate absent from cached descriptor",
        ));
    }
    Ok(())
}
