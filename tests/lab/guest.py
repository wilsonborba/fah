#!/usr/bin/python3
"""Runs only inside the marked disposable guest; never on the host."""
import errno
import fcntl
import json
import os
import pathlib
import struct
import subprocess
import sys
import time
import traceback

BDF = '0000:00:02.0'
PCI = pathlib.Path('/sys/bus/pci/devices') / BDF
MARKER = pathlib.Path('/sys/firmware/qemu_fw_cfg/by_name/opt/fah-lab/raw')


def report(kind, **values):
    print('FAH_LAB ' + json.dumps({'kind': kind, **values}), flush=True)


def guard():
    expected = pathlib.Path('/run-id').read_bytes()
    assert expected and MARKER.read_bytes() == expected, 'guest marker mismatch'
    assert PCI.joinpath('vendor').read_text().strip() == '0x1b36'
    assert PCI.joinpath('device').read_text().strip() == '0x000d'
    assert PCI.joinpath('class').read_text().strip() == '0x0c0330'
    members = list(PCI.joinpath('iommu_group/devices').iterdir())
    assert [entry.name for entry in members] == [BDF], 'shared/unknown guest group'
    return PCI.joinpath('iommu_group').resolve().name


def bind():
    group = guard()
    assert not PCI.joinpath('driver').exists(), 'expected unused emulated controller'
    PCI.joinpath('driver_override').write_text('vfio-pci')
    pathlib.Path('/sys/bus/pci/drivers/vfio-pci/bind').write_text(BDF)
    assert PCI.joinpath('driver').resolve().name == 'vfio-pci'
    return group


def ioctl_buffer(fd, code, size, *fields):
    data = bytearray(size)
    struct.pack_into('<' + 'I' * len(fields), data, 0, *fields)
    fcntl.ioctl(fd, code, data, True)
    return data


def metadata():
    group_number = bind()
    with open('/dev/vfio/vfio', 'r+b', buffering=0) as container, open('/dev/vfio/' + group_number, 'r+b', buffering=0) as group:
        assert fcntl.ioctl(container, 0x3b64) == 0
        assert fcntl.ioctl(container, 0x3b65, 1) > 0, 'translated type1 IOMMU required'
        status = ioctl_buffer(group, 0x3b67, 8, 8, 0)
        assert struct.unpack_from('<II', status)[1] & 1
        fcntl.ioctl(group, 0x3b68, struct.pack('<i', container.fileno()))
        fcntl.ioctl(container, 0x3b66, 1)
        device = fcntl.ioctl(group, 0x3b6a, bytearray(BDF.encode() + b'\0'), True)
        assert isinstance(device, int) and device >= 0
        try:
            info = ioctl_buffer(device, 0x3b6b, 32, 32, 0)
            _, flags, regions, irqs = struct.unpack_from('<IIII', info)
            region_info = []
            for index in range(regions):
                try:
                    data = ioctl_buffer(device, 0x3b6c, 4096, 4096, 0, index, 0)
                except OSError as error:
                    if index == 8 and error.errno == errno.EINVAL:
                        region_info.append(dict(index=index, available=False, errno=error.errno, reason='non-VGA PCI device'))
                        continue
                    raise
                size, region_flags, _, cap_offset, length, offset = struct.unpack_from('<IIIIQQ', data)
                region_info.append(dict(index=index, argsz=size, flags=region_flags, cap_offset=cap_offset, size=length, offset=offset, raw=data[:size].hex()))
            irq_info = []
            for index in range(irqs):
                data = ioctl_buffer(device, 0x3b6d, 16, 16, 0, index, 0)
                argsz, irq_flags, _, count = struct.unpack('<IIII', data)
                irq_info.append(dict(index=index, argsz=argsz, flags=irq_flags, count=count))
            iommu = ioctl_buffer(container, 0x3b70, 4096, 4096, 0)
            report('metadata', bdf=BDF, group=group_number, viable=True, translated=True, flags=flags, regions=region_info, irqs=irq_info, iommu_raw=iommu[:struct.unpack_from('<I', iommu)[0]].hex(), kernel=os.uname().release, cmdline=pathlib.Path('/proc/cmdline').read_text())
        finally:
            os.close(device)
    return group_number


def restore():
    guard()
    assert PCI.joinpath('driver').resolve().name=='vfio-pci'
    pathlib.Path('/sys/bus/pci/drivers/vfio-pci/unbind').write_text(BDF)
    PCI.joinpath('driver_override').write_text('xhci_hcd')
    for module in json.loads(pathlib.Path('/restore-modules.json').read_text()):
        subprocess.run(['/bin/insmod',module],check=True)
    if not PCI.joinpath('driver').exists():
        pathlib.Path('/sys/bus/pci/drivers/xhci_hcd/bind').write_text(BDF)
    assert PCI.joinpath('driver').resolve().name=='xhci_hcd'
    import time
    deadline=time.monotonic()+10
    disk=pathlib.Path('/dev/sda')
    while not disk.exists() and time.monotonic()<deadline:time.sleep(.05)
    with disk.open('rb')as device:
        data=device.read(512)
    assert data==bytes(range(256))*2,'synthetic sector mismatch after guest restoration'
    report('restored',driver='xhci_hcd',disk='/dev/sda',bytes=len(data),data_correct=True)



def host_action(action,**values):
    report('host_action',action=action,**values)
    answer=json.loads(sys.stdin.readline())
    assert answer['ok'],answer
    return answer


