import asyncio
import bz2
import ctypes
import decimal
import gzip
import hashlib
import lzma
from pathlib import Path
import sqlite3
import ssl
import sys
import simplejson
from simplejson import _speedups


async def probe(payload, setting):
    assert _speedups.__file__.endswith(".so")
    assert _speedups.scanstring('"native"', 1) == ("native", 8)
    assert simplejson.loads(simplejson.dumps({"native": 42})) == {"native": 42}
    runtime = Path(sys.executable).resolve().parents[1]
    bundle = runtime.parent
    assert Path(sys.prefix).resolve() == runtime
    assert all(Path(path).resolve().is_relative_to(bundle) for path in sys.path)
    for module in tuple(sys.modules.values()):
        path = getattr(module, "__file__", None)
        if path and not path.startswith("<"):
            assert Path(path).resolve().is_relative_to(bundle), path
    await asyncio.sleep(0)
    message = "独立标准库".encode()
    for compress, decompress in ((gzip.compress, gzip.decompress), (bz2.compress, bz2.decompress), (lzma.compress, lzma.decompress)):
        assert decompress(compress(message)) == message
    assert len(hashlib.sha256(message).digest()) == 32
    assert ssl.OPENSSL_VERSION.startswith("OpenSSL ")
    assert ctypes.sizeof(ctypes.c_uint64) == 8
    assert decimal.Decimal("0.1") + decimal.Decimal("0.2") == decimal.Decimal("0.3")
    with sqlite3.connect(":memory:") as connection:
        assert connection.execute("select 6 * 7").fetchone() == (42,)
    return {"okay": True}
