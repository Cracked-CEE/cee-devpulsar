"""Postgres advisory-lock coordination.

Ensures a genesis backfill (:mod:`tansu.events.backfill`) and the regular
``fetch_events`` poll cycle never process overlapping ledger ranges for the
same contract at the same time. Advisory locks are connection-scoped in
Postgres, so the caller must keep the connection open for as long as the
lock needs to be held -- that's why this takes an already-checked-out
``AsyncConnection`` rather than a session/engine.
"""

import hashlib
from contextlib import asynccontextmanager

import sqlalchemy
from sqlalchemy.ext.asyncio import AsyncConnection


class ContractLockedError(RuntimeError):
    """Another process (backfill or poll cycle) already holds the contract lock."""


def lock_key(contract_id: str) -> int:
    """Stable key for `contract_id`, fit for Postgres's signed-bigint advisory lock id."""
    digest = hashlib.sha256(contract_id.encode()).digest()[:8]
    return int.from_bytes(digest, "big") % (2**63)


@asynccontextmanager
async def contract_lock(conn: AsyncConnection, contract_id: str):
    """Hold the advisory lock for `contract_id` for the duration of the block.

    Non-blocking: raises `ContractLockedError` immediately if another
    process already holds it, instead of queueing behind it. Callers (the
    backfill and the regular poller) are expected to treat that as "skip
    this cycle/run", not retry in a tight loop.
    """
    key = lock_key(contract_id)
    acquired = (
        await conn.execute(sqlalchemy.select(sqlalchemy.func.pg_try_advisory_lock(key)))
    ).scalar_one()
    if not acquired:
        raise ContractLockedError(
            f"contract {contract_id!r} is locked by another backfill/poll process"
        )
    try:
        yield
    finally:
        await conn.execute(sqlalchemy.select(sqlalchemy.func.pg_advisory_unlock(key)))
