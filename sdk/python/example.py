"""Runnable protocol example with a fixed compiler-owned fixture schema."""

import asyncio
from dever_component import BusinessError, Contract, Worker, main


def setting(value):
    if not isinstance(value, dict) or set(value) != {"prefix"} or not isinstance(value["prefix"], str):
        raise ValueError("invalid setting")
    return value["prefix"]


def payload(value):
    if not isinstance(value, dict) or set(value) != {"text", "delay_ms"}:
        raise ValueError("invalid payload")
    if not isinstance(value["text"], str) or type(value["delay_ms"]) is not int or not 0 <= value["delay_ms"] <= 10_000:
        raise ValueError("invalid payload")
    return value


async def render(value, prefix):
    await asyncio.sleep(value["delay_ms"] / 1000)
    return {"value": prefix + value["text"]}


async def fail(value, prefix):
    raise BusinessError("example.rejected", {"reason": "rejected"})


def number(value):
    if not isinstance(value, dict) or set(value) != {"number"} or type(value["number"]) is not int:
        raise ValueError("invalid number")
    return value["number"]


async def echo_number(value, prefix):
    return {"number": value}


async def stubborn(value, prefix):
    try:
        await asyncio.sleep(5)
    except asyncio.CancelledError:
        await asyncio.sleep(5)
    return {"value": prefix + value["text"]}


worker = Worker(Contract("example.Text", "fixture-schema-v1", "example.TextAdapter", ("text.render", "text.fail", "number.echo", "text.stubborn"), errors=("example.rejected",)), setting)
worker.register("text.render", payload, render)
worker.register("text.fail", payload, fail)
worker.register("number.echo", number, echo_number)
worker.register("text.stubborn", payload, stubborn)

if __name__ == "__main__":
    main(worker)