class Cli:
    def __init__(self):
        self.errors=open('/tmp/fah.stderr','w+b')
        self.process=subprocess.Popen(['/bin/fah','shell','--json','--run-id',pathlib.Path('/run-id').read_text()],stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=self.errors,bufsize=0)
        self.events=[]

    def read(self,timeout=12):
        import select
        assert select.select([self.process.stdout],[],[],timeout)[0],'FAH response deadline'
        line=self.process.stdout.readline()
        if not line:
            self.errors.seek(0)
            raise RuntimeError('FAH exited: '+self.errors.read().decode(errors='replace'))
        frame=json.loads(line)
        assert frame['schema_version']==1
        report('cli',frame=frame)
        return frame

    def raw(self,operation):
        report('input',operation=operation)
        self.process.stdin.write((json.dumps(operation)+'\n').encode())
        self.process.stdin.flush()
        while True:
            frame=self.read()
            if frame['kind']=='response':return frame
            self.events.append(frame)

    def command(self,operation):
        frame=self.raw(operation)
        assert frame['outcome']['status']=='completed',frame
        return frame['outcome']['payload']

    def await_request(self,id,timeout_ms=3000):
        accepted=self.raw({'op':'wait','id':id,'timeout_ms':timeout_ms})
        assert accepted['outcome']['status']=='pending',accepted
        operation=accepted['operation_id']
        while True:
            event=next((item for item in self.events if item['operation_id']==operation),None)
            if event:
                self.events.remove(event)
            else:event=self.read()
            if event['operation_id']!=operation:
                self.events.append(event)
                continue
            assert event['kind']=='event'
            return event

    def transfer(self,device,endpoint,kind,direction,length,data=None,setup=None):
        accepted=self.command({'op':'submit','request':dict(device=device,endpoint=endpoint,transfer_type=kind,direction=direction,length=length,data=data or [],setup=setup)})
        event=self.await_request(accepted['id'])
        assert event['outcome']['status']=='completed',event
        completed=event['outcome']['payload']
        assert completed['state']['status']=='completed',completed
        reclaimed=self.command({'op':'reclaim','id':accepted['id']})
        assert reclaimed==completed
        return completed

    def quit(self,expected_exit=0):
        if self.process.poll()is None:
            self.command({'op':'quit'})
            self.process.wait(timeout=10)
        self.errors.seek(0)
        errors=self.errors.read().decode(errors='replace')
        assert self.process.returncode==expected_exit,dict(exit=self.process.returncode,expected_exit=expected_exit,stderr=errors)
        assert not errors,errors


def fixture_role(endpoints):
    capabilities={(ep['transfer_type'],ep['direction'])for ep in endpoints}
    if capabilities=={('bulk','in'),('bulk','out')}:
        return 'storage'
    if capabilities=={('interrupt','in')}:
        return 'keyboard'
    raise ValueError('unknown synthetic fixture endpoint capabilities')


def validate_csw(raw,tag,usb_length,scsi_length):
    assert len(raw)==13 and raw[:4]==b'USBS','invalid CSW signature/size'
    assert struct.unpack_from('<I',raw,4)[0]==tag,'CSW tag mismatch'
    assert raw[12]==0,'SCSI command failed'
    assert 0<=scsi_length<=usb_length,'invalid SCSI expectation'
    assert struct.unpack_from('<I',raw,8)[0]==usb_length-scsi_length,'SCSI residue mismatch'


def smoke():
    cli=Cli()
    try:
        cli.command({'op':'open','target':BDF})
        cli.command({'op':'init'})
        state=cli.command({'op':'state'})
        assert state['state']=='initialized'and state['resources']['interrupt_mode']=='msix',state
        ports=cli.command({'op':'ports'})
        connected=[port for port in ports if port['connected']]
        assert len(connected)==2,ports
        devices={}
        for port in connected:
            value=cli.command({'op':'enumerate','port':port['id'],'configure':True})
            steps=value['steps']
            descriptor=next(step['result']for step in steps if step['operation']=='device_descriptor')
            assert descriptor['bytes'][1]==1 and descriptor['actual_length']==18
            endpoints=next(step['result']for step in steps if step['operation']=='endpoints')
            role=fixture_role(endpoints)
            assert role not in devices,'duplicate fixture role'
            devices[role]=(value['device'],endpoints)
        storage,endpoints=devices['storage']
        bulk_in=next(ep['address']for ep in endpoints if ep['transfer_type']=='bulk'and ep['direction']=='in')
        bulk_out=next(ep['address']for ep in endpoints if ep['transfer_type']=='bulk'and ep['direction']=='out')
        short=cli.transfer(storage,0,'control','in',64,setup=dict(request_type=128,request=6,value=256,index=0,length=64))
        assert len(short['data'])==18 and short['state']['actual_length']==18
        assert short['state']['completion_code']in(1,13)
        tag=0
        def scsi(cdb,length,scsi_length=None):
            nonlocal tag
            tag+=1
            cbw=b'USBC'+struct.pack('<II',tag,length)+bytes([128,0,len(cdb)])+bytes(cdb).ljust(16,b'\0')
            assert len(cbw)==31
            out=cli.transfer(storage,bulk_out,'bulk','out',31,list(cbw))
            assert out['state']['actual_length']==31
            data=cli.transfer(storage,bulk_in,'bulk','in',length)
            csw=cli.transfer(storage,bulk_in,'bulk','in',13)
            raw=bytes(csw['data'])
            processed=length if scsi_length is None else scsi_length
            validate_csw(raw,tag,length,processed)
            assert len(data['data'])==length, data
            assert not any(data['data'][processed:]),'nonzero synthetic padding'
            return data
        inquiry=scsi([18,0,0,0,36,0],64,36)
        assert len(inquiry['data'])==64 and bytes(inquiry['data'][8:12])==b'QEMU'
        scsi([3,0,0,0,18,0],18)
        block=scsi([40,0,0,0,0,0,0,0,1,0],512)
        assert bytes(block['data'])==bytes(range(256))*2
        keyboard,endpoints=devices['keyboard']
        interrupt=next(ep['address']for ep in endpoints if ep['transfer_type']=='interrupt')
        host_action('key')
        reports=[]
        for _ in range(3):
            value=cli.transfer(keyboard,interrupt,'interrupt','in',8)
            reports.append(value['data'])
            if 4 in value['data'][2:]:break
        assert any(4 in data[2:]for data in reports),reports
        stale=block['id']
        cli.command({'op':'close'})
        closed=cli.command({'op':'state'})
        assert closed['resources']['retained_allocations']==0 and closed['state']=='closed',closed
        cli.command({'op':'open','target':BDF})
        cli.command({'op':'init'})
        rejected=cli.raw({'op':'status','id':stale})
        assert rejected['outcome']['status']=='failed'
        cli.command({'op':'close'})
        # Negative commands set CLI exit status1 even though close succeeded.
        cli.command({'op':'quit'})
        cli.process.wait(timeout=10)
        assert cli.process.returncode==1
        report('smoke',control_short=len(short['data']),bulk_read=len(block['data']),inquiry_usb_bytes=len(inquiry['data']),inquiry_scsi_bytes=36,inquiry_csw_residue=28,interrupt_reports=reports,closed_allocations=0,reopen=True,stale_rejected=True)
    finally:
        if cli.process.poll()is None:
            try:cli.command({'op':'close'});cli.command({'op':'quit'});cli.process.wait(timeout=10)
            except BaseException as error:report('cleanup_failure',error=repr(error))
        cli.errors.seek(0)
        errors=cli.errors.read().decode(errors='replace')
        if errors:report('cli_stderr',text=errors)
    restore()



