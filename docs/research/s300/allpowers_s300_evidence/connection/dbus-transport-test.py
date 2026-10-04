import asyncio,json,time,traceback
from control_allpowers import resolve_adapter
from bleak import BleakScanner
async def main():
    records=[]
    for name,fn in (("resolve",resolve_adapter),("bleak_scanner_start",lambda: BleakScanner(bluez={"adapter":"hci2"}).start())):
        start=time.monotonic()
        try:
            await asyncio.wait_for(fn(),5)
        except Exception as e:
            records.append(dict(operation=name,type=type(e).__module__+"."+type(e).__qualname__,message=str(e),repr=repr(e),seconds=time.monotonic()-start,traceback=traceback.format_exc()))
    print(json.dumps(records),flush=True)
    open("/tmp/allpowers-contention/dbus-transport-unavailable.json","x").write(json.dumps(records,indent=2)+"\n")
asyncio.run(main())
