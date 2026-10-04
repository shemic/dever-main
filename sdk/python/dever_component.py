"""Python Worker for the Dever Component Protocol.

The compiler owns the Port schema. Pass its exact identity to Contract; this
module only transports and decodes values supplied by the application.
"""

import asyncio
import json
import math
import re
import struct
import sys
import uuid
from contextlib import suppress
from dataclasses import dataclass
from typing import Any, Callable


VERSION = "dever-component-1"
MAX_BYTES = 16 * 1024 * 1024
MAX_DEPTH = 64
MAX_ELEMENTS = 65_536
CANCEL_GRACE = 1


class ProtocolError(Exception):
    pass


class BusinessError(Exception):
    def __init__(self, identity: str, payload: Any = None):
        super().__init__(identity)
        self.identity = identity
        self.payload = payload


@dataclass(frozen=True)
class Contract:
    port: str
    schema: str
    adapter: str
    operations: tuple[str, ...]
    capabilities: tuple[str, ...] = ()
    errors: tuple[str, ...] = ()

    def __post_init__(self) -> None:
        for name in ("operations", "capabilities", "errors"):
            values = getattr(self, name)
            if not isinstance(values, (list, tuple)):
                raise ValueError("component contract entries must be lists")
            object.__setattr__(self, name, tuple(values))
        if not all(isinstance(value, str) and value for value in (self.port, self.schema, self.adapter)) or not self.operations:
            raise ValueError("incomplete component contract")
        for names in (self.operations, self.capabilities, self.errors):
            if any(not isinstance(name, str) or not name for name in names) or len(set(names)) != len(names):
                raise ValueError("invalid or duplicate component contract entry")


def _typed(value: Any, schema: dict[str, Any]) -> Any:
    kind = schema["type"]
    if kind == "nullable":
        return None if value is None else _typed(value, schema["value"])
    if kind == "list":
        if not isinstance(value, list):
            raise ValueError("expected list")
        return [_typed(item, schema["value"]) for item in value]
    if kind == "record":
        fields = schema["fields"]
        if not isinstance(value, dict) or not value.keys() <= fields.keys():
            raise ValueError("invalid record fields")
        return {name: _typed(value[name], field) if name in value else
                (None if field["type"] == "nullable" else _missing_field())
                for name, field in fields.items()}
    if kind in ("int64", "duration", "model_id"):
        if type(value) is not int or not -(2**63) <= value < 2**63:
            raise ValueError("expected signed 64-bit integer")
    elif kind == "float64":
        if type(value) not in (int, float) or not math.isfinite(value):
            raise ValueError("expected finite float")
    elif kind == "bool":
        if type(value) is not bool:
            raise ValueError("expected boolean")
    elif kind == "json":
        _bounded(value, 0, [MAX_ELEMENTS])
    elif kind in ("text", "id", "secret", "decimal", "uuid", "datetime", "date", "time"):
        if not isinstance(value, str):
            raise ValueError("expected text")
        if kind == "decimal" and not re.fullmatch(r"[+-]?(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?", value):
            raise ValueError("invalid decimal")
        if kind == "uuid":
            if not re.fullmatch(r"[0-9a-fA-F]{8}(?:-[0-9a-fA-F]{4}){3}-[0-9a-fA-F]{12}", value):
                raise ValueError("invalid UUID")
            uuid.UUID(value)
        if kind == "date":
            import datetime
            if not re.fullmatch(r"\d{4}-\d{2}-\d{2}", value):
                raise ValueError("invalid Date")
            datetime.date.fromisoformat(value)
        if kind == "time":
            import datetime
            if not re.fullmatch(r"\d{2}:\d{2}:\d{2}(?:\.\d{3})?", value):
                raise ValueError("invalid Time")
            datetime.time.fromisoformat(value)
        if kind == "datetime":
            import datetime
            if not re.fullmatch(r"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?(?:Z|[+-]\d{2}:\d{2})", value):
                raise ValueError("invalid DateTime")
            datetime.datetime.fromisoformat(value.replace("Z", "+00:00"))
    else:
        raise ValueError("unknown worker type")
    return value


def _missing_field() -> Any:
    raise ValueError("missing required record field")


def _pairs(values: list[tuple[str, Any]]) -> dict[str, Any]:
    result = {}
    for key, value in values:
        if key in result:
            raise ProtocolError("duplicate JSON field")
        result[key] = value
    return result


def _reject_constant(_: str) -> None:
    raise ProtocolError("invalid JSON number")


def _bounded(value: Any, depth: int, budget: list[int]) -> None:
    if depth > MAX_DEPTH or (depth == MAX_DEPTH and isinstance(value, (dict, list))):
        raise ProtocolError("wire JSON exceeds depth limit")
    budget[0] -= 1
    if budget[0] < 0:
        raise ProtocolError("wire JSON exceeds element limit")
    if isinstance(value, dict):
        for child in value.values():
            _bounded(child, depth + 1, budget)
    elif isinstance(value, list):
        for child in value:
            _bounded(child, depth + 1, budget)
    elif isinstance(value, float) and not math.isfinite(value):
        raise ProtocolError("non-finite wire Float")


