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
}
pub struct UsbSession<B: DeviceBackend> {
    pub session: Session<B>,
    ports: Option<Ports>,
    reset_ports: BTreeMap<u8, PortId>,
    devices: BTreeMap<u8, DeviceSnapshot>,
    next_generation: u64,
}
impl<B: DeviceBackend> UsbSession<B> {
    pub fn new(backend: B, identity: u64) -> Self {
        Self {
            session: Session::new(backend, identity),
            ports: None,
            reset_ports: BTreeMap::new(),
            devices: BTreeMap::new(),
            next_generation: 1,
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
    fn fresh_port(&mut self, id: PortId) -> Result<PortSnapshot, OperationError> {
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
        };
        self.devices.insert(slot, device.clone());
        Ok(device)
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
            && !(device.state == DeviceState::DefaultAddress && !bsr)
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
        Ok(())
    }
    fn effect<T>(&mut self, result: Result<T, OperationError>) -> Result<T, OperationError> {
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
