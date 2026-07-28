"""Tests for `tansu.events.backfill.backfill_from_genesis`.

Uses the same `FlakyRpcServer` double as `test_chaos_ingest.py` (it already
models cursor-based RPC pagination) plus a thin wrapper that fails
persistently after a chosen call, to simulate a process crash mid-backfill.
The core acceptance criterion: running once, interrupted, then resumed
produces the same final event set as a single uninterrupted run.
"""

import pytest
from sqlalchemy import delete, func, select
from stellar_sdk.exceptions import ConnectionError as SorobanConnectionError

from tansu.events import backfill, ingest
from tansu.events.database import db_models
from tansu.events.database.session_factory import SessionFactory
from tests.conftest import FlakyRpcServer, make_soroban_event

CONTRACT_ID = "CDLZFC3SYJYDZT7K67VZ75HPJVIEUVNIXF47ZG2FB2RMQQVU2HHGCYSC"


def _events(project_key: str, n: int, *, start_ledger: int = 100):
    return [
        make_soroban_event(
            ledger=start_ledger + i,
            action="commit",
            project_key=project_key,
            value=f"{project_key}-value-{i}",
        )
        for i in range(n)
    ]


async def _count(project_key: str) -> int:
    async with SessionFactory() as session:
        result = await session.execute(
            select(func.count())
            .select_from(db_models.Event)
            .where(db_models.Event.project_key == project_key)
        )
        return result.scalar_one()


async def _purge(project_key: str) -> None:
    async with SessionFactory() as session:
        async with session.begin():
            await session.execute(
                delete(db_models.Event).where(
                    db_models.Event.project_key == project_key
                )
            )
            await session.execute(
                delete(db_models.BackfillCheckpoint).where(
                    db_models.BackfillCheckpoint.contract_id == CONTRACT_ID
                )
            )


class BreakingRpcServer:
    """Delegates to `inner`, but fails every call from `fail_after` onward.

    Models a Soroban RPC / process crash: the backfill gets partway through
    (however many pages succeeded before `fail_after`) and then the whole
    call chain starts erroring, so retries can't paper over it either.
    """

    def __init__(self, inner: FlakyRpcServer, fail_after: int) -> None:
        self._inner = inner
        self._fail_after = fail_after
        self.get_events_calls = 0

    def get_latest_ledger(self):
        return self._inner.get_latest_ledger()

    def get_events(self, *args, **kwargs):
        self.get_events_calls += 1
        if self.get_events_calls >= self._fail_after:
            raise SorobanConnectionError("simulated crash mid-backfill")
        return self._inner.get_events(*args, **kwargs)


@pytest.fixture(autouse=True)
async def _cleanup():
    yield
    await _purge("backfill-full")
    await _purge("backfill-resume")
    await _purge("backfill-noop")
    await _purge("backfill-genesis-mismatch")


async def test_full_backfill_reaches_tip(monkeypatch):
    project_key = "backfill-full"
    server = FlakyRpcServer(
        events=_events(project_key, 10, start_ledger=100),
        latest_ledger=200,
        oldest_ledger=50,
        page_size=4,
    )
    monkeypatch.setattr(ingest.stellar_sdk, "SorobanServer", lambda *a, **k: server)

    checkpoint = await backfill.backfill_from_genesis(
        CONTRACT_ID,
        genesis_ledger=100,
        checkpoint_interval=1,
        page_limit=4,
        page_delay_seconds=0,
    )

    assert await _count(project_key) == 10
    assert checkpoint.completed_at is not None
    assert checkpoint.last_completed_ledger == 200
    assert checkpoint.tip_ledger == 200


async def test_resume_after_interruption_matches_full_backfill(monkeypatch):
    project_key = "backfill-resume"
    total_events = 12
    inner = FlakyRpcServer(
        events=_events(project_key, total_events, start_ledger=100),
        latest_ledger=300,
        oldest_ledger=50,
        page_size=3,
    )
    breaking = BreakingRpcServer(inner, fail_after=3)  # 2 pages (6 events) succeed
    monkeypatch.setattr(ingest.stellar_sdk, "SorobanServer", lambda *a, **k: breaking)

    with pytest.raises(SorobanConnectionError):
        await backfill.backfill_from_genesis(
            CONTRACT_ID,
            genesis_ledger=100,
            checkpoint_interval=1,
            page_limit=3,
            page_delay_seconds=0,
            max_retries=0,
        )

    # Partial progress survived: 2 pages worth of events, checkpoint not done.
    assert await _count(project_key) == 6
    async with SessionFactory() as session:
        partial = await session.get(db_models.BackfillCheckpoint, CONTRACT_ID)
    assert partial.completed_at is None
    assert partial.last_completed_ledger == 105
    assert partial.tip_ledger == 300  # pinned on the first run

    # Resume against the same underlying server (past the failure point).
    monkeypatch.setattr(ingest.stellar_sdk, "SorobanServer", lambda *a, **k: inner)
    checkpoint = await backfill.backfill_from_genesis(
        CONTRACT_ID,
        genesis_ledger=100,
        checkpoint_interval=1,
        page_limit=3,
        page_delay_seconds=0,
    )

    assert await _count(project_key) == total_events
    assert checkpoint.completed_at is not None
    assert checkpoint.last_completed_ledger == 300
    assert checkpoint.tip_ledger == 300


async def test_completed_backfill_is_noop(monkeypatch):
    project_key = "backfill-noop"
    server = FlakyRpcServer(
        events=_events(project_key, 5, start_ledger=100),
        latest_ledger=150,
        oldest_ledger=50,
    )
    monkeypatch.setattr(ingest.stellar_sdk, "SorobanServer", lambda *a, **k: server)

    await backfill.backfill_from_genesis(
        CONTRACT_ID, genesis_ledger=100, checkpoint_interval=1000, page_delay_seconds=0
    )
    assert await _count(project_key) == 5
    calls_after_first_run = server.get_events_calls

    def _boom(*a, **k):
        raise AssertionError("should not contact the RPC once already completed")

    monkeypatch.setattr(ingest.stellar_sdk, "SorobanServer", _boom)

    checkpoint = await backfill.backfill_from_genesis(
        CONTRACT_ID, genesis_ledger=100, checkpoint_interval=1000, page_delay_seconds=0
    )

    assert await _count(project_key) == 5
    assert checkpoint.completed_at is not None
    assert server.get_events_calls == calls_after_first_run


async def test_mismatched_genesis_raises(monkeypatch):
    project_key = "backfill-genesis-mismatch"
    server = FlakyRpcServer(
        events=_events(project_key, 3, start_ledger=100),
        latest_ledger=150,
        oldest_ledger=50,
    )
    monkeypatch.setattr(ingest.stellar_sdk, "SorobanServer", lambda *a, **k: server)

    await backfill.backfill_from_genesis(
        CONTRACT_ID, genesis_ledger=100, checkpoint_interval=1000, page_delay_seconds=0
    )

    with pytest.raises(ValueError):
        await backfill.backfill_from_genesis(
            CONTRACT_ID,
            genesis_ledger=50,  # different genesis than the persisted checkpoint
            checkpoint_interval=1000,
            page_delay_seconds=0,
        )
