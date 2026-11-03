//! Passive root-port observations. IDs identify observed controller epochs;
//! unobserved disconnect/reconnect cannot be ruled out by a status read alone.
use crate::domain::models::OperationError;
use serde::{Deserialize, Serialize};
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PortId {
    pub session: u64,
    pub generation: u64,
    pub number: u8,
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PortSnapshot {
    pub id: PortId,
    pub raw: u32,
    pub connected: bool,
    pub enabled: bool,
    pub resetting: bool,
    pub speed_code: u8,
    pub change_bits: u32,
    pub source: String,
}
pub struct Ports {
    session: u64,
    entries: Vec<Option<PortSnapshot>>,
    generations: Vec<u64>,
}
impl Ports {
    pub fn new(session: u64, count: u8) -> Self {
        Self {
            session,
            entries: vec![None; count as usize],
            generations: vec![1; count as usize],
        }
    }
    pub fn count(&self) -> u8 {
        self.entries.len() as u8
    }
    pub fn observe(&mut self, number: u8, raw: u32) -> Result<PortSnapshot, OperationError> {
        let index = self.index(number)?;
        if self.entries[index]
            .as_ref()
            .is_some_and(|old| old.connected != (raw & 1 != 0))
        {
            self.invalidate(number)?;
        }
        let snapshot = PortSnapshot {
            id: PortId {
                session: self.session,
                generation: self.generations[index],
                number,
            },
            raw,
            connected: raw & 1 != 0,
            enabled: raw & 2 != 0,
            resetting: raw & (1 << 4 | 1 << 31) != 0,
            speed_code: ((raw >> 10) & 15) as u8,
            change_bits: raw & 0x00fe0000,
            source: "fresh_register_read".into(),
        };
        self.entries[index] = Some(snapshot.clone());
        Ok(snapshot)
    }
    pub fn validate(&self, id: PortId) -> Result<&PortSnapshot, OperationError> {
        let index = self.index(id.number)?;
        if id.session != self.session || id.generation != self.generations[index] {
            return Err(OperationError::validation("stale port identity"));
        }
        self.entries[index]
            .as_ref()
            .ok_or_else(|| OperationError::validation("port has not been observed"))
    }
    pub fn invalidate(&mut self, number: u8) -> Result<(), OperationError> {
        let index = self.index(number)?;
        self.generations[index] = self.generations[index]
            .checked_add(1)
            .ok_or_else(|| OperationError::validation("port generation exhausted"))?;
        self.entries[index] = None;
        Ok(())
    }
    fn index(&self, number: u8) -> Result<usize, OperationError> {
        if number == 0 || number as usize > self.entries.len() {
            return Err(OperationError::validation("invalid root port"));
        }
        Ok(number as usize - 1)
    }
    /// Only callback reads are invoked. The callback contract forbids acknowledgments
    /// and device discovery. Mutations have separate backend entry points.
    pub fn inspect(
        &mut self,
        mut read: impl FnMut(u8) -> Result<u32, OperationError>,
    ) -> Result<Vec<PortSnapshot>, OperationError> {
        (1..=self.count())
            .map(|number| {
                let raw = read(number)?;
                self.observe(number, raw)
            })
            .collect()
    }
}
