import json,subprocess,time
from datetime import datetime,timezone
from pathlib import Path
root=Path("/tmp/allpowers-contention")
events=[]
def command(args):
    start=time.monotonic()
    result=subprocess.run(args,capture_output=True,text=True,timeout=15)
    event=dict(utc=datetime.now(timezone.utc).isoformat(),args=args,exit=result.returncode,stdout=result.stdout,stderr=result.stderr,seconds=time.monotonic()-start)
    events.append(event)
    (root/"host-mutations.json").write_text(json.dumps(events,indent=2)+"\n")
    print(json.dumps(event),flush=True)
    return result

def inspect():
    command(["rfkill","--json"])
    for prop in ("Powered","PowerState"):
        command(["busctl","get-property","org.bluez","/org/bluez/hci2","org.bluez.Adapter1",prop])
def test(label):
    result=subprocess.run(["uv","run","--no-project","s300_connection_lab.py",str(root/(label+".jsonl"))],input="note "+label+" controlled host condition; no output writes\nresolve\nrawscan hci2 1\nsnapshot\nquit\n",capture_output=True,text=True,timeout=15)
    (root/(label+"-stdout.txt")).write_text(result.stdout+result.stderr)
    print(result.stdout,flush=True)

if command(["busctl","get-property","org.bluez","/org/bluez/hci2","org.bluez.Adapter1","Powered"]).stdout.strip()!="b true":
    raise SystemExit("Original Actions power state not true; refuse mutation")
initial=command(["rfkill","--json"]).stdout
if any(x["soft"]!="unblocked" or x["hard"]!="unblocked" for x in json.loads(initial)["rfkilldevices"] if x["device"]=="hci2"):
    raise SystemExit("Original rfkill state not clear; refuse mutation")
try:
    if command(["busctl","set-property","org.bluez","/org/bluez/hci2","org.bluez.Adapter1","Powered","b","false"]).returncode==0:
        inspect();test("actions-powered-off")
finally:
    command(["busctl","set-property","org.bluez","/org/bluez/hci2","org.bluez.Adapter1","Powered","b","true"])
    inspect()
try:
    if command(["rfkill","block","2"]).returncode==0:
        time.sleep(0.3)
        inspect();test("actions-rfkill-blocked")
        command(["busctl","set-property","org.bluez","/org/bluez/hci2","org.bluez.Adapter1","Powered","b","true"])
finally:
    command(["rfkill","unblock","2"])
    time.sleep(0.3)
    command(["busctl","set-property","org.bluez","/org/bluez/hci2","org.bluez.Adapter1","Powered","b","true"])
    inspect()
