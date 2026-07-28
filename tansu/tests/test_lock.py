"""Tests for the Postgres advisory-lock coordination in `tansu.events.lock`.

This is the primitive `backfill_from_genesis` and `ingest.run_poll_cycle`
both use to guarantee they never process overlapping ledger ranges for the
same contract concurrently.
"""

import pytest

from tansu.events import lock
from tansu.events.database.session_factory import engine


async def test_contract_lock_rejects_concurrent_holder():
    contract_id = "lock-test-contract"

    async with engine.connect() as conn_a, engine.connect() as conn_b:
        async with lock.contract_lock(conn_a, contract_id):
            with pytest.raises(lock.ContractLockedError):
                async with lock.contract_lock(conn_b, contract_id):
                    pass  # pragma: no cover - must not be reached

        # Released once the first holder's block exits, so a fresh attempt succeeds.
        async with lock.contract_lock(conn_b, contract_id):
            pass


async def test_contract_lock_is_scoped_per_contract_id():
    async with engine.connect() as conn_a, engine.connect() as conn_b:
        async with lock.contract_lock(conn_a, "contract-a"):
            # A different contract id is a different lock: no contention.
            async with lock.contract_lock(conn_b, "contract-b"):
                pass
