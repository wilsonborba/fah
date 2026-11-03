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

    def quit(self):
        if self.process.poll()is None:
            self.command({'op':'quit'})
            self.process.wait(timeout=10)
        self.errors.seek(0)
        errors=self.errors.read().decode(errors='replace')
        assert self.process.returncode==0,dict(exit=self.process.returncode,stderr=errors)
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


def main():
    report('start', suite=sys.argv[1], run_id=pathlib.Path('/run-id').read_text())
    metadata()
    if sys.argv[1]=='explicit':
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
