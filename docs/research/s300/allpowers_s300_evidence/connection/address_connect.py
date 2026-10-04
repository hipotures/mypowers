import asyncio, json, time, traceback
from datetime import datetime, timezone
from bleak import BleakClient
async def main():
    start=time.monotonic()
    record={"utc":datetime.now(timezone.utc).isoformat(),"test":"phone-first address-based implicit scan","address":"2A:02:01:48:6B:D0","adapter":"hci2","client_timeout":25}
    client=BleakClient(record["address"],timeout=25,bluez={"adapter":"hci2"})
    try:
        await asyncio.wait_for(client.connect(),35)
        record["connected"]=client.is_connected
    except Exception as error:
        record.update(type=type(error).__module__+"."+type(error).__qualname__,message=str(error),repr=repr(error),args=error.args,traceback=traceback.format_exc())
    finally:
        record["duration_s"]=time.monotonic()-start
        await client.disconnect()
        print(json.dumps(record),flush=True)
        open("/tmp/allpowers-contention/phone-first-address-connect.json","x").write(json.dumps(record,indent=2)+"\n")
asyncio.run(main())