def _decode(body: bytes) -> dict[str, Any]:
    try:
        value = json.loads(body.decode("utf-8"), object_pairs_hook=_pairs,
                           parse_constant=_reject_constant)
    except (UnicodeError, ValueError, RecursionError) as error:
        raise ProtocolError("invalid component JSON") from error
    _bounded(value, 0, [MAX_ELEMENTS])
    if not isinstance(value, dict):
        raise ProtocolError("component message must be an object")
    return value


def _fields(message: dict[str, Any], names: set[str]) -> None:
    if message.keys() != names:
        raise ProtocolError("component message has missing or extra fields")


def _id(value: Any) -> int:
    if type(value) is not int or not 0 <= value <= 2**63 - 1:
        raise ProtocolError("invalid request id")
    return value


async def _read_frame(reader: asyncio.StreamReader) -> dict[str, Any]:
    try:
        size = struct.unpack(">I", await reader.readexactly(4))[0]
        if not 0 < size <= MAX_BYTES:
            raise ProtocolError("component frame exceeds byte limit")
        return _decode(await reader.readexactly(size))
    except asyncio.IncompleteReadError as error:
        raise ProtocolError("incomplete component frame") from error


def _write_frame(message: dict[str, Any]) -> None:
    try:
        body = json.dumps(message, ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode("utf-8")
    except (TypeError, ValueError) as error:
        raise ProtocolError("invalid handler response") from error
    _bounded(_decode(body), 0, [MAX_ELEMENTS])
    if not 0 < len(body) <= MAX_BYTES:
        raise ProtocolError("component frame exceeds byte limit")
    sys.__stdout__.buffer.write(struct.pack(">I", len(body)) + body)
    sys.__stdout__.buffer.flush()


async def _stop_handler(task: asyncio.Task) -> None:
    task.cancel()
    completed, _ = await asyncio.wait({task}, timeout=CANCEL_GRACE)
    if not completed:
        raise ProtocolError("component handler did not stop after cancellation")


class Worker:
    def __init__(self, contract: Contract, decode_setting: Callable[[Any], Any]):
        if not callable(decode_setting):
            raise ValueError("component setting decoder is required")
        self.contract = contract
        self.decode_setting = decode_setting
        self.handlers: dict[str, tuple[Callable[[Any], Any], Callable[..., Any]]] = {}
        self.output_types: dict[str, Any] = {}
        self.error_types: dict[str, Any] = {}

    @classmethod
    def from_manifest(cls, manifest: dict[str, Any], handlers: dict[str, Callable[..., Any]] | None = None) -> "Worker":
        operations = tuple(manifest["operations"])
        inputs = manifest["inputs"]
        outputs = manifest["outputs"]
        errors = manifest["errors"]
        if set(inputs) != set(operations) or set(outputs) != set(operations):
            raise ValueError("incomplete typed Worker contract")
        contract = Contract(manifest["port"], manifest["schema"], manifest["adapter"],
                            operations, tuple(manifest["capabilities"]), tuple(errors))
        setting = manifest["setting"]
        worker = cls(contract, lambda value: _typed(value, setting) if setting is not None else
                     (None if value is None else _missing_field()))
        worker.input_types = inputs
        worker.output_types = outputs
        worker.error_types = errors
        if handlers is not None:
            if set(handlers) != set(operations):
                raise ValueError("component operations are not fully registered")
            for operation, handler in handlers.items():
                worker.register_handler(operation, handler)
        return worker

    def register_handler(self, operation: str, handler: Callable[..., Any]) -> None:
        if not hasattr(self, "input_types") or operation not in self.input_types:
            raise ValueError("operation is not in a typed Worker contract")
        self.register(operation, lambda value: _typed(value, self.input_types[operation]), handler)

    def register(self, operation: str, decode_payload: Callable[[Any], Any], handler: Callable[..., Any]) -> None:
        if operation not in self.contract.operations or operation in self.handlers:
            raise ValueError("unknown or duplicate operation")
        if not callable(decode_payload):
            raise ValueError("component payload decoder is required")
        if not asyncio.iscoroutinefunction(handler):
            raise ValueError("component handler must be async")
        self.handlers[operation] = (decode_payload, handler)

    async def serve(self) -> None:
        if set(self.handlers) != set(self.contract.operations):
            raise ValueError("component operations are not fully registered")
        loop = asyncio.get_running_loop()
        reader = asyncio.StreamReader()
        transport, _ = await loop.connect_read_pipe(
            lambda: asyncio.StreamReaderProtocol(reader), sys.stdin.buffer
        )
        try:
            await self._serve(reader)
        finally:
            transport.close()

    async def _serve(self, reader: asyncio.StreamReader) -> None:
        hello = await _read_frame(reader)
        _fields(hello, {"kind", "version", "port", "schema", "adapter", "capabilities", "operations", "setting"})
        expected = self.contract
        if (hello["kind"], hello["version"], hello["port"], hello["schema"], hello["adapter"], hello["capabilities"], hello["operations"]) != ("hello", VERSION, expected.port, expected.schema, expected.adapter, list(expected.capabilities), list(expected.operations)):
            raise ProtocolError("component handshake does not match registered contract")
        try:
            setting = self.decode_setting(hello["setting"])
        except Exception as error:
            raise ProtocolError("invalid component setting") from error
        _write_frame({"kind": "ready", "version": VERSION, "port": expected.port, "schema": expected.schema, "adapter": expected.adapter, "capabilities": list(expected.capabilities), "operations": list(expected.operations)})

        messages: asyncio.Queue = asyncio.Queue(maxsize=4)

        async def read_messages() -> None:
            while True:
                try:
                    message = await _read_frame(reader)
                except (OSError, ProtocolError) as error:
                    await messages.put(("fault", error))
                    return
                await messages.put(("frame", message))

        receiver = asyncio.create_task(read_messages())
        try:
            await self._dispatch(messages, setting)
        finally:
            # Cancel and join the pipe reader before Python finalizes stdio.
            receiver.cancel()
            with suppress(asyncio.CancelledError):
                await receiver

    async def _dispatch(self, messages: asyncio.Queue, setting: Any) -> None:
        loop = asyncio.get_running_loop()
        active_id: int | None = None
        active_task: asyncio.Task | None = None
        active_operation: str | None = None
        next_id = 1
        last_cancelled = False
        completions: set[asyncio.Task] = set()

        def on_done(task: asyncio.Task) -> None:
            try:
                task.exception()
            except asyncio.CancelledError:
                pass
            notification = loop.create_task(messages.put(("done", task)))
            completions.add(notification)
            notification.add_done_callback(completions.discard)

        try:
            while True:
                event, value = await messages.get()
                if event == "fault":
                    raise value
                if event == "done":
                    task = value
                    if task is active_task:
                        try:
                            payload = task.result()
                            if self.output_types:
                                payload = _typed(payload, self.output_types[active_operation])
                            _write_frame({"kind": "result", "id": active_id, "payload": payload})
                        except BusinessError as error:
                            if error.identity not in self.contract.errors:
                                raise ProtocolError("undeclared business error identity") from error
                            if self.error_types:
                                error.payload = _typed(error.payload, self.error_types[error.identity])
                            _write_frame({"kind": "error", "id": active_id, "error": error.identity, "payload": error.payload})
                        except asyncio.CancelledError:
                            pass
                        except Exception as error:
                            raise ProtocolError("component handler failed") from error
                        active_id, active_task, active_operation = None, None, None
                        last_cancelled = False
                    continue
                message = value
                kind = message.get("kind")
                if kind == "health":
                    _fields(message, {"kind", "id"})
                    if _id(message["id"]) != 0:
                        raise ProtocolError("invalid health id")
                    _write_frame(message)
                elif kind == "call":
                    _fields(message, {"kind", "id", "operation", "payload"})
                    request_id = _id(message["id"])
                    operation = message["operation"]
                    if request_id != next_id or active_task is not None or not isinstance(operation, str) or operation not in self.handlers:
                        raise ProtocolError("invalid component call")
                    next_id += 1
                    decoder, handler = self.handlers[operation]
                    try:
                        payload = decoder(message["payload"])
                    except Exception as error:
                        raise ProtocolError("invalid component payload") from error
                    async def invoke() -> Any:
                        return await handler(payload, setting)
                    active_id = request_id
                    active_operation = operation
                    active_task = loop.create_task(invoke())
                    active_task.add_done_callback(on_done)
                elif kind == "cancel":
                    _fields(message, {"kind", "id"})
                    request_id = _id(message["id"])
                    if request_id == 0:
                        raise ProtocolError("invalid cancel id")
                    if active_id == request_id and active_task is not None:
                        stopped_task = active_task
                        active_id, active_task, active_operation = None, None, None
                        await _stop_handler(stopped_task)
                        last_cancelled = True
                        _write_frame({"kind": "error", "id": request_id, "error": "dever.cancelled", "payload": None})
                    elif request_id != next_id - 1 or last_cancelled:
                        raise ProtocolError("unknown component cancel id")
                elif kind == "shutdown":
                    _fields(message, {"kind"})
                    if active_task is not None:
                        stopped_task = active_task
                        active_id, active_task = None, None
                        await _stop_handler(stopped_task)
                    _write_frame({"kind": "shutdown"})
                    return
                else:
                    raise ProtocolError("unknown component message")

        finally:
            if active_task is not None:
                active_task.remove_done_callback(on_done)
            for notification in completions:
                notification.cancel()
            await asyncio.gather(*completions, return_exceptions=True)


def main(worker: Worker) -> None:
    loop = asyncio.new_event_loop()
    try:
        asyncio.set_event_loop(loop)
        loop.run_until_complete(worker.serve())
    except (ProtocolError, OSError, ValueError) as error:
        print(f"component worker: {error}", file=sys.stderr)
        raise SystemExit(1) from None
    finally:
        for task in asyncio.all_tasks(loop):
            task.cancel()
        loop.close()
        asyncio.set_event_loop(None)