def phase_begin(name):
    return host_action('phase',phase=name+'_begin')['trace_offset']


def phase_end(name,start,**checks):
    end=host_action('phase',phase=name+'_end')['trace_offset']
    report('trace_window',name=name,start=start,end=end,**checks)


def prepare_explicit(cli,port,configure=True):
    port=cli.command({'op':'port_reset','port':port})
    device=cli.command({'op':'slot_enable','port':port['id']})['id']
    cli.command({'op':'address','device':device})
    prefix=cli.command({'op':'descriptor','device':device,'request':dict(kind=1,index=0,language=0,length=8)})
    encoded=prefix['bytes'][7]
    packet=(1<<encoded)if port['speed_code']in(4,5)else encoded
    cli.command({'op':'packet_size','device':device,'size':packet})
    cli.command({'op':'descriptor','device':device,'request':dict(kind=1,index=0,language=0,length=18)})
    header=cli.command({'op':'descriptor','device':device,'request':dict(kind=2,index=0,language=0,length=9)})
    length=int.from_bytes(bytes(header['bytes'][2:4]),'little')
    full=cli.command({'op':'descriptor','device':device,'request':dict(kind=2,index=0,language=0,length=length)})
    if configure:
        cli.command({'op':'configuration','device':device,'value':full['bytes'][5]})
        cli.command({'op':'endpoints','device':device,'interface':0})
    return device


def explicit_behaviors():
    cli=Cli()
    try:
        start=phase_begin('passive')
        cli.command({'op':'open','target':BDF})
        invalid=cli.raw({'op':'ports'})
        assert invalid['outcome']['status']=='failed'
        cli.command({'op':'init'})
        ports=cli.command({'op':'ports'})
        cli.command({'op':'state'})
        phase_end('passive',start,forbid=['TR_SETUP','CR_ENABLE_SLOT','CR_ADDRESS_DEVICE','CR_CONFIGURE_ENDPOINT','usb_xhci_port_write'])
        port=next(port for port in ports if port['connected']and port['speed_code']==4)
        stale=port['id']
        start=phase_begin('chosen_reset')
        port=cli.command({'op':'port_reset','port':port['id']})
        phase_end('chosen_reset',start,require=['usb_xhci_port_reset port 2, warm 0'],forbid=['TR_SETUP','CR_ADDRESS_DEVICE','CR_ENABLE_SLOT'])
        start=phase_begin('slot')
        device=cli.command({'op':'slot_enable','port':port['id']})['id']
        phase_end('slot',start,require=['CR_ENABLE_SLOT'],forbid=['TR_SETUP','CR_ADDRESS_DEVICE'])
        start=phase_begin('bsr')
        cli.command({'op':'address','device':device,'bsr':True})
        phase_end('bsr',start,require=['CR_ADDRESS_DEVICE'],forbid=['TR_SETUP'],command_bit={'type':'CR_ADDRESS_DEVICE','mask':512,'set':True})
        start=phase_begin('descriptor_at_default')
        descriptor=cli.command({'op':'descriptor','device':device,'request':dict(kind=1,index=0,language=0,length=8)})
        assert descriptor['bytes'][0:2]==[18,1]
        phase_end('descriptor_at_default',start,require=['TR_SETUP','TR_DATA','TR_STATUS'],forbid=['CR_CONFIGURE_ENDPOINT'])
        start=phase_begin('assigned_address')
        cli.command({'op':'address','device':device,'bsr':False})
        phase_end('assigned_address',start,require=['CR_ADDRESS_DEVICE'],forbid=['TR_SETUP'],command_bit={'type':'CR_ADDRESS_DEVICE','mask':512,'set':False})
        start=phase_begin('compatibility_plan')
        plan=dict(device=device,profile='device_compatibility',setup=dict(request_type=128,request=6,value=256,index=0,length=8),data_stage=dict(direction='in',length=18),status_direction='out',data=[])
        request=cli.command({'op':'control_plan','plan':plan})
        event=cli.await_request(request['id'])
        assert event['outcome']['status']=='completed',event
        value=event['outcome']['payload']
        assert value['state']['actual_length']==8 and value['data'][0:2]==[18,1],value
        cli.command({'op':'reclaim','id':request['id']})
        phase_end('compatibility_plan',start,require=['TR_SETUP','TR_DATA','TR_STATUS'],setup_wlength=8,data_trb_length=18)
        start=phase_begin('negative_capabilities')
        plan['status_direction']=None
        rejected=cli.raw({'op':'control_plan','plan':plan})
        assert rejected['outcome']['status']=='failed'and rejected['outcome']['payload']['category']=='unsupported'
        for capability in ['arbitrary_data_pid','direct_dpdm','split_control_transaction']:
            rejected=cli.raw({'op':'unsupported','capability':capability})
            assert rejected['outcome']['status']=='failed'and rejected['outcome']['payload']['category']=='unsupported'
        assert cli.raw({'op':'port_reset','port':stale})['outcome']['status']=='failed'
        phase_end('negative_capabilities',start,forbid=['TR_SETUP','CR_ENABLE_SLOT','CR_ADDRESS_DEVICE','usb_xhci_port_write'])
        start=phase_begin('noop')
        result=cli.command({'op':'noop'})
        assert result['completion_code']==1
        phase_end('noop',start,require=['CR_NOOP'])
        cli.command({'op':'close'})
        cli.command({'op':'open','target':BDF});cli.command({'op':'init'})
        port=next(port for port in cli.command({'op':'ports'})if port['connected']and port['speed_code']==4)
        start=phase_begin('equivalent_explicit')
        prepare_explicit(cli,port['id'])
        phase_end('equivalent_explicit',start)
        cli.command({'op':'close'})
        cli.command({'op':'open','target':BDF});cli.command({'op':'init'})
        port=next(port for port in cli.command({'op':'ports'})if port['connected']and port['speed_code']==4)
        start=phase_begin('equivalent_convenience')
        cli.command({'op':'enumerate','port':port['id'],'configure':True})
        phase_end('equivalent_convenience',start,equivalent_to='equivalent_explicit')
        cli.command({'op':'close'});cli.command({'op':'quit'});cli.process.wait(timeout=10)
        assert cli.process.returncode==1,'negative commands must set nonzero exit status'
        report('explicit_behavior',bsr=True,compatibility_header=8,compatibility_buffer=18,compatibility_actual=8,no_implicit_enumeration=True,negative_cases=True)
    finally:
        if cli.process.poll()is None:
            try:cli.command({'op':'close'});cli.command({'op':'quit'});cli.process.wait(timeout=10)
            except BaseException as error:report('cleanup_failure',error=repr(error))
    restore()



