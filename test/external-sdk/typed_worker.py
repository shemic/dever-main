import json
from pathlib import Path
import sys

sys.path.insert(0, str(Path(__file__).resolve().parents[2] / "sdk/python"))
from dever_component import BusinessError, Worker, main


async def render(payload, setting):
    message = payload["message"]
    if message["name"] == "reject":
        raise BusinessError("example.Rejected", {"reason": "rejected"})
    return {"text": setting["prefix"] + message["name"] + str(message["number"])}


manifest = json.loads(Path(sys.argv[1]).read_text())
main(Worker.from_manifest(manifest, {"render": render}))
