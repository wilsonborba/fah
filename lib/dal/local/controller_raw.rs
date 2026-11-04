//! Literal operations use the existing host rings and the existing event consumer.
use super::super::iova::Lease;
use super::*;
use crate::domain::models::raw::{Handle, RawOperation as Op, Ring};
use serde_json::{Value, json};
use std::collections::BTreeMap;

#[derive(Default)]
pub(super) struct RawOwner {
    sequence: u64,
    pub leases: BTreeMap<u64, Lease>,
    staged: BTreeMap<u64, [u32; 4]>,
    placed: BTreeMap<u64, (Ring, usize, bool, u64, bool)>,
    used: BTreeMap<Ring, usize>,
    published: bool,
    unknown_references: bool,
    active_dependencies: Vec<u64>,
    dependencies: BTreeMap<u64, std::collections::BTreeSet<u64>>,
}
impl RawOwner {
    fn handle(&mut self, session: u64) -> Result<Handle, OperationError> {
        self.sequence = self
            .sequence
            .checked_add(1)
            .ok_or_else(|| OperationError::validation("raw identity exhausted"))?;
        Ok(Handle {
            session,
            sequence: self.sequence,
        })
    }
}
fn validate_handle(handle: Handle, session: u64) -> Result<u64, OperationError> {
    if handle.session != session {
        return Err(OperationError::validation("stale raw session"));
    }
    Ok(handle.sequence)
}
fn ring_parts(ring: Ring) -> (u8, u8) {
    match ring {
        Ring::Command => (0, 0),
        Ring::Endpoint { slot, endpoint } => (slot, endpoint),
        Ring::Stream { slot, endpoint, .. } => (slot, endpoint),
    }
}
impl LocalController {
    fn raw_lease(&self, handle: Handle, session: u64) -> Result<Lease, OperationError> {
        let id = validate_handle(handle, session)?;
        self.raw
            .leases
            .get(&id)
            .copied()
            .ok_or_else(|| OperationError::validation("unknown raw DMA handle"))
    }
    fn read_register(&self, offset: usize) -> Result<u32, OperationError> {
        let mapping = self
            .mapping
            .as_ref()
            .ok_or_else(|| OperationError::validation("MMIO unavailable"))?;
        if !offset.is_multiple_of(4)
            || offset
                .checked_add(4)
                .is_none_or(|end| end > mapping.length())
        {
            return Err(OperationError::validation(
                "MMIO offset outside mapped span",
            ));
        }
        Ok(unsafe {
            mapping
                .pointer()
                .as_ptr()
                .add(offset)
                .cast::<u32>()
                .read_volatile()
        })
    }
    pub(super) fn execute_raw(
        &mut self,
        session: u64,
        request: Op,
    ) -> Result<Value, OperationError> {
        match request {
            Op::Barrier => {
                std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
                Ok(json!({"barrier":"seq_cst","packet_timing_guaranteed":false}))
            }
            Op::Timing => {
                let runtime = (self.read_register(24)? & !31) as usize;
                Ok(
                    json!({"mfindex":self.read_register(runtime)? & 0x3fff,"units":"125us modulo16384","physical_timing_guaranteed":false}),
                )
            }
            Op::Moderation { interval, counter } => {
                let runtime = (self.read_register(24)? & !31) as usize;
                let offset = runtime + 0x24;
                self.read_register(offset)?;
                unsafe {
                    self.mapping
                        .as_ref()
                        .unwrap()
                        .pointer()
                        .as_ptr()
                        .add(offset)
                        .cast::<u32>()
                        .write_volatile(interval as u32 | ((counter as u32) << 16));
                }
                Ok(json!({"interval":interval,"counter":counter,"interrupter":0}))
            }
            Op::NormalBuild {
                iova,
                length,
                max_packet,
                input,
            } => {
                let trbs = crate::domain::models::raw::normal_td(iova, length, max_packet, input)?;
                Ok(json!({"trbs":trbs,"executed":false}))
            }
            Op::ScatterTransfer {
                slot,
                endpoint,
                segments,
                max_packet,
                input,
            } => {
                let mut addresses = Vec::new();
                let mut dependencies = Vec::new();
                for segment in segments {
                    let lease = self.raw_lease(segment.handle, session)?;
                    if segment
                        .offset
                        .checked_add(segment.length)
                        .is_none_or(|end| end as u64 > lease.length)
                    {
                        return Err(OperationError::validation(
                            "scatter segment outside DMA lease",
                        ));
                    }
                    addresses.push((lease.iova + segment.offset as u64, segment.length));
                    dependencies.push(segment.handle.sequence);
                }
                let trbs = crate::domain::models::raw::scatter_td(&addresses, max_packet, input)?;
                self.raw.active_dependencies = dependencies;
                let result = self.execute_raw(
                    session,
                    Op::Transfer {
                        slot,
                        endpoint,
                        trbs,
                    },
                );
                self.raw.active_dependencies.clear();
                if result.is_err() {
                    self.raw.unknown_references = true;
                }
                result
            }
            Op::StreamConfigure {
                slot,
                endpoint,
                entries,
                device_max_streams,
            } => {
                let cap = (self.read_register(16)? >> 12) & 15;
                crate::domain::models::raw::validate_streams(
                    cap as u8,
                    device_max_streams,
                    entries,
                )?;
                let host = self
                    .host
                    .as_mut()
                    .ok_or_else(|| OperationError::validation("host unavailable"))?;
                device_future(host.configure_streams(slot, endpoint, entries))?;
                Ok(
                    json!({"entries":entries,"first_stream":1,"last_stream":entries-1,"managed_state":"unknown","end_to_end":"requires stream-capable fixture"}),
                )
            }
            Op::SlotEnable => {
                let host = self
                    .host
                    .as_mut()
                    .ok_or_else(|| OperationError::validation("host unavailable"))?;
                let slot = device_future(host.enable_slot())?;
                Ok(json!({"slot":slot,"managed_state":"unknown"}))
            }
            Op::AddressRoute {
                slot,
                root_port,
                speed,
                route,
                parent_slot,
                parent_port,
                multi_tt,
                tt_think,
                bsr,
            } => {
                let host = self
                    .host
                    .as_mut()
                    .ok_or_else(|| OperationError::validation("host unavailable"))?;
                device_future(host.address_route(
                    slot,
                    root_port,
                    speed,
                    route,
                    parent_slot,
                    parent_port,
                    multi_tt,
                    tt_think,
                    bsr,
                ))?;
                Ok(json!({"slot":slot,"route":route,"managed_state":"unknown"}))
            }
            Op::HubConfigure {
                slot,
                root_port,
                ports,
                multi_tt,
                tt_think_ns,
            } => {
                let host = self
                    .host
                    .as_mut()
                    .ok_or_else(|| OperationError::validation("host unavailable"))?;
                device_future(host.configure_hub(slot, root_port, ports, multi_tt, tt_think_ns))?;
                Ok(json!({"slot":slot,"ports":ports,"managed_state":"unknown"}))
            }
            Op::DeviceDescriptor {
                slot,
                kind,
                index,
                language,
                length,
            } => {
                use crate::domain::services::usb::ConfigurationBackend;
                let bytes = self.descriptor(
                    slot,
                    crate::domain::models::descriptors::DescriptorRequest {
                        kind,
                        index,
                        language,
                        length,
                    },
                )?;
                Ok(json!({"bytes":bytes}))
            }
            Op::DeviceConfiguration { slot, value } => {
                use crate::domain::services::usb::ConfigurationBackend;
                self.set_configuration(slot, value)?;
                Ok(json!({"value":value,"managed_state":"unknown"}))
            }
            Op::EndpointConfigure {
                slot,
                interface,
                alternate,
                configuration,
            } => {
                use crate::domain::services::usb::ConfigurationBackend;
                let endpoints = self.setup_endpoints(slot, interface, alternate, &configuration)?;
                Ok(json!({"endpoints":endpoints,"managed_state":"unknown"}))
            }
            Op::IsoSchedule {
                slot,
                endpoint,
                asap,
                frame_id,
            } => {
                if frame_id > 0x7ff {
                    return Err(OperationError::validation("FrameID exceeds11bits"));
                }
                self.host
                    .as_mut()
                    .ok_or_else(|| OperationError::validation("host unavailable"))?
                    .iso_schedule(slot, endpoint, asap, frame_id)
                    .map_err(usb_error)?;
                Ok(json!({"asap":asap,"frame_id":frame_id,"submitted":false}))
            }
            Op::Inspect => {
                let count = self
                    .mapping
                    .as_ref()
                    .ok_or_else(|| OperationError::validation("MMIO unavailable"))?
                    .length();
                let bytes: Vec<u8> = (0..count / 4)
                    .map(|i| self.read_register(i * 4).map(u32::to_le_bytes))
                    .collect::<Result<Vec<_>, _>>()?
                    .into_iter()
                    .flatten()
                    .collect();
                let hcc = self.read_register(16)?;
                let protocols = crate::domain::models::raw::protocols(&bytes);
                Ok(
                    json!({"hciversion":self.read_register(0)?>>16,"ac64":hcc&1!=0,"context_bytes":if hcc&4!=0{64}else{32},"max_psa_size":(hcc>>12)&15,"streams_supported":(hcc>>12)&15!=0,"protocols":protocols.as_ref().ok(),"protocol_error":protocols.err(),"mapped_bytes":count,"raw_capabilities":&bytes[..bytes.len().min(32)]}),
                )
            }
            Op::RegisterRead { offset, count } => {
                if count > 16384 {
                    return Err(OperationError::validation(
                        "register response limit exceeded",
                    ));
                }
                let mut words = Vec::new();
                for i in 0..count {
                    words.push(
                        self.read_register(
                            offset
                                .checked_add(i * 4)
                                .ok_or_else(|| OperationError::validation("offset overflow"))?,
                        )?,
                    );
                }
                Ok(json!({"offset":offset,"words":words}))
            }
            Op::RegisterWrite { offset, value } => {
                self.read_register(offset)?;
                let mapping = self.mapping.as_ref().unwrap();
                self.raw.published = true;
                self.raw.unknown_references = true;
                unsafe {
                    mapping
                        .pointer()
                        .as_ptr()
                        .add(offset)
                        .cast::<u32>()
                        .write_volatile(value);
                }
                std::sync::atomic::fence(std::sync::atomic::Ordering::SeqCst);
                Ok(json!({"written":value,"managed_state":"unknown","scope":"controller"}))
            }
            Op::DmaAlloc { bytes, align } => {
                let handle = self.raw.handle(session)?;
                let adapter = self
                    .adapter
                    .ok_or_else(|| OperationError::validation("DMA unavailable"))?;
                let lease = adapter.raw_allocate(bytes, align).map_err(os_error)?;
                self.raw.leases.insert(handle.sequence, lease);
                Ok(
                    json!({"handle":handle,"iova":lease.iova,"length":bytes,"mapped_length":lease.length,"coherent":true}),
                )
            }
            Op::DmaWrite {
                handle,
                offset,
                bytes,
            } => {
                let lease = self.raw_lease(handle, session)?;
                self.adapter
                    .unwrap()
                    .raw_write(lease, offset, &bytes)
                    .map_err(os_error)?;
                Ok(json!({"written":bytes.len()}))
            }
            Op::DmaRead {
                handle,
                offset,
                length,
            } => {
                if length > 524288 {
                    return Err(OperationError::validation("DMA response limit exceeded"));
                }
                let lease = self.raw_lease(handle, session)?;
                Ok(
                    json!({"bytes":self.adapter.unwrap().raw_read(lease,offset,length).map_err(os_error)?}),
                )
            }
            Op::DmaInspect { handle } => {
                let lease = self.raw_lease(handle, session)?;
                Ok(
                    json!({"handle":handle,"iova":lease.iova,"mapped_length":lease.length,"retained_until_close":self.raw.published}),
                )
            }
            Op::DmaUnmap { handle } => {
                let lease = self.raw_lease(handle, session)?;
                let adapter = self.adapter.unwrap();
                adapter.raw_retire(lease).map_err(os_error)?;
                let dependencies = self
                    .raw
                    .dependencies
                    .get(&handle.sequence)
                    .cloned()
                    .unwrap_or_default();
                let mut proven = dependencies.is_empty() && !self.raw.unknown_references;
                if !dependencies.is_empty() && !self.raw.unknown_references {
                    self.execute_raw(session, Op::Drain { timeout_ms: 1000 })?;
                    let events = self
                        .host
                        .as_ref()
                        .unwrap()
                        .raw_events(0, 4096)
                        .map_err(usb_error)?;
                    proven = dependencies.iter().all(|address| {
                        events.iter().any(|(_, words)| {
                            ((words[3] >> 10) & 63) == 32
                                && words[3] & (1 << 2) == 0
                                && matches!(words[2] >> 24, 1 | 13)
                                && ((words[0] as u64) | ((words[1] as u64) << 32)) == *address
                        })
                    });
                }
                if proven && !self.raw.unknown_references {
                    // No literal operation has ever been visible to this controller.
                    unsafe { adapter.reclaim_lease_after_quiescence(lease) }.map_err(os_error)?;
                    self.raw.leases.remove(&handle.sequence);
                    self.raw.dependencies.remove(&handle.sequence);
                }
                Ok(
                    json!({"retired":true,"released":proven&&!self.raw.unknown_references,"retained_until_close":!proven||self.raw.unknown_references}),
                )
            }
            Op::Compose { words } => {
                if self.raw.staged.len() >= 4096 {
                    return Err(OperationError::validation("staged TRB budget exceeded"));
                }
                let handle = self.raw.handle(session)?;
                self.raw.staged.insert(handle.sequence, words);
                Ok(json!({"staged":handle}))
            }
            Op::Place { staged, ring } => {
                let id = validate_handle(staged, session)?;
                let words = *self
                    .raw
                    .staged
                    .get(&id)
                    .ok_or_else(|| OperationError::validation("unknown staged TRB"))?;
                if !self.pending.is_empty() {
                    return Err(OperationError::validation(
                        "resolve managed outstanding requests before reserving a raw ring",
                    ));
                }
                let used = self.raw.used.get(&ring).copied().unwrap_or(0);
                if used >= 128 {
                    return Err(OperationError::validation(
                        "raw ring reservation budget exhausted; close before reuse",
                    ));
                }
                let handle = self.raw.handle(session)?;
                let (slot, endpoint) = ring_parts(ring);
                self.raw.published = true; // Prepared dependencies also pin mappings.
                if self.raw.active_dependencies.is_empty() {
                    self.raw.unknown_references = true;
                }
                let host = self
                    .host
                    .as_mut()
                    .ok_or_else(|| OperationError::validation("host unavailable"))?;
                let (index, cycle, address) = match ring {
                    Ring::Stream { stream, .. } => host.stream_place(slot, endpoint, stream, words),
                    _ => host.raw_place(slot, endpoint, words),
                }
                .map_err(usb_error)?;
                self.raw.staged.remove(&id);
                self.raw.used.insert(ring, used + 1);
                self.raw
                    .placed
                    .insert(handle.sequence, (ring, index, cycle, address, false));
                Ok(
                    json!({"publish_token":handle,"trb_iova":address,"index":index,"managed_state":"unknown"}),
                )
            }
            Op::Publish { token } => {
                let id = validate_handle(token, session)?;
                let (ring, index, cycle, address, published) = *self
                    .raw
                    .placed
                    .get(&id)
                    .ok_or_else(|| OperationError::validation("unknown publish token"))?;
                if published {
                    return Err(OperationError::validation("token already published"));
                }
                let (slot, endpoint) = ring_parts(ring);
                // Pin all current/future raw mappings before any controller-visible cycle.
                self.raw.published = true;
                let host = self
                    .host
                    .as_mut()
                    .ok_or_else(|| OperationError::validation("host unavailable"))?;
                match ring {
                    Ring::Stream { stream, .. } => {
                        host.stream_publish(slot, endpoint, stream, index, cycle)
                    }
                    _ => host.raw_publish(slot, endpoint, index, cycle),
                }
                .map_err(usb_error)?;
                self.raw.placed.get_mut(&id).unwrap().4 = true;
                Ok(json!({"published":token,"trb_iova":address,"managed_state":"unknown"}))
            }
            Op::Doorbell {
                slot,
                endpoint,
                stream,
            } => {
                let base = self.read_register(20)? as usize & !3;
                let slots = self.read_register(4)? as u8;
                if slot > slots || endpoint > 31 {
                    return Err(OperationError::validation("doorbell target out of range"));
                }
                let offset = base + slot as usize * 4;
                self.read_register(offset)?;
                self.raw.published = true;
                if self.raw.active_dependencies.is_empty() {
                    self.raw.unknown_references = true;
                }
                let value = endpoint as u32 | ((stream as u32) << 16);
                unsafe {
                    self.mapping
                        .as_ref()
                        .unwrap()
                        .pointer()
                        .as_ptr()
                        .add(offset)
                        .cast::<u32>()
                        .write_volatile(value);
                }
                Ok(json!({"doorbell":slot,"value":value,"managed_state":"unknown"}))
            }
            Op::Events { since, limit } => {
                if limit == 0 || limit > 4096 {
                    return Err(OperationError::validation("event limit must be1..4096"));
                }
                let events = self
                    .host
                    .as_ref()
                    .ok_or_else(|| OperationError::validation("host unavailable"))?
                    .raw_events(since, limit)
                    .map_err(usb_error)?;
                let next = events.last().map_or(since, |(seq, _)| *seq);
                let decoded:Vec<_>=events.iter().map(|(sequence,words)|{
                    let kind=((words[3]>>10)&63)as u8;let code=(words[2]>>24)as u8;
                    json!({"sequence":sequence,"type":kind,"code":code,"name":crate::domain::models::raw::completion_name(code),"class":crate::domain::models::raw::completion_class(kind,code)})
                }).collect();
                Ok(json!({"events":events,"decoded":decoded,"next":next}))
            }
            Op::Drain { timeout_ms } => {
                if timeout_ms > 60000 {
                    return Err(OperationError::validation("drain timeout exceeds60s"));
                }
                self.pump
                    .as_ref()
                    .ok_or_else(|| OperationError::validation("event consumer unavailable"))?
                    .drain_now(Duration::from_millis(timeout_ms))
                    .map_err(|e| os_error(e.to_string()))?;
                Ok(json!({"acknowledged":true,"events_retained":true}))
            }
            Op::Command { words, timeout_ms } => {
                let staged = self.execute_raw(session, Op::Compose { words })?;
                let staged: Handle = serde_json::from_value(staged["staged"].clone()).unwrap();
                let placed = self.execute_raw(
                    session,
                    Op::Place {
                        staged,
                        ring: Ring::Command,
                    },
                )?;
                let token: Handle =
                    serde_json::from_value(placed["publish_token"].clone()).unwrap();
                self.execute_raw(session, Op::Publish { token })?;
                self.execute_raw(
                    session,
                    Op::Doorbell {
                        slot: 0,
                        endpoint: 0,
                        stream: 0,
                    },
                )?;
                let deadline = Instant::now() + Duration::from_millis(timeout_ms.min(60000));
                loop {
                    self.execute_raw(session, Op::Drain { timeout_ms: 1000 })?;
                    let events = self
                        .host
                        .as_ref()
                        .unwrap()
                        .raw_events(0, 4096)
                        .map_err(usb_error)?;
                    if let Some((_, event)) = events.iter().find(|(_, e)| {
                        (e[3] >> 10) & 63 == 33
                            && (e[0] as u64 | ((e[1] as u64) << 32))
                                == placed["trb_iova"].as_u64().unwrap()
                    }) {
                        return Ok(
                            json!({"completion":event,"publish_token":token,"managed_state":"unknown"}),
                        );
                    }
                    if Instant::now() >= deadline {
                        return Ok(
                            json!({"pending":true,"publish_token":token,"managed_state":"unknown"}),
                        );
                    }
                    std::thread::sleep(Duration::from_millis(1));
                }
            }
            Op::Transfer {
                slot,
                endpoint,
                trbs,
            } => {
                if trbs.is_empty() || trbs.len() > 128 {
                    return Err(OperationError::validation("raw TD must contain1..128 TRBs"));
                }
                let mut tokens = Vec::new();
                for words in trbs {
                    let staged = self.execute_raw(session, Op::Compose { words })?;
                    let staged = serde_json::from_value(staged["staged"].clone()).unwrap();
                    let placed = self.execute_raw(
                        session,
                        Op::Place {
                            staged,
                            ring: Ring::Endpoint { slot, endpoint },
                        },
                    )?;
                    tokens.push(
                        serde_json::from_value::<Handle>(placed["publish_token"].clone()).unwrap(),
                    );
                }
                if let Some(last) = tokens.last() {
                    let address = self.raw.placed[&last.sequence].3;
                    for id in self.raw.active_dependencies.clone() {
                        self.raw.dependencies.entry(id).or_default().insert(address);
                    }
                }
                for token in tokens.iter().skip(1) {
                    self.execute_raw(session, Op::Publish { token: *token })?;
                }
                self.execute_raw(session, Op::Publish { token: tokens[0] })?;
                let dci = if endpoint & 15 == 0 {
                    1
                } else {
                    (endpoint & 15) * 2 + ((endpoint >> 7) & 1)
                };
                self.execute_raw(
                    session,
                    Op::Doorbell {
                        slot,
                        endpoint: dci,
                        stream: 0,
                    },
                )?;
                let addresses: Vec<_> = tokens
                    .iter()
                    .map(|token| self.raw.placed[&token.sequence].3)
                    .collect();
                Ok(
                    json!({"publish_tokens":tokens,"trb_iovas":addresses,"managed_state":"unknown","completion":"observe raw events"}),
                )
            }
            Op::Reconcile { slot } => Ok(
                json!({"scope":slot,"managed_state":"unknown","proof_required":"unknown raw state requires explicit reconstruction or coordinated close; a matching readback is insufficient"}),
            ),
            Op::CommandBuild {
                command,
                parameter,
                status,
                control,
            } => {
                let code = command.code();
                if code > 63 {
                    return Err(OperationError::validation("TRB type exceeds6bits"));
                }
                Ok(
                    json!({"words":[parameter as u32,(parameter>>32)as u32,status,(control&!(63<<10))|((code as u32)<<10)],"executed":false,"capability_support":"not inferred from encoding"}),
                )
            }
        }
    }
}