def retained(cli):
    return cli.command({'op':'state'})['resources']['retained_allocations']


def bme_enabled():
    guard()
    with PCI.joinpath('config').open('rb')as config:
        config.seek(4)
        return bool(int.from_bytes(config.read(2),'little')&4)


def pending_interrupt(cli,device,endpoint):
    for _ in range(4):
        request=cli.command({'op':'submit','request':dict(device=device,endpoint=endpoint,transfer_type='interrupt',direction='in',length=8,setup=None)})
        event=cli.await_request(request['id'],20)
        if event['outcome']['status']=='partial':
            assert event['outcome']['payload']['error']['category']=='timeout',event
            return request['id']
        assert event['outcome']['status']=='completed',event
        cli.command({'op':'reclaim','id':request['id']})
    raise AssertionError('could not obtain a pending synthetic interrupt')


def prepared_keyboard():
    import contextlib
    @contextlib.contextmanager
    def context():
        host_action('key',down=False)
        cli=Cli()
        try:
            cli.command({'op':'open','target':BDF});cli.command({'op':'init'})
            # The fixed fixture's USB2 connector1 is observed at xHCI port5.
            port=next(port for port in cli.command({'op':'ports'})if port['id']['number']==5 and port['connected'])
            value=cli.command({'op':'enumerate','port':port['id'],'configure':True})
            endpoints=next(step['result']for step in value['steps']if step['operation']=='endpoints')
            assert fixture_role(endpoints)=='keyboard'
            yield cli,value['device'],endpoints[0]['address']
        finally:
            if cli.process.poll()is None:
                cli.command({'op':'close'})
                assert retained(cli)==0
                cli.command({'op':'quit'});cli.process.wait(timeout=10)
                assert cli.process.returncode in(0,1)
    return context()


def case_timeout_cancel_late():
    with prepared_keyboard()as(cli,device,endpoint):
        baseline=retained(cli)
        id=pending_interrupt(cli,device,endpoint)
        assert retained(cli)==baseline+1
        cli.command({'op':'logical_cancel','id':id})
        assert retained(cli)==baseline+1
        assert cli.raw({'op':'reclaim','id':id})['outcome']['status']=='failed'
        host_action('key',down=True)
        event=cli.await_request(id)
        assert event['outcome']['status']=='completed',event
        value=event['outcome']['payload']
        assert value['state']['status']=='completed'and value['logical_cancel']and 4 in value['data'][2:]
        assert retained(cli)==baseline
        cli.command({'op':'reclaim','id':id})
        return dict(baseline=baseline,retained_while_pending=baseline+1,retained_after_completion=baseline,logical_cancel_kept_dma=True,late_completion=True)


def case_abort_restart():
    with prepared_keyboard()as(cli,device,endpoint):
        baseline=retained(cli)
        first=pending_interrupt(cli,device,endpoint)
        second=cli.command({'op':'submit','request':dict(device=device,endpoint=endpoint,transfer_type='interrupt',direction='in',length=8,setup=None)})['id']
        assert retained(cli)==baseline+2
        result=cli.command({'op':'abort','id':first})
        assert result['restart_ready']and result['scope']=='endpoint_wide',result
        assert first in result['aborted']and second in result['aborted'],result
        assert retained(cli)==baseline+1,'one old coherent ring is quarantined; both streaming maps must retire'
        cli.command({'op':'reclaim','id':first});cli.command({'op':'reclaim','id':second})
        host_action('key',down=True)
        new=cli.transfer(device,endpoint,'interrupt','in',8)
        assert 4 in new['data'][2:],new
        return dict(collateral_requests=2,restart=True,retired_streaming=2,quarantined_coherent_rings=1)


def suspend_event_worker(cli):
    import contextlib
    import ctypes
    import time
    @contextlib.contextmanager
    def context():
        guard()
        assert pathlib.Path(f'/proc/{cli.process.pid}/exe').resolve()==pathlib.Path('/bin/fah')
        worker=next(path for path in pathlib.Path(f'/proc/{cli.process.pid}/task').iterdir()if(path/'comm').read_text().strip()=='fah-events')
        deadline=time.monotonic()+2
        while 'poll'not in(worker/'wchan').read_text()and time.monotonic()<deadline:time.sleep(.01)
        assert 'poll'in(worker/'wchan').read_text(),'event worker must be idle before injection'
        libc=ctypes.CDLL(None,use_errno=True)
        libc.ptrace.argtypes=[ctypes.c_ulong,ctypes.c_ulong,ctypes.c_void_p,ctypes.c_void_p]
        libc.ptrace.restype=ctypes.c_long
        tid=int(worker.name)
        if libc.ptrace(0x4206,tid,None,None)!=0:raise OSError(ctypes.get_errno(),'PTRACE_SEIZE guest event worker')
        attached=True
        try:
            if libc.ptrace(0x4207,tid,None,None)!=0:raise OSError(ctypes.get_errno(),'PTRACE_INTERRUPT guest event worker')
            waited,status=os.waitpid(tid,0x40000000)
            assert waited==tid and os.WIFSTOPPED(status)
            yield tid
        finally:
            if attached:
                assert libc.ptrace(17,tid,None,None)==0,'resume event worker before close/join'
    return context()


