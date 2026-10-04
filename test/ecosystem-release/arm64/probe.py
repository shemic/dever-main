import asyncio
import gzip
import hashlib
from pathlib import Path
import platform
import sqlite3
import sys

import six


async def probe(payload, setting):
    assert platform.machine() == "aarch64"
    assert six.__version__ == "1.17.0" and six.PY3
    assert list(six.iteritems({"value": 42})) == [("value", 42)]
    runtime = Path(sys.executable).resolve().parents[1]
    bundle = runtime.parent
    assert Path(sys.prefix).resolve() == runtime
    assert all(Path(path).resolve().is_relative_to(bundle) for path in sys.path)
    assert Path(six.__file__).resolve().is_relative_to(bundle)
    message = "ARM 独立依赖".encode()
    assert gzip.decompress(gzip.compress(message)) == message
    assert len(hashlib.sha256(message).digest()) == 32
    with sqlite3.connect(":memory:") as connection:
        assert connection.execute("select 6 * 7").fetchone() == (42,)
    await asyncio.sleep(0)
    return {"okay": True}


async def reject(payload, setting):
    raise RuntimeError("expected ARM Worker failure")
