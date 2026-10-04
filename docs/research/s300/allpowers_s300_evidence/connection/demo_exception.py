import argparse,asyncio,json,sys,time,traceback
from datetime import datetime,timezone
from read_allpowers import read_station
async def main():
    start=time.monotonic()
    record=dict(utc=datetime.now(timezone.utc).isoformat(),scenario=sys.argv[1],scan_timeout_s=20,client_timeout_s=25)
    try:
        await asyncio.wait_for(read_station(argparse.Namespace(address="2A:02:01:48:6B:D0",adapter="hci2",seconds=5)),55)
        record["success"]=True
    except Exception as e:
        record.update(type=type(e).__module__+"."+type(e).__qualname__,message=str(e),repr=repr(e),traceback=traceback.format_exc())
    record["duration_s"]=time.monotonic()-start
    print(json.dumps(record),flush=True)
    open("/tmp/allpowers-contention/"+sys.argv[1]+"-demo.json","x").write(json.dumps(record,indent=2)+"\n")
asyncio.run(main())