def case_uncertain_stop():
    with prepared_keyboard()as(cli,device,endpoint):
        id=pending_interrupt(cli,device,endpoint)
        before=retained(cli)
        with suspend_event_worker(cli)as tid:
            failure=cli.raw({'op':'abort','id':id})
            assert failure['outcome']['status']=='failed'and failure['session_state']=='recovery_required',failure
            assert retained(cli)==before
            assert cli.command({'op':'status','id':id})['state']['status']=='pending'
        # The delayed command event may now drain. Close still requires full halt.
        cli.command({'op':'close'})
        assert retained(cli)==0
        return dict(injection='guest_ptrace_event_worker_only',worker=tid,uncertain_stop=True,retained_before=before,retained_until_proof=before,closed_allocations=0)


def case_close_pending():
    with prepared_keyboard()as(cli,device,endpoint):
        id=pending_interrupt(cli,device,endpoint)
        before=retained(cli)
        cli.command({'op':'close'})
        assert retained(cli)==0 and not bme_enabled()
        return dict(pending=id,retained_before=before,closed_allocations=0,bme_after=False)


def case_eof_pending():
    with prepared_keyboard()as(cli,device,endpoint):
        id=pending_interrupt(cli,device,endpoint)
        before=retained(cli)
        cli.process.stdin.close()
        frame=cli.read()
        assert frame['session_state']=='closed'and frame['outcome']['payload']['reason']=='eof',frame
        cli.process.wait(timeout=10)
        assert cli.process.returncode==1 and not bme_enabled()
        return dict(pending=id,retained_before=before,normal_shutdown=True,bme_after=False)


def case_signals():
    import signal
    with prepared_keyboard()as(cli,device,endpoint):
        id=pending_interrupt(cli,device,endpoint)
        before=retained(cli)
        accepted=cli.raw({'op':'wait','id':id,'timeout_ms':100000})
        assert accepted['outcome']['status']=='pending'
        os.kill(cli.process.pid,signal.SIGINT)
        event=cli.read()
        assert event['kind']=='event'and event['operation_id']==accepted['operation_id']and event['outcome']['payload']['usb_aborted']is False,event
        assert retained(cli)==before and cli.command({'op':'status','id':id})['state']['status']=='pending'
        os.kill(cli.process.pid,signal.SIGINT)
        closed=cli.read()
        assert closed['session_state']=='closed'and closed['outcome']['payload']['reason']=='sigint'
        cli.process.wait(timeout=10)
        assert cli.process.returncode==1 and not bme_enabled()
        return dict(wait_interrupt_only=True,retained_during_wait_cancel=before,second_interrupt_closed=True,bme_after=False)


def case_term():
    import signal
    with prepared_keyboard()as(cli,device,endpoint):
        pending_interrupt(cli,device,endpoint)
        os.kill(cli.process.pid,signal.SIGTERM)
        closed=cli.read()
        assert closed['session_state']=='closed'and closed['outcome']['payload']['reason']=='sigterm'
        cli.process.wait(timeout=10)
        assert cli.process.returncode==1 and not bme_enabled()
        return dict(coordinated_close=True,bme_after=False)


def case_reset(preserve):
    with prepared_keyboard()as(cli,device,endpoint):
        request=cli.command({'op':'submit','request':dict(device=device,endpoint=0,transfer_type='control',direction='in',length=1,setup=dict(request_type=192,request=255,value=0,index=0,length=1))})
        failed=cli.await_request(request['id'])
        value=failed['outcome']['payload']
        assert value['state']['status']=='failed'and value['state']['completion_code']==6,value
        before=retained(cli)
        reset=cli.command({'op':'reset_endpoint','id':request['id'],'preserve':preserve})
        assert reset['restart_ready']and request['id']in reset['aborted'],reset
        cli.command({'op':'reclaim','id':request['id']})
        state=cli.command({'op':'state'})
        assert state['state']=='initialized','confirmed scoped endpoint recovery must allow continued operations'
        status=cli.transfer(device,0,'control','in',2,setup=dict(request_type=128,request=0,value=0,index=0,length=2))
        assert status['state']['actual_length']==2
        return dict(preserve=preserve,stall_code=6,retained_before_reset=before,restart=True,new_control_transfer=True)


def case_process_death():
    import signal
    with prepared_keyboard()as(cli,device,endpoint):
        old=pending_interrupt(cli,device,endpoint)
        before=retained(cli)
        os.kill(cli.process.pid,signal.SIGKILL)
        cli.process.wait(timeout=10)
        assert cli.process.returncode==-signal.SIGKILL
        assert not bme_enabled(),'guest VFIO/kernel did not disable PCI BME after process death'
    with prepared_keyboard()as(cli,device,endpoint):
        for index in range(old['sequence']+1):
            accepted=cli.command({'op':'submit','request':dict(device=device,endpoint=0,transfer_type='control',direction='in',length=2,setup=dict(request_type=128,request=0,value=0,index=0,length=2))})
            if index<old['sequence']:
                cli.await_request(accepted['id']);cli.command({'op':'reclaim','id':accepted['id']})
        assert accepted['id']!=old,'new CLI process reused the old session/request identity'
        assert cli.raw({'op':'status','id':old})['outcome']['status']=='failed','dead-process request ID aliased live work'
        assert cli.await_request(accepted['id'])['outcome']['payload']['state']['status']=='completed'
        cli.command({'op':'reclaim','id':accepted['id']})
        return dict(sigkill='guest_FA H_child_only'.replace(' ',''),retained_before_death=before,kernel_cleanup_bme_off=True,reacquired=True,old_identity_rejected=True,physical_behavior='unverified')


def case_disconnect():
    with prepared_keyboard()as(cli,device,endpoint):
        id=pending_interrupt(cli,device,endpoint)
        before=retained(cli)
        host_action('unplug_keyboard')
        import time
        deadline=time.monotonic()+3
        while True:
            ports=cli.command({'op':'ports'})
            disconnected=next(port for port in ports if port['id']['number']==5)
            if not disconnected['connected']:break
            assert time.monotonic()<deadline,'emulated unplug deadline'
            time.sleep(.01)
        report=cli.command({'op':'poll','id':id})
        assert retained(cli)==before,'disconnect alone must not free uncertain DMA'
        cli.command({'op':'close'})
        assert retained(cli)==0
        return dict(disconnected=True,request_state=report['state'],retained_until_close=before,closed_allocations=0)


