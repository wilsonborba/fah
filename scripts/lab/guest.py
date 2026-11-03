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
            devices['storage'if port['speed_code']>=3 else 'keyboard']=(value['device'],endpoints)
        storage,endpoints=devices['storage']
        bulk_in=next(ep['address']for ep in endpoints if ep['transfer_type']=='bulk'and ep['direction']=='in')
        bulk_out=next(ep['address']for ep in endpoints if ep['transfer_type']=='bulk'and ep['direction']=='out')
        short=cli.transfer(storage,0,'control','in',64,setup=dict(request_type=128,request=6,value=256,index=0,length=64))
        assert len(short['data'])==18 and short['state']['actual_length']==18
        assert short['state']['completion_code']in(1,13)
        tag=0
        def scsi(cdb,length):
            nonlocal tag
            tag+=1
            cbw=b'USBC'+struct.pack('<II',tag,length)+bytes([128,0,len(cdb)])+bytes(cdb).ljust(16,b'\0')
            assert len(cbw)==31
            out=cli.transfer(storage,bulk_out,'bulk','out',31,list(cbw))
            assert out['state']['actual_length']==31
            data=cli.transfer(storage,bulk_in,'bulk','in',length)
            csw=cli.transfer(storage,bulk_in,'bulk','in',13)
            raw=bytes(csw['data'])
            assert raw[:4]==b'USBS'and struct.unpack_from('<I',raw,4)[0]==tag and raw[12]==0,raw
            assert struct.unpack_from('<I',raw,8)[0]==length-len(data['data'])
            return data
        inquiry=scsi([18,0,0,0,36,0],64)
        assert len(inquiry['data'])==36 and bytes(inquiry['data'][8:12])==b'QEMU'
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
        report('smoke',control_short=len(short['data']),bulk_read=len(block['data']),inquiry_short=len(inquiry['data']),interrupt_reports=reports,closed_allocations=0,reopen=True,stale_rejected=True)
    finally:
        if cli.process.poll()is None:
            try:cli.command({'op':'close'});cli.command({'op':'quit'});cli.process.wait(timeout=10)
            except BaseException as error:report('cleanup_failure',error=repr(error))
        cli.errors.seek(0)
        errors=cli.errors.read().decode(errors='replace')
        if errors:report('cli_stderr',text=errors)
    restore()


def main():
    report('start', suite=sys.argv[1], run_id=pathlib.Path('/run-id').read_text())
    metadata()
    if sys.argv[1]=='smoke':
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