def faults():
    cases=[('timeout_logical_cancel_late_completion',case_timeout_cancel_late),('endpoint_abort_restart',case_abort_restart),('uncertain_stop',case_uncertain_stop),('close_pending',case_close_pending),('eof_pending',case_eof_pending),('sigint_wait_then_shutdown',case_signals),('sigterm_pending',case_term),('endpoint_reset_state',lambda:case_reset(False)),('endpoint_reset_preserve',lambda:case_reset(True)),('guest_process_death',case_process_death),('disconnect_pending',case_disconnect)]
    for name,operation in cases:
        report('fault_start',name=name)
        result=operation()
        report('fault_case',name=name,verified=True,evidence='emulated_only',**result)
    restore()


def m8_behaviors():
    cli=Cli()
    expected_exit=0
    def raw(action,**args):return cli.command({'op':'raw','request':dict(action=action,**args)})
    def case(name,**values):report('m8_case',name=name,verified=True,**values)
    def alloc(length):return raw('dma_alloc',bytes=length)['handle']
    def terminal(tokens,timeout=5):
        token=tokens[-1]
        address=placements[token['sequence']]
        deadline=time.monotonic()+timeout
        while time.monotonic()<deadline:
            raw('drain',timeout_ms=1000)
            events=raw('events',since=0,limit=4096)['events']
            for _,words in events:
                if (words[3]>>10)&63==32 and words[0]|(words[1]<<32)==address:
                    assert words[2]>>24 in (1,13),words
                    return words
            time.sleep(.005)
        raise AssertionError('raw terminal event deadline')
    placements={}
    def scatter(slot,endpoint,handle,offset,length,packet):
        before=raw('events',since=0,limit=4096)['next']
        value=raw('scatter_transfer',slot=slot,endpoint=endpoint,segments=[dict(handle=handle,offset=offset,length=length)],max_packet=packet,input=bool(endpoint&128))
        # Capture the last published address from the raw event itself; a terminal
        # event is required, not merely a successful enqueue response.
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            raw('drain',timeout_ms=1000)
            events=raw('events',since=before,limit=4096)['events']
            matches=[words for _,words in events if (words[3]>>10)&63==32 and words[0]|(words[1]<<32)==value['trb_iovas'][-1]]
            if matches:
                words=matches[-1];assert words[2]>>24 in(1,13),words
                case('scatter_event',trbs=len(value['publish_tokens']),words=words)
                return value,words
            time.sleep(.005)
        raise AssertionError('scatter terminal event deadline')
    try:
        cli.command({'op':'open','target':BDF});cli.command({'op':'init'})
        cap=raw('inspect');assert cap['mapped_bytes']>=4096 and cap['protocols'],cap
        case('capabilities',capabilities=cap)
        raw('barrier');assert raw('timing')['physical_timing_guaranteed']is False
        # Isolate known-command coherence proof from deliberately unknown raw traffic.
        for port in cli.command({'op':'ports'}):
            if not port['connected']:continue
            prepared=cli.command({'op':'enumerate','port':port['id'],'configure':True})
            slot=prepared['device']['slot']
            break
        # A known Evaluate Context with unchanged slot fields may reconcile.
        snapshot=raw('slot_context',slot=slot)
        context=alloc(33*cap['context_bytes'])
        image=bytearray(33*cap['context_bytes']);struct.pack_into('<I',image,4,1)
        for i,word in enumerate(snapshot['words']):struct.pack_into('<I',image,cap['context_bytes']+i*4,word)
        raw('dma_write',handle=context,offset=0,bytes=list(image))
        pointer=raw('dma_inspect',handle=context)['iova']
        evaluated=raw('command',words=[pointer&0xffffffff,pointer>>32,0,(13<<10)|(slot<<24)])
        assert evaluated['completion'][2]>>24==1,evaluated
        reconciled=raw('reconcile',slot=slot)
        assert reconciled['restored'] and reconciled['managed_state']=='coherent',reconciled
        assert cli.command({'op':'ports'})
        case('scoped_reconcile',generation=evaluated['coherence_generation'])
        cli.command({'op':'close'});assert retained(cli)==0
        cli.command({'op':'open','target':BDF});cli.command({'op':'init'})
        large=alloc(196608)
        raw('dma_write',handle=large,offset=65530,bytes=list(range(20)))
        assert raw('dma_read',handle=large,offset=65530,length=20)['bytes']==list(range(20))
        plan=raw('normal_build',iova=0x1ff00,length=512,max_packet=512,input=True)['trbs']
        assert [trb[2]&0x1ffff for trb in plan]==[256,256]
        case('dma_large',split=plan)
        ports=cli.command({'op':'ports'})
        storage=None
        for port in ports:
            if not port['connected']:continue
            value=cli.command({'op':'enumerate','port':port['id'],'configure':True})
            endpoints=next(step['result']for step in value['steps']if step['operation']=='endpoints')
            if fixture_role(endpoints)=='storage':storage=(value['device'],endpoints)
        assert storage
        device,endpoints=storage;slot=device['slot']
        incoming=next(ep for ep in endpoints if ep['direction']=='in')
        outgoing=next(ep for ep in endpoints if ep['direction']=='out')
        # Real131072-byte READ10 through three chained TRBs, crossing two boundaries.
        tag=0x4d38
        cbw=b'USBC'+struct.pack('<II',tag,131072)+bytes([128,0,10])+bytes([0x28,0,0,0,0,0,0,1,0,0])+bytes(6)
        command=alloc(31);raw('dma_write',handle=command,offset=0,bytes=list(cbw))
        scatter(slot,outgoing['address'],command,0,31,outgoing['max_packet_size'])
        data,completion=scatter(slot,incoming['address'],large,65280,131072,incoming['max_packet_size'])
        assert len(data['publish_tokens'])==3 and completion[2]&0xffffff==0
        assert raw('dma_read',handle=large,offset=65280,length=512)['bytes']==list(range(256))*2
        status=alloc(13);scatter(slot,incoming['address'],status,0,13,incoming['max_packet_size'])
        csw=bytes(raw('dma_read',handle=status,offset=0,length=13)['bytes'])
        validate_csw(csw,tag,131072,131072)
        assert raw('dma_unmap',handle=command)['released']
        assert raw('dma_unmap',handle=large)['released']
        assert raw('dma_unmap',handle=status)['released']
        case('scatter_read10',trbs=3,bytes=131072)
        tur=b'USBC'+struct.pack('<II',tag+1,0)+bytes([0,0,6])+bytes(16)
        tur_command=alloc(31);raw('dma_write',handle=tur_command,offset=0,bytes=list(tur))
        scatter(slot,outgoing['address'],tur_command,0,31,outgoing['max_packet_size'])
        bei_buffer=alloc(13);iova=raw('dma_inspect',handle=bei_buffer)['iova']
        words=raw('normal_build',iova=iova,length=13,max_packet=incoming['max_packet_size'],input=True)['trbs'][0]
        words[3]|=1<<9
        since=raw('events',since=0,limit=4096)['next']
        submitted=raw('transfer',slot=slot,endpoint=incoming['address'],trbs=[words])
        deadline=time.monotonic()+5
        while time.monotonic()<deadline:
            raw('drain',timeout_ms=1000)
            events=raw('events',since=since,limit=4096)['events']
            if any((w[3]>>10)&63==32 and w[0]|(w[1]<<32)==submitted['trb_iovas'][-1] and w[2]>>24==1 for _,w in events):break
            time.sleep(.005)
        else:raise AssertionError('BEI completion not observable by explicit drain')
        validate_csw(bytes(raw('dma_read',handle=bei_buffer,offset=0,length=13)['bytes']),tag+1,0,0)
        case('bei_drain',words=words)
        # Literal Noop bypasses the typed TRB enum, using independent operations.
        built=raw('command_build',command='noop')['words'];assert built==[0,0,0,23<<10]
        staged=raw('compose',words=built)['staged']
        placed=raw('place',staged=staged,ring={'kind':'command'})
        raw('publish',token=placed['publish_token']);raw('doorbell',slot=0,endpoint=0)
        raw('drain',timeout_ms=1000)
        deadline=time.monotonic()+3
        while time.monotonic()<deadline:
            events=raw('events',since=0,limit=4096)['events']
            if any((w[3]>>10)&63==33 and w[0]|(w[1]<<32)==placed['trb_iova'] and w[2]>>24==1 for _,w in events):break
            raw('drain',timeout_ms=1000)
        else:raise AssertionError('literal Noop completion missing')
        case('literal_noop',words=built)
        retained_handle=alloc(32)
        assert raw('dma_unmap',handle=retained_handle)['retained_until_close']
        assert raw('reconcile')['managed_state']=='unknown'
        case('unknown_retention')
        cli.command({'op':'close'})
        cli.command({'op':'open','target':BDF});cli.command({'op':'init'})
        rejected=cli.raw({'op':'raw','request':{'action':'dma_read','handle':retained_handle,'offset':0,'length':1}})
        assert rejected['outcome']['status']=='failed'
        expected_exit=1
        case('session_handles')
        cli.command({'op':'close'});assert cli.command({'op':'state'})['resources']['retained_allocations']==0
    finally:cli.quit(expected_exit=expected_exit)


def topology_behaviors():
    """Disposable QEMU hub/audio/UAS fixtures; no host device access.

    UAS IU layouts and pipe IDs follow QEMU v10.0.0 hw/usb/dev-uas.c.
    """
    cli=Cli()
    def raw(action,**args):return cli.command({'op':'raw','request':dict(action=action,**args)})
    def case(name,**values):report('topology_case',name=name,verified=True,**values)
    def alloc(length):return raw('dma_alloc',bytes=length)['handle']
    def write(handle,data):raw('dma_write',handle=handle,offset=0,bytes=list(data))
    def read(handle,length):return bytes(raw('dma_read',handle=handle,offset=0,length=length)['bytes'])
    def wait(address,since=0):
        deadline=time.monotonic()+8
        while time.monotonic()<deadline:
            raw('drain',timeout_ms=1000)
            for _,words in raw('events',since=since,limit=4096)['events']:
                if (words[3]>>10)&63==32 and words[0]|(words[1]<<32)==address:
                    assert words[2]>>24 in(1,13),words
                    return words
            time.sleep(.005)
        raise AssertionError('topology transfer completion deadline')
    def control(slot,request_type,request,value,index,length):
        setup=struct.pack('<BBHHH',request_type,request,value,index,length)
        first,second=struct.unpack('<II',setup)
        trbs=[[first,second,8,(2<<10)|(1<<6)|((3 if request_type&128 else 2)<<16 if length else 0)]]
        buffer=None
        if length:
            buffer=alloc(length);iova=raw('dma_inspect',handle=buffer)['iova']
            trbs.append([iova&0xffffffff,iova>>32,length,(3<<10)|((1<<16)|(1<<2)if request_type&128 else 0)])
        trbs.append([0,0,0,(4<<10)|(1<<5)|(0 if length and request_type&128 else 1<<16)])
        result=raw('transfer',slot=slot,endpoint=0,trbs=trbs)
        wait(result['trb_iovas'][-1])
        return read(buffer,length)if buffer else b''
    def descriptor(slot,kind,length):return bytes(raw('device_descriptor',slot=slot,kind=kind,length=length)['bytes'])
    def fields(configuration):
        result=[];offset=0
        while offset<len(configuration):
            length=configuration[offset];assert length>=2 and offset+length<=len(configuration)
            result.append(configuration[offset:offset+length]);offset+=length
        return result
    def stream_submit(slot,endpoint,handle,length,stream):
        address=raw('dma_inspect',handle=handle)['iova']
        words=raw('normal_build',iova=address,length=length,max_packet=1024,input=bool(endpoint&128))['trbs'][0]
        staged=raw('compose',words=words)['staged']
        placed=raw('place',staged=staged,ring=dict(kind='stream',slot=slot,endpoint=endpoint,stream=stream))
        raw('publish',token=placed['publish_token'])
        dci=(endpoint&15)*2+int(bool(endpoint&128))
        raw('doorbell',slot=slot,endpoint=dci,stream=stream)
        return placed['trb_iova']
    try:
        cli.command({'op':'open','target':BDF});cli.command({'op':'init'})
        capabilities=raw('inspect');roots=[]
        for port in cli.command({'op':'ports'}):
            if not port['connected']:continue
            value=cli.command({'op':'enumerate','port':port['id'],'configure':False})
            desc=next(s['result']['bytes']for s in value['steps']if s['operation']=='device_descriptor')
            config=bytes(next(s['result']['bytes']for s in value['steps']if s['operation']=='configuration_body'))
            roots.append(dict(slot=value['device']['slot'],port=port,descriptor=desc,configuration=config))
        hub=next(d for d in roots if d['descriptor'][4]==9)
        uas=next(d for d in roots if any(f[1]==4 and len(f)>=9 and f[5]==8 and f[7]==0x62 for f in fields(d['configuration'])))
        hubslot=hub['slot'];rootport=hub['port']['id']['number']
        raw('device_configuration',slot=hubslot,value=hub['configuration'][5])
        hubdesc=control(hubslot,0xa0,6,0x2900,0,9)
        raw('hub_configure',slot=hubslot,root_port=rootport,ports=hubdesc[2])
        context=raw('slot_context',slot=hubslot)
        prepared=context['input_words'];output=context['words']
        assert prepared[0]&(1<<26) and prepared[1]>>24==hubdesc[2],context
        hub_fields_persisted=bool(output[0]&(1<<26) and output[1]>>24==hubdesc[2])
        control(hubslot,0x23,3,8,1,0) # Explicit downstream PORT_POWER.
        control(hubslot,0x23,3,4,1,0) # Explicit downstream PORT_RESET.
        deadline=time.monotonic()+3
        while True:
            status=struct.unpack('<HH',control(hubslot,0xa3,0,0,1,4))[0]
            if status&3==3 and not status&16:break
            assert time.monotonic()<deadline,status
            time.sleep(.02)
        speed=2 if status&(1<<9)else 3 if status&(1<<10)else 1
        child=raw('slot_enable')['slot']
        raw('address_route',slot=child,root_port=rootport,speed=speed,route=1,parent_slot=hubslot if hub['port']['speed_code']==3 else 0,parent_port=1)
        prefix=descriptor(child,1,8);raw('device_packet_size',slot=child,size=prefix[7])
        full=descriptor(child,1,18);assert len(full)==18
        header=descriptor(child,2,9);configuration=descriptor(child,2,int.from_bytes(header[2:4],'little'))
        case('hub_route',root_port=rootport,child=child,descriptor=list(full),input_context_verified=True,output_hub_fields_persisted=hub_fields_persisted,model_limitation=None if hub_fields_persisted else 'QEMU configure_slot only copies Context Entries; physical Hub/NumPorts persistence unvalidated')
        interfaces=[f for f in fields(configuration)if f[1]==4 and len(f)>=9]
        audio=next(f for f in interfaces if f[3]>0 and f[5:7]==bytes([1,2]))
        raw('device_configuration',slot=child,value=configuration[5])
        raw('device_interface',slot=child,interface=audio[2],alternate=audio[3])
        endpoints=raw('endpoint_configure',slot=child,interface=audio[2],alternate=audio[3],configuration=list(configuration))['endpoints']
        iso=next(ep for ep in endpoints if ep['transfer_type']=='isochronous' and ep['direction']=='out')
        raw('iso_schedule',slot=child,endpoint=iso['address'],asap=True,frame_id=0)
        audio_buffer=alloc(iso['max_packet_size']);pointer=raw('dma_inspect',handle=audio_buffer)['iova']
        trb=[pointer&0xffffffff,pointer>>32,iso['max_packet_size'],(5<<10)|(1<<5)|(1<<31)]
        result=raw('transfer',slot=child,endpoint=iso['address'],trbs=[trb])
        event=wait(result['trb_iovas'][-1]);case('isochronous',words=event,packet=iso['max_packet_size'])
        slot=uas['slot'];configuration=uas['configuration']
        raw('device_configuration',slot=slot,value=configuration[5])
        interface=next(f for f in fields(configuration)if f[1]==4 and f[5]==8 and f[7]==0x62)
        endpoints=raw('endpoint_configure',slot=slot,interface=interface[2],alternate=interface[3],configuration=list(configuration))['endpoints']
        pipes={};stream_caps={};endpoint=None
        for field in fields(configuration):
            if field[1]==5:endpoint=field[2]
            elif field[1]==0x24 and len(field)==4:pipes[field[2]]=endpoint
            elif field[1]==0x30 and endpoint is not None:stream_caps[endpoint]=field[3]&31
        assert capabilities['streams_supported'] and all(pipe in pipes for pipe in(1,2,3,4)),(capabilities,pipes)
        for pipe in(2,3,4):raw('stream_configure',slot=slot,endpoint=pipes[pipe],entries=4,device_max_streams=stream_caps[pipes[pipe]])
        sense=alloc(64);data=alloc(36)
        sense_trb=stream_submit(slot,pipes[2],sense,64,1)
        data_trb=stream_submit(slot,pipes[3],data,36,1)
        # QEMU command IU header +28-byte body; tag1 selects stream1.
        iu=bytes([1,0,0,1])+bytes(12)+bytes([0x12,0,0,0,36,0])+bytes(10)
        command=alloc(len(iu));write(command,iu);pointer=raw('dma_inspect',handle=command)['iova']
        words=raw('normal_build',iova=pointer,length=len(iu),max_packet=1024,input=False)['trbs']
        result=raw('transfer',slot=slot,endpoint=pipes[1],trbs=words)
        wait(result['trb_iovas'][-1]);wait(data_trb);wait(sense_trb)
        inquiry=read(data,36);status=read(sense,16)
        assert inquiry[8:12]==b'QEMU',inquiry
        assert status[0]==3 and status[2:4]==bytes([0,1])and status[6]==0,status
        case('stream_inquiry',stream_id=1,inquiry=list(inquiry),status=list(status))
        cli.command({'op':'close'});assert cli.command({'op':'state'})['resources']['retained_allocations']==0
    finally:cli.quit()


def main():
    report('start', suite=sys.argv[1], run_id=pathlib.Path('/run-id').read_text())
    metadata()
    if sys.argv[1]=='topology':
        topology_behaviors()
    elif sys.argv[1]=='m8':
        m8_behaviors()
    elif sys.argv[1]=='faults':
        faults()
    elif sys.argv[1]=='explicit':
        explicit_behaviors()
    elif sys.argv[1]=='smoke':
        smoke()
    elif sys.argv[1]=='fixture':
        restore()
    else:
        assert sys.argv[1] == 'feasibility', 'unknown suite'
    report('result', success=True, suite=sys.argv[1], evidence='emulated_model_only')


if __name__ == '__main__':
    try:
        main()
    except BaseException as error:
        report('failure', error=repr(error), traceback=traceback.format_exc())
        report('result', success=False, suite=sys.argv[1])
        sys.exit(1)
